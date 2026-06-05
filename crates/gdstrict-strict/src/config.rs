//! Severity configuration: map each Godot warning code to `error | warn | off`.
//!
//! This is the project-wide "warnings-as-errors" enforcement Godot lacks. A
//! `gdstrict.toml` profile resolves every warning code to an [`Action`]; the
//! built-in `strict` [`Preset`] turns the strict-typing family into hard errors.
//!
//! ## Config format (a deliberately small TOML subset)
//!
//! ```toml
//! preset = "strict"          # optional; known: "strict"
//!
//! [warnings]                 # optional per-code overrides (beat the preset)
//! INTEGER_DIVISION = "off"
//! RETURN_VALUE_DISCARDED = "warn"
//! ```
//!
//! We hand-parse this subset rather than depend on the full `toml` crate (which
//! pulls in serde_derive + syn): the grammar here is fixed and tiny. If the
//! config surface grows beyond key/value + the `[warnings]` table, swap in the
//! `toml` crate.

use crate::{Diagnostic, Severity};
use std::collections::HashMap;

/// What gdstrict does with a diagnostic of a given warning code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Suppress the diagnostic entirely.
    Off,
    /// Keep it as a non-failing warning.
    Warn,
    /// Promote it to an error (makes `check` fail).
    Error,
}

impl Action {
    fn parse(s: &str) -> Option<Action> {
        match s {
            "error" => Some(Action::Error),
            "warn" | "warning" => Some(Action::Warn),
            "off" => Some(Action::Off),
            _ => None,
        }
    }
}

/// A named bundle of per-code defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Warnings-as-errors for the strict-typing family: `UNTYPED_DECLARATION`,
    /// `INFERRED_DECLARATION`, the `UNSAFE_*` family, and `RETURN_VALUE_DISCARDED`.
    Strict,
}

impl Preset {
    fn parse(s: &str) -> Option<Preset> {
        match s {
            "strict" => Some(Preset::Strict),
            _ => None,
        }
    }

    /// The preset's action for a code, before any user overrides.
    fn action(self, code: &str) -> Action {
        match self {
            Preset::Strict => match code {
                "UNTYPED_DECLARATION" | "INFERRED_DECLARATION" | "RETURN_VALUE_DISCARDED" => {
                    Action::Error
                }
                c if c.starts_with("UNSAFE_") => Action::Error,
                _ => Action::Warn,
            },
        }
    }
}

/// Resolves each warning code to an [`Action`].
///
/// Precedence, highest first: an explicit `[warnings]` override, then the
/// [`Preset`] default, then the global fallback ([`Action::Warn`] — warnings
/// stay warnings unless something promotes or silences them).
#[derive(Debug, Clone, Default)]
pub struct SeverityConfig {
    preset: Option<Preset>,
    overrides: HashMap<String, Action>,
}

impl SeverityConfig {
    /// The built-in `strict` preset with no overrides.
    pub fn strict() -> Self {
        SeverityConfig {
            preset: Some(Preset::Strict),
            overrides: HashMap::new(),
        }
    }

    /// Resolve the configured action for a warning `code`.
    pub fn action_for(&self, code: &str) -> Action {
        if let Some(a) = self.overrides.get(code) {
            return *a;
        }
        if let Some(p) = self.preset {
            return p.action(code);
        }
        Action::Warn
    }

    /// Apply this config to raw diagnostics: drop `off` warnings, promote
    /// `error` warnings to [`Severity::Error`], and pass hard errors through
    /// untouched (the severity map governs *warnings*, never real parse errors).
    ///
    /// Warnings with no recognized code use the global/preset fallback, which is
    /// always [`Action::Warn`] (we can't key a preset on an unknown code).
    pub fn apply(&self, diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
        diags
            .into_iter()
            .filter_map(|mut d| {
                if d.severity == Severity::Error {
                    return Some(d);
                }
                let action = match d.code {
                    Some(code) => self.action_for(code),
                    None => Action::Warn,
                };
                match action {
                    Action::Off => None,
                    Action::Warn => Some(d),
                    Action::Error => {
                        d.severity = Severity::Error;
                        Some(d)
                    }
                }
            })
            .collect()
    }
}

