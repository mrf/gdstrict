//! Severity configuration: map each Godot warning code to `error | warn | off`.
//!
//! This is the project-wide "warnings-as-errors" enforcement Godot lacks. A
//! `gdstrict.toml` profile resolves every warning code to an [`Action`]; the
//! built-in `strict` [`Preset`] turns the strict-typing family into hard errors.
//!
//! The gdstrict CLI is the single parse authority for `gdstrict.toml`; it uses
//! the `toml` crate and feeds the strict half back via [`SeverityConfig::from_parts`].
//! [`Action::parse`] and [`Preset::parse`] are the shared canonical token decoders.

use crate::{codes, Diagnostic, Severity};
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
    /// Parse a severity action token (`error` | `warn`/`warning` | `off`).
    ///
    /// Public so a host that parses `gdstrict.toml` with a full TOML library (the
    /// CLI) can reuse this canonical token set instead of duplicating it.
    pub fn parse(s: &str) -> Option<Action> {
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
    /// Parse a preset name (only `strict` is known).
    ///
    /// Public for the same reason as [`Action::parse`]: the CLI parses the unified
    /// `gdstrict.toml` with the `toml` crate and reuses this to validate `preset`.
    pub fn parse(s: &str) -> Option<Preset> {
        match s {
            "strict" => Some(Preset::Strict),
            _ => None,
        }
    }

    /// The preset's action for a code, before any user overrides.
    fn action(self, code: &str) -> Action {
        match self {
            Preset::Strict => match code {
                codes::UNTYPED_DECLARATION
                | codes::INFERRED_DECLARATION
                | codes::RETURN_VALUE_DISCARDED => Action::Error,
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

    /// Build a config from an already-parsed preset and per-code overrides.
    ///
    /// Used by the CLI: it parses the unified `gdstrict.toml` with the `toml` crate
    /// and hands the strict half back here without re-parsing.
    pub fn from_parts(preset: Option<Preset>, overrides: HashMap<String, Action>) -> Self {
        SeverityConfig { preset, overrides }
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
            codes::UNTYPED_DECLARATION,
            codes::INFERRED_DECLARATION,
            codes::RETURN_VALUE_DISCARDED,
            codes::UNSAFE_METHOD_ACCESS,
            codes::UNSAFE_PROPERTY_ACCESS,
            codes::UNSAFE_CAST,
            "UNSAFE_CALL_ARGUMENT", // not yet classified, but the family rule still promotes it
        ] {
            assert_eq!(cfg.action_for(code), Action::Error, "code {code}");
        }
    }

    #[test]
    fn strict_preset_leaves_others_as_warn() {
        let cfg = SeverityConfig::strict();
        assert_eq!(cfg.action_for(codes::INTEGER_DIVISION), Action::Warn);
        assert_eq!(cfg.action_for("SOMETHING_ELSE"), Action::Warn);
    }

    #[test]
    fn no_preset_defaults_to_warn() {
        let cfg = SeverityConfig::default();
        assert_eq!(cfg.action_for(codes::UNTYPED_DECLARATION), Action::Warn);
    }

    // ---- apply() ----------------------------------------------------------

    #[test]
    fn apply_promotes_errors_and_filters_off() {
        let cfg = {
            let mut overrides = std::collections::HashMap::new();
            overrides.insert(codes::INTEGER_DIVISION.to_string(), Action::Off);
            SeverityConfig::from_parts(Some(Preset::Strict), overrides)
        };
        let raw = vec![
            warn(Some(codes::UNTYPED_DECLARATION)),  // -> error
            warn(Some(codes::INTEGER_DIVISION)),     // -> dropped (off)
            warn(Some(codes::UNSAFE_METHOD_ACCESS)), // -> error
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
        assert!(out.iter().all(|d| d.code != Some(codes::INTEGER_DIVISION)));
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
        let out = cfg.apply(vec![warn(Some(codes::UNTYPED_DECLARATION)), warn(None)]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.severity == Severity::Warning));
    }
}
