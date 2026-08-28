//! Config discovery + resolution (black/ruff style).
//!
//! A single `gdstrict.toml` is the one place a project configures every gdstrict
//! subsystem. It carries four independent knobs across two concerns:
//!
//! - **format/lint** — `line-length` (default 100) controls formatter wrapping
//!   width; the `[lint]` table is per-rule enable/disable for the `lint` command.
//! - **strict severity** — `preset` selects a severity bundle (`strict`, the
//!   default) and the `[warnings]` table sets per-code overrides (`error` | `warn`
//!   | `off`) consumed by `check`'s strict pass.
//!
//! This module is the **single parse authority** for that file: it parses the
//! whole thing with the `toml` crate and hands the strict half to the
//! `gdstrict-strict` crate as a structured [`gdstrict_strict::SeverityConfig`]
//! (via [`gdstrict_strict::SeverityConfig::from_parts`]).
//!
//! Precedence, highest first:
//!   1. `--line-length <n>` — overrides line length for every file.
//!   2. `--config <file>` — use this exact `gdstrict.toml` for every file,
//!      skipping per-file discovery.
//!   3. The nearest `gdstrict.toml` found by walking UP the directory tree from
//!      each input file (black/ruff style); defaults apply when none is found.
//!
//! Example unified `gdstrict.toml`:
//!
//! ```toml
//! line-length = 100
//! preset = "strict"
//!
//! [lint]
//! function-name-case = false
//! constant-name-case = false
//! max-complexity = 15
//!
//! [warnings]
//! INTEGER_DIVISION = "off"
//! ```
//!
//! In the `[lint]` table, `false` disables a rule; `true` (or omitting the key)
//! keeps it enabled; an integer sets a threshold rule's limit (`max-complexity`,
//! `max-line-length`, `function-arguments-number`, `max-public-methods`) and
//! implies enabled. In `[warnings]`, each value is an action token. When no
//! `preset` key is given, severity defaults to the built-in `strict` preset — the
//! strictest-by-default position `check` already took.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gdstrict_strict::{Action, Preset, SeverityConfig};
use serde::Deserialize;

/// Default line width (Godot style guide / black default). Aliased to the
/// formatter engine's default so the two can never silently drift apart.
pub const DEFAULT_LINE_LENGTH: usize = gdstrict_format::DEFAULT_WIDTH;

const CONFIG_FILENAME: &str = "gdstrict.toml";

/// On-disk shape of a `gdstrict.toml`. Unknown top-level keys are rejected
/// (`deny_unknown_fields`) so a typo like `line_length` fails loudly rather than
/// silently doing nothing — strictest-by-default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(rename = "line-length")]
    line_length: Option<usize>,
    /// Per-rule settings: `false` disables a rule, `true` (or absent) enables it,
    /// and an integer sets a threshold rule's limit. See [`LintSetting`].
    #[serde(default)]
    lint: HashMap<String, LintSetting>,
    /// Strict severity preset name (only `strict` is known). Absent ⇒ `strict`.
    preset: Option<String>,
    /// Per-code strict severity overrides; each value is `error` | `warn` | `off`.
    #[serde(default)]
    warnings: HashMap<String, String>,
}

/// One value in the `[lint]` table. A rule is either switched on/off or — for the
/// threshold rules (`max-complexity`, `max-line-length`, …) — given its limit,
/// which implies enabled. Untagged, so both spellings live in one table:
///
/// ```toml
/// [lint]
/// max-complexity = 15        # threshold
/// function-name-case = false # disabled
/// ```
#[derive(Deserialize)]
#[serde(untagged)]
enum LintSetting {
    Enabled(bool),
    Limit(usize),
}

/// Effective per-file lint settings resolved from config.
#[derive(Clone, Default)]
pub struct LintConfig {
    disabled: HashSet<String>,
    limits: HashMap<String, usize>,
}

impl LintConfig {
    pub fn is_enabled(&self, rule_id: &str) -> bool {
        !self.disabled.contains(rule_id)
    }

    /// Threshold overrides keyed by rule id, in the shape
    /// [`gdstrict_lint::rules::default_rules_with_limits`] consumes.
    pub fn limits(&self) -> &HashMap<String, usize> {
        &self.limits
    }
}