/// A parse failure in a `gdstrict.toml` severity profile, with the 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Malformed { line: usize, text: String },
    UnknownSection { line: usize, name: String },
    UnknownKey { line: usize, name: String },
    UnknownPreset { line: usize, name: String },
    UnknownAction { line: usize, value: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Malformed { line, text } => {
                write!(f, "line {line}: expected `key = value`, got `{text}`")
            }
            ConfigError::UnknownSection { line, name } => write!(
                f,
                "line {line}: unknown section [{name}] (only [warnings] is supported)"
            ),
            ConfigError::UnknownKey { line, name } => {
                write!(f, "line {line}: unknown key `{name}`")
            }
            ConfigError::UnknownPreset { line, name } => {
                write!(f, "line {line}: unknown preset `{name}` (known: strict)")
            }
            ConfigError::UnknownAction { line, value } => write!(
                f,
                "line {line}: unknown action `{value}` (expected error|warn|off)"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Parse a `gdstrict.toml` severity profile (the small subset documented above).
pub fn parse(src: &str) -> Result<SeverityConfig, ConfigError> {
    let mut cfg = SeverityConfig::default();
    let mut in_warnings = false;
    for (i, raw) in src.lines().enumerate() {
        let lineno = i + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            let section = inner.trim();
            if section != "warnings" {
                return Err(ConfigError::UnknownSection {
                    line: lineno,
                    name: section.to_string(),
                });
            }
            in_warnings = true;
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| ConfigError::Malformed {
            line: lineno,
            text: line.to_string(),
        })?;
        let key = key.trim();
        let value = unquote(value.trim());
        if in_warnings {
            let action = Action::parse(&value).ok_or_else(|| ConfigError::UnknownAction {
                line: lineno,
                value: value.clone(),
            })?;
            cfg.overrides.insert(key.to_string(), action);
        } else {
            match key {
                "preset" => {
                    cfg.preset =
                        Some(
                            Preset::parse(&value).ok_or_else(|| ConfigError::UnknownPreset {
                                line: lineno,
                                name: value.clone(),
                            })?,
                        );
                }
                other => {
                    return Err(ConfigError::UnknownKey {
                        line: lineno,
                        name: other.to_string(),
                    });
                }
            }
        }
    }
    Ok(cfg)
}

/// Trim a trailing `#` line comment, ignoring `#` inside a double-quoted string.
fn strip_comment(line: &str) -> &str {
    let mut in_quote = false;
    for (idx, b) in line.bytes().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b'#' if !in_quote => return &line[..idx],
            _ => {}
        }
    }
    line
}

/// Strip surrounding double quotes if present; otherwise return as-is.
fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warn(code: Option<&'static str>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Warning,
            file: "res://x.gd".to_string(),
            line: 1,
            code,
            message: "m".to_string(),
        }
    }

    // ---- preset semantics -------------------------------------------------

    #[test]
    fn strict_preset_promotes_typing_family() {
        let cfg = SeverityConfig::strict();
        for code in [
            "UNTYPED_DECLARATION",
            "INFERRED_DECLARATION",
            "RETURN_VALUE_DISCARDED",
            "UNSAFE_METHOD_ACCESS",
            "UNSAFE_PROPERTY_ACCESS",
            "UNSAFE_CAST",
            "UNSAFE_CALL_ARGUMENT", // not yet classified, but the family rule still promotes it
        ] {
            assert_eq!(cfg.action_for(code), Action::Error, "code {code}");
        }
    }

    #[test]
    fn strict_preset_leaves_others_as_warn() {
        let cfg = SeverityConfig::strict();
        assert_eq!(cfg.action_for("INTEGER_DIVISION"), Action::Warn);
        assert_eq!(cfg.action_for("SOMETHING_ELSE"), Action::Warn);
    }

    #[test]
    fn no_preset_defaults_to_warn() {
        let cfg = SeverityConfig::default();
        assert_eq!(cfg.action_for("UNTYPED_DECLARATION"), Action::Warn);
    }

    // ---- parsing (positive) ----------------------------------------------

    #[test]
    fn parses_preset_only() {
        let cfg = parse("preset = \"strict\"\n").unwrap();
        assert_eq!(cfg.action_for("UNSAFE_CAST"), Action::Error);
    }

    #[test]
    fn override_beats_preset() {
        let src = "preset = \"strict\"\n\n[warnings]\nUNTYPED_DECLARATION = \"off\"\nINTEGER_DIVISION = \"error\"\n";
        let cfg = parse(src).unwrap();
        assert_eq!(cfg.action_for("UNTYPED_DECLARATION"), Action::Off); // override wins
        assert_eq!(cfg.action_for("UNSAFE_CAST"), Action::Error); // still from preset
        assert_eq!(cfg.action_for("INTEGER_DIVISION"), Action::Error); // raised by override
    }

    #[test]
    fn handles_comments_blank_lines_and_warning_alias() {
        let src = "# top comment\npreset = \"strict\"   # inline\n\n[warnings]  # section\n  RETURN_VALUE_DISCARDED = \"warning\"  # alias for warn\n";
        let cfg = parse(src).unwrap();
        assert_eq!(cfg.action_for("RETURN_VALUE_DISCARDED"), Action::Warn);
    }

    #[test]
    fn accepts_bare_unquoted_values() {
        let cfg = parse("preset = strict\n[warnings]\nINTEGER_DIVISION = off\n").unwrap();
        assert_eq!(cfg.action_for("UNSAFE_CAST"), Action::Error);
        assert_eq!(cfg.action_for("INTEGER_DIVISION"), Action::Off);
    }

    #[test]
    fn empty_config_is_all_warn() {
        let cfg = parse("\n# just a comment\n\n").unwrap();
        assert_eq!(cfg.action_for("UNTYPED_DECLARATION"), Action::Warn);
    }

    // ---- parsing (negative) ----------------------------------------------

    #[test]
    fn rejects_unknown_preset() {
        let err = parse("preset = \"pedantic\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownPreset { line: 1, .. }));
    }

    #[test]
    fn rejects_unknown_action() {
        let err = parse("[warnings]\nUNTYPED_DECLARATION = \"fatal\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownAction { line: 2, .. }));
    }

    #[test]
    fn rejects_unknown_section() {
        let err = parse("[format]\nx = 1\n").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownSection { line: 1, .. }));
    }

    #[test]
    fn rejects_unknown_toplevel_key() {
        let err = parse("line_length = 100\n").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownKey { line: 1, .. }));
    }

    #[test]
    fn rejects_malformed_line() {
        let err = parse("preset\n").unwrap_err();
        assert!(matches!(err, ConfigError::Malformed { line: 1, .. }));
    }

    // ---- apply() ----------------------------------------------------------

    #[test]
    fn apply_promotes_filters_and_preserves_errors() {
        let cfg = parse("preset = \"strict\"\n[warnings]\nINTEGER_DIVISION = \"off\"\n").unwrap();
        let raw = vec![
            warn(Some("UNTYPED_DECLARATION")),  // -> error
            warn(Some("INTEGER_DIVISION")),     // -> dropped (off)
            warn(Some("UNSAFE_METHOD_ACCESS")), // -> error
            Diagnostic {
                severity: Severity::Error, // hard error: untouched
                file: "res://x.gd".to_string(),
                line: 9,
                code: None,
                message: "Parse Error".to_string(),
            },
        ];
        let out = cfg.apply(raw);
        assert_eq!(out.len(), 3); // INTEGER_DIVISION dropped
        let promoted = out.iter().filter(|d| d.severity == Severity::Error).count();
        assert_eq!(promoted, 3); // two promoted warnings + the original error
        assert!(out.iter().all(|d| d.code != Some("INTEGER_DIVISION")));
    }

    #[test]
    fn apply_uncoded_warning_stays_warning() {
        let cfg = SeverityConfig::strict();
        let out = cfg.apply(vec![warn(None)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, Severity::Warning);
    }

    #[test]
    fn apply_default_config_is_passthrough() {
        let cfg = SeverityConfig::default();
        let out = cfg.apply(vec![warn(Some("UNTYPED_DECLARATION")), warn(None)]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.severity == Severity::Warning));
    }
}