/// Everything one `gdstrict.toml` resolves to, kept together so the three knobs
/// are discovered and cached as a single unit (no risk of one cache drifting out
/// of step with another).
#[derive(Clone)]
struct Resolved {
    line_length: usize,
    lint: LintConfig,
    severity: SeverityConfig,
}

impl Resolved {
    /// The values that apply when no `gdstrict.toml` is discovered. Severity
    /// defaults to the built-in `strict` preset (not `SeverityConfig::default`,
    /// which is all-`warn`) — `check` is strict-by-default.
    fn defaults() -> Self {
        Self {
            line_length: DEFAULT_LINE_LENGTH,
            lint: LintConfig::default(),
            severity: SeverityConfig::strict(),
        }
    }
}

/// Resolves the effective line length, lint config, and strict severity profile
/// for each file, applying the precedence described in the module docs. Discovery
/// results are memoized per directory so processing a large tree does not re-walk
/// for every file.
pub struct Resolver {
    cli_line_length: Option<usize>,
    forced: Option<Resolved>,
    cache: HashMap<PathBuf, Resolved>,
}

impl Resolver {
    /// Build a resolver from the CLI overrides. Parses `--config` eagerly so a
    /// bad config path/contents is reported once, up front, rather than per file.
    pub fn new(config: Option<&Path>, cli_line_length: Option<usize>) -> Result<Self, String> {
        if let Some(n) = cli_line_length {
            validate(n, "--line-length")?;
        }
        let forced = match config {
            Some(path) => Some(load(path)?),
            None => None,
        };
        Ok(Self {
            cli_line_length,
            forced,
            cache: HashMap::new(),
        })
    }

    /// The effective line length to format `file` at. The one accessor with an
    /// extra precedence tier: `--line-length` beats `--config` and discovery.
    pub fn line_length_for(&mut self, file: &Path) -> Result<usize, String> {
        if let Some(n) = self.cli_line_length {
            return Ok(n);
        }
        Ok(self.resolved_for(file)?.line_length)
    }

    /// The effective lint config for `file`.
    pub fn lint_config_for(&mut self, file: &Path) -> Result<LintConfig, String> {
        Ok(self.resolved_for(file)?.lint)
    }

    /// The effective strict severity profile for `file`. Unlike line length, the
    /// CLI has no per-run severity override flag, so precedence is just `--config`
    /// then discovery.
    pub fn severity_config_for(&mut self, file: &Path) -> Result<SeverityConfig, String> {
        Ok(self.resolved_for(file)?.severity)
    }

    /// The full resolved bundle for `file`: a `--config` file (if given) wins over
    /// per-file discovery. The shared `forced`-then-`discover` precedence lives
    /// here so the public accessors don't each re-implement it.
    fn resolved_for(&mut self, file: &Path) -> Result<Resolved, String> {
        if let Some(ref f) = self.forced {
            return Ok(f.clone());
        }
        self.discover_for(file)
    }

    /// Resolve and cache the [`Resolved`] bundle for `file`'s directory.
    fn discover_for(&mut self, file: &Path) -> Result<Resolved, String> {
        let start = file.parent().unwrap_or_else(|| Path::new("."));
        if let Some(hit) = self.cache.get(start) {
            return Ok(hit.clone());
        }
        let mut found = Resolved::defaults();
        let mut dir = Some(start);
        while let Some(d) = dir {
            let candidate = d.join(CONFIG_FILENAME);
            if candidate.is_file() {
                found = load(&candidate)?;
                break;
            }
            dir = d.parent();
        }
        self.cache.insert(start.to_path_buf(), found.clone());
        Ok(found)
    }
}

/// Read and parse a unified `gdstrict.toml`, returning its resolved values.
fn load(path: &Path) -> Result<Resolved, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("reading config {}: {e}", path.display()))?;
    let raw: RawConfig =
        toml::from_str(&text).map_err(|e| format!("parsing config {}: {e}", path.display()))?;
    let line_length = match raw.line_length {
        Some(n) => {
            validate(n, &format!("line-length in {}", path.display()))?;
            n
        }
        None => DEFAULT_LINE_LENGTH,
    };
    let mut disabled: HashSet<String> = HashSet::new();
    let mut limits: HashMap<String, usize> = HashMap::new();
    for (rule, setting) in raw.lint {
        match setting {
            LintSetting::Enabled(false) => {
                disabled.insert(rule);
            }
            LintSetting::Enabled(true) => {}
            LintSetting::Limit(n) => {
                validate(n, &format!("[lint] {rule} in {}", path.display()))?;
                limits.insert(rule, n);
            }
        }
    }
    let severity = severity_from(raw.preset, raw.warnings, path)?;
    Ok(Resolved {
        line_length,
        lint: LintConfig { disabled, limits },
        severity,
    })
}

/// Build the strict severity profile from the unified config's `preset` /
/// `[warnings]`. An absent `preset` defaults to `strict` (strictest-by-default);
/// unknown preset names or action tokens are hard errors, not silent skips.
fn severity_from(
    preset: Option<String>,
    warnings: HashMap<String, String>,
    path: &Path,
) -> Result<SeverityConfig, String> {
    let preset = match preset {
        Some(name) => Some(Preset::parse(&name).ok_or_else(|| {
            format!(
                "unknown preset `{name}` in {} (known: strict)",
                path.display()
            )
        })?),
        None => Some(Preset::Strict),
    };
    let mut overrides = HashMap::new();
    for (code, token) in warnings {
        let action = Action::parse(&token).ok_or_else(|| {
            format!(
                "unknown action `{token}` for `{code}` in {} (expected error|warn|off)",
                path.display()
            )
        })?;
        overrides.insert(code, action);
    }
    Ok(SeverityConfig::from_parts(preset, overrides))
}

/// A line length of 0 would force maximal wrapping on every construct; reject it
/// rather than silently producing pathological output.
fn validate(n: usize, source: &str) -> Result<(), String> {
    if n == 0 {
        return Err(format!("{source} must be >= 1"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fresh temp dir for a unit test.
    fn scratch(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("gdstrict-config-test-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn default_when_no_config() {
        let dir = scratch("default");
        let mut r = Resolver::new(None, None).unwrap();
        let file = dir.join("a.gd");
        assert_eq!(r.line_length_for(&file).unwrap(), DEFAULT_LINE_LENGTH);
    }

    #[test]
    fn discovers_nearest_walking_up() {
        let dir = scratch("discover");
        fs::write(dir.join("gdstrict.toml"), "line-length = 80\n").unwrap();
        let nested = dir.join("sub").join("deep");
        fs::create_dir_all(&nested).unwrap();

        let mut r = Resolver::new(None, None).unwrap();
        assert_eq!(r.line_length_for(&nested.join("x.gd")).unwrap(), 80);
    }

    #[test]
    fn nearest_config_wins_over_ancestor() {
        let dir = scratch("nearest");
        fs::write(dir.join("gdstrict.toml"), "line-length = 80\n").unwrap();
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("gdstrict.toml"), "line-length = 60\n").unwrap();

        let mut r = Resolver::new(None, None).unwrap();
        assert_eq!(r.line_length_for(&sub.join("x.gd")).unwrap(), 60);
        assert_eq!(r.line_length_for(&dir.join("y.gd")).unwrap(), 80);
    }

    #[test]
    fn cli_line_length_overrides_discovery() {
        let dir = scratch("cli-override");
        fs::write(dir.join("gdstrict.toml"), "line-length = 80\n").unwrap();
        let mut r = Resolver::new(None, Some(42)).unwrap();
        assert_eq!(r.line_length_for(&dir.join("x.gd")).unwrap(), 42);
    }

    #[test]
    fn config_flag_skips_discovery() {
        let dir = scratch("config-flag");
        fs::write(dir.join("gdstrict.toml"), "line-length = 80\n").unwrap();
        let explicit = dir.join("other.toml");
        fs::write(&explicit, "line-length = 55\n").unwrap();

        let mut r = Resolver::new(Some(&explicit), None).unwrap();
        assert_eq!(r.line_length_for(&dir.join("x.gd")).unwrap(), 55);
    }

    #[test]
    fn missing_key_defaults() {
        let dir = scratch("empty-config");
        fs::write(dir.join("gdstrict.toml"), "# nothing here\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert_eq!(
            r.line_length_for(&dir.join("x.gd")).unwrap(),
            DEFAULT_LINE_LENGTH
        );
    }

    #[test]
    fn malformed_config_is_an_error() {
        let dir = scratch("malformed");
        fs::write(dir.join("gdstrict.toml"), "line-length = \"oops\"\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert!(r.line_length_for(&dir.join("x.gd")).is_err());
    }

    #[test]
    fn zero_line_length_rejected() {
        assert!(Resolver::new(None, Some(0)).is_err());
        let dir = scratch("zero");
        fs::write(dir.join("gdstrict.toml"), "line-length = 0\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert!(r.line_length_for(&dir.join("x.gd")).is_err());
    }

    // ── lint config ──────────────────────────────────────────────────────────

    #[test]
    fn all_rules_enabled_by_default() {
        let dir = scratch("lint-default");
        let mut r = Resolver::new(None, None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert!(lc.is_enabled("function-name-case"));
        assert!(lc.is_enabled("constant-name-case"));
    }

    #[test]
    fn lint_section_disables_named_rules() {
        let dir = scratch("lint-disable");
        fs::write(
            dir.join("gdstrict.toml"),
            "[lint]\nfunction-name-case = false\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert!(!lc.is_enabled("function-name-case"));
        assert!(lc.is_enabled("constant-name-case"));
    }

    #[test]
    fn explicit_true_keeps_rule_enabled() {
        let dir = scratch("lint-explicit-true");
        fs::write(
            dir.join("gdstrict.toml"),
            "[lint]\nfunction-name-case = true\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert!(lc.is_enabled("function-name-case"));
    }

    #[test]
    fn forced_config_supplies_lint_config() {
        let dir = scratch("lint-forced");
        let cfg = dir.join("explicit.toml");
        fs::write(&cfg, "[lint]\nconstant-name-case = false\n").unwrap();
        let mut r = Resolver::new(Some(&cfg), None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert!(!lc.is_enabled("constant-name-case"));
        assert!(lc.is_enabled("function-name-case"));
    }

    #[test]
    fn lint_config_is_memoized_per_directory() {
        let dir = scratch("lint-memo");
        fs::write(
            dir.join("gdstrict.toml"),
            "[lint]\nsignal-name-case = false\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let lc1 = r.lint_config_for(&dir.join("a.gd")).unwrap();
        let lc2 = r.lint_config_for(&dir.join("b.gd")).unwrap();
        assert!(!lc1.is_enabled("signal-name-case"));
        assert!(!lc2.is_enabled("signal-name-case"));
    }

    // ── lint thresholds ───────────────────────────────────────────────────────

    #[test]
    fn integer_lint_value_sets_a_threshold() {
        let dir = scratch("lint-threshold");
        fs::write(dir.join("gdstrict.toml"), "[lint]\nmax-complexity = 15\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert_eq!(lc.limits().get("max-complexity").copied(), Some(15));
        // A threshold implies the rule stays enabled.
        assert!(lc.is_enabled("max-complexity"));
    }

    #[test]
    fn bools_and_thresholds_coexist_in_one_table() {
        let dir = scratch("lint-mixed");
        fs::write(
            dir.join("gdstrict.toml"),
            "[lint]\nmax-complexity = 15\nfunction-name-case = false\nmax-line-length = 120\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert_eq!(lc.limits().get("max-complexity").copied(), Some(15));
        assert_eq!(lc.limits().get("max-line-length").copied(), Some(120));
        assert!(!lc.is_enabled("function-name-case"));
    }

    #[test]
    fn disabled_rule_records_no_threshold() {
        let dir = scratch("lint-threshold-off");
        fs::write(
            dir.join("gdstrict.toml"),
            "[lint]\nmax-complexity = false\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert!(!lc.is_enabled("max-complexity"));
        assert!(lc.limits().is_empty());
    }

    #[test]
    fn zero_threshold_is_rejected() {
        // Same posture as `line-length = 0`: a limit of 0 is nonsense, so fail
        // loudly rather than flag every function in the project.
        let dir = scratch("lint-threshold-zero");
        fs::write(dir.join("gdstrict.toml"), "[lint]\nmax-complexity = 0\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert!(r.lint_config_for(&dir.join("a.gd")).is_err());
    }

    #[test]
    fn non_bool_non_integer_lint_value_is_rejected() {
        let dir = scratch("lint-bad-value");
        fs::write(
            dir.join("gdstrict.toml"),
            "[lint]\nmax-complexity = \"15\"\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert!(r.lint_config_for(&dir.join("a.gd")).is_err());
    }

    #[test]
    fn no_config_means_no_thresholds() {
        let dir = scratch("lint-no-thresholds");
        let mut r = Resolver::new(None, None).unwrap();
        let lc = r.lint_config_for(&dir.join("a.gd")).unwrap();
        assert!(lc.limits().is_empty());
    }

    // ── severity config ───────────────────────────────────────────────────────

    #[test]
    fn severity_defaults_to_strict_preset() {
        let dir = scratch("sev-default");
        let mut r = Resolver::new(None, None).unwrap();
        let sc = r.severity_config_for(&dir.join("a.gd")).unwrap();
        // No config discovered ⇒ built-in strict preset, not all-warn default.
        assert_eq!(sc.action_for("UNSAFE_CAST"), Action::Error);
        assert_eq!(sc.action_for("INTEGER_DIVISION"), Action::Warn);
    }

    #[test]
    fn severity_absent_preset_key_still_strict() {
        // A config that configures only formatting still leaves severity strict —
        // `check` is strict-by-default whether or not a config file exists.
        let dir = scratch("sev-format-only");
        fs::write(dir.join("gdstrict.toml"), "line-length = 80\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let sc = r.severity_config_for(&dir.join("a.gd")).unwrap();
        assert_eq!(sc.action_for("UNTYPED_DECLARATION"), Action::Error);
    }

    #[test]
    fn severity_warnings_override_beats_preset() {
        let dir = scratch("sev-override");
        fs::write(
            dir.join("gdstrict.toml"),
            "preset = \"strict\"\n\n[warnings]\nINTEGER_DIVISION = \"off\"\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let sc = r.severity_config_for(&dir.join("a.gd")).unwrap();
        assert_eq!(sc.action_for("INTEGER_DIVISION"), Action::Off); // override wins
        assert_eq!(sc.action_for("UNSAFE_CAST"), Action::Error); // still from preset
    }

    #[test]
    fn unified_file_satisfies_format_and_severity_together() {
        // One gdstrict.toml carrying all four knobs parses cleanly and resolves
        // each concern — the whole point of the unified schema.
        let dir = scratch("sev-unified");
        fs::write(
            dir.join("gdstrict.toml"),
            "line-length = 80\npreset = \"strict\"\n\n[lint]\nfunction-name-case = false\n\n[warnings]\nINTEGER_DIVISION = \"off\"\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let file = dir.join("a.gd");
        assert_eq!(r.line_length_for(&file).unwrap(), 80);
        assert!(!r
            .lint_config_for(&file)
            .unwrap()
            .is_enabled("function-name-case"));
        let sc = r.severity_config_for(&file).unwrap();
        assert_eq!(sc.action_for("INTEGER_DIVISION"), Action::Off);
        assert_eq!(sc.action_for("UNSAFE_CAST"), Action::Error);
    }

    #[test]
    fn forced_config_supplies_severity_config() {
        let dir = scratch("sev-forced");
        let cfg = dir.join("explicit.toml");
        fs::write(&cfg, "[warnings]\nUNSAFE_CAST = \"off\"\n").unwrap();
        let mut r = Resolver::new(Some(&cfg), None).unwrap();
        let sc = r.severity_config_for(&dir.join("a.gd")).unwrap();
        assert_eq!(sc.action_for("UNSAFE_CAST"), Action::Off); // override
        assert_eq!(sc.action_for("UNTYPED_DECLARATION"), Action::Error); // strict default
    }

    #[test]
    fn unknown_preset_is_an_error() {
        let dir = scratch("sev-bad-preset");
        fs::write(dir.join("gdstrict.toml"), "preset = \"pedantic\"\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert!(r.severity_config_for(&dir.join("a.gd")).is_err());
    }

    #[test]
    fn unknown_action_is_an_error() {
        let dir = scratch("sev-bad-action");
        fs::write(
            dir.join("gdstrict.toml"),
            "[warnings]\nINTEGER_DIVISION = \"fatal\"\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert!(r.severity_config_for(&dir.join("a.gd")).is_err());
    }

    #[test]
    fn unknown_toplevel_key_is_rejected() {
        // `deny_unknown_fields`: a typo'd key fails loudly instead of doing nothing.
        let dir = scratch("sev-typo-key");
        fs::write(dir.join("gdstrict.toml"), "line_length = 80\n").unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        assert!(r.line_length_for(&dir.join("a.gd")).is_err());
    }
}
