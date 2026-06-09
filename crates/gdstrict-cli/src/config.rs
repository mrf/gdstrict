//! Config discovery + resolution (black/ruff style).
//!
//! ## The unified `gdstrict.toml` schema
//!
//! A single `gdstrict.toml` carries the settings of **two independent
//! subsystems** that historically had their own parsers:
//!
//! - **format / lint** (this module) — `line-length` and the `[lint]` table.
//! - **strict severity** (`gdstrict-strict`'s [`gdstrict_strict::parse_config`])
//!   — `preset` and the `[warnings]` table.
//!
//! ```toml
//! line-length = 100          # formatter wrapping width (this module)
//!
//! [lint]                     # per-rule enable/disable for `lint` (this module)
//! function-name-case = false
//!
//! preset = "strict"          # strict severity preset (gdstrict-strict)
//!
//! [warnings]                 # per-code severity overrides (gdstrict-strict)
//! INTEGER_DIVISION = "off"
//! ```
//!
//! The conflict this resolves: each parser used to **reject** the other's keys,
//! so one file could not satisfy both. The fix makes each parser *tolerate* (and
//! ignore) the other side's keys while still rejecting genuinely unknown ones:
//!
//! - This (format-side) parser uses serde without `deny_unknown_fields`, so it
//!   silently ignores `preset` / `[warnings]`.
//! - The strict parser explicitly ignores `line-length` / `[lint]` (see its
//!   module docs) but still flags typos.
//!
//! So `line-length` and `preset` are the two ends; one unified file feeds both.
//!
//! ## Discovery precedence (both subsystems), highest first
//!   1. `--line-length <n>` — overrides line length for every file (format only).
//!   2. `--config <file>` — use this exact `gdstrict.toml` for every file,
//!      skipping per-file discovery (feeds *both* the format Resolver and the
//!      [`SeverityResolver`]).
//!   3. The nearest `gdstrict.toml` found by walking UP the directory tree from
//!      each input file (black/ruff style); defaults apply when none is found.
//!
//! In the `[lint]` table, `false` disables a rule; `true` (or omitting the key)
//! keeps it enabled.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gdstrict_strict::{Preset, SeverityConfig};
use serde::Deserialize;

/// Default line width (Godot style guide / black default). Aliased to the
/// formatter engine's default so the two can never silently drift apart.
pub const DEFAULT_LINE_LENGTH: usize = gdstrict_format::DEFAULT_WIDTH;

const CONFIG_FILENAME: &str = "gdstrict.toml";

/// On-disk shape of a `gdstrict.toml`.
#[derive(Deserialize)]
struct RawConfig {
    #[serde(rename = "line-length")]
    line_length: Option<usize>,
    /// Per-rule enable/disable. `false` disables a rule; `true` or absent means enabled.
    #[serde(default)]
    lint: HashMap<String, bool>,
}

/// Effective per-file lint settings resolved from config.
#[derive(Clone, Default)]
pub struct LintConfig {
    disabled: HashSet<String>,
}

impl LintConfig {
    pub fn is_enabled(&self, rule_id: &str) -> bool {
        !self.disabled.contains(rule_id)
    }
}

/// Resolves the effective line length and lint config for each file, applying
/// the precedence described in the module docs. Discovery results are memoized
/// per directory so processing a large tree does not re-walk for every file.
pub struct Resolver {
    cli_line_length: Option<usize>,
    forced_line_length: Option<usize>,
    forced_lint: Option<LintConfig>,
    cache: HashMap<PathBuf, usize>,
    lint_cache: HashMap<PathBuf, LintConfig>,
}

impl Resolver {
    /// Build a resolver from the CLI overrides. Parses `--config` eagerly so a
    /// bad config path/contents is reported once, up front, rather than per file.
    pub fn new(config: Option<&Path>, cli_line_length: Option<usize>) -> Result<Self, String> {
        if let Some(n) = cli_line_length {
            validate(n, "--line-length")?;
        }
        let (forced_line_length, forced_lint) = match config {
            Some(path) => {
                let (ll, lc) = load(path)?;
                (Some(ll), Some(lc))
            }
            None => (None, None),
        };
        Ok(Self {
            cli_line_length,
            forced_line_length,
            forced_lint,
            cache: HashMap::new(),
            lint_cache: HashMap::new(),
        })
    }

    /// The effective line length to format `file` at.
    pub fn line_length_for(&mut self, file: &Path) -> Result<usize, String> {
        if let Some(n) = self.cli_line_length {
            return Ok(n);
        }
        if let Some(n) = self.forced_line_length {
            return Ok(n);
        }
        let start = file.parent().unwrap_or_else(|| Path::new("."));
        Ok(self.discover(start)?.0)
    }

    /// The effective lint config for `file`.
    pub fn lint_config_for(&mut self, file: &Path) -> Result<LintConfig, String> {
        if let Some(ref lc) = self.forced_lint {
            return Ok(lc.clone());
        }
        let start = file.parent().unwrap_or_else(|| Path::new("."));
        Ok(self.discover(start)?.1)
    }

    /// Walk up from `start` to the nearest `gdstrict.toml`; return defaults if none.
    fn discover(&mut self, start: &Path) -> Result<(usize, LintConfig), String> {
        // Both caches are populated together — if one entry exists, both do.
        if let Some(&ll) = self.cache.get(start) {
            let lc = self.lint_cache.get(start).cloned().unwrap_or_default();
            return Ok((ll, lc));
        }
        let (found_ll, found_lc) = match find_config_file(start) {
            Some(path) => load(&path)?,
            None => (DEFAULT_LINE_LENGTH, LintConfig::default()),
        };
        self.cache.insert(start.to_path_buf(), found_ll);
        self.lint_cache.insert(start.to_path_buf(), found_lc.clone());
        Ok((found_ll, found_lc))
    }
}

/// Walk UP from `start` to the nearest `gdstrict.toml` (black/ruff style); the
/// shared discovery seam for both the format [`Resolver`] and the
/// [`SeverityResolver`] so the two never disagree on which file wins.
fn find_config_file(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(CONFIG_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Resolves the strict **severity profile** for each file, mirroring [`Resolver`]'s
/// discovery so `check` honors per-project `gdstrict.toml` overrides (e.g.
/// `INTEGER_DIVISION = "off"`) instead of always using the built-in preset.
///
/// Precedence, highest first:
///   1. `--config <file>` — that exact file's profile for every input file.
///   2. The nearest discovered `gdstrict.toml` (same walk as [`Resolver`]).
///   3. No config found → the built-in [`SeverityConfig::strict`] preset.
///
/// Strict-by-default: a discovered config that omits `preset` still gets the
/// `strict` preset as its baseline (via [`SeverityConfig::with_default_preset`]),
/// so a file that only tweaks formatting never silently disables strict typing.
///
/// Kept separate from [`Resolver`] (rather than folded into one `discover` that
/// reads the file once) on purpose: only `check` resolves severity, and only the
/// strict parser rejects a malformed `preset`/`[warnings]`. Merging would make
/// `format`/`lint` parse — and start failing on — the strict side of the file,
/// regressing their behavior. The cost is one extra walk + read of the same tiny
/// `gdstrict.toml` per cold directory, memoized thereafter and dwarfed by `check`'s
/// per-file Godot subprocess; revisit (share a path memo) only if a third consumer
/// appears.
pub struct SeverityResolver {
    forced: Option<SeverityConfig>,
    cache: HashMap<PathBuf, SeverityConfig>,
}

impl SeverityResolver {
    /// Build from the CLI `--config` override. Parses `--config` eagerly so a bad
    /// path/contents is reported once, up front, rather than per file.
    pub fn new(config: Option<&Path>) -> Result<Self, String> {
        let forced = match config {
            Some(path) => Some(load_severity(path)?),
            None => None,
        };
        Ok(Self {
            forced,
            cache: HashMap::new(),
        })
    }

    /// The effective severity profile for `file`.
    pub fn severity_for(&mut self, file: &Path) -> Result<SeverityConfig, String> {
        if let Some(ref cfg) = self.forced {
            return Ok(cfg.clone());
        }
        let start = file.parent().unwrap_or_else(|| Path::new("."));
        if let Some(cfg) = self.cache.get(start) {
            return Ok(cfg.clone());
        }
        let cfg = match find_config_file(start) {
            Some(path) => load_severity(&path)?,
            None => SeverityConfig::strict(),
        };
        self.cache.insert(start.to_path_buf(), cfg.clone());
        Ok(cfg)
    }
}

/// Read and parse a `gdstrict.toml`'s strict severity profile, defaulting the
/// preset to `strict` when the file omits one (strict-by-default).
fn load_severity(path: &Path) -> Result<SeverityConfig, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("reading config {}: {e}", path.display()))?;
    let cfg = gdstrict_strict::parse_config(&text)
        .map_err(|e| format!("parsing config {}: {e}", path.display()))?;
    Ok(cfg.with_default_preset(Preset::Strict))
}

/// Read and parse a `gdstrict.toml`, returning its resolved values.
fn load(path: &Path) -> Result<(usize, LintConfig), String> {
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
    let disabled: HashSet<String> = raw
        .lint
        .into_iter()
        .filter_map(|(k, enabled)| if !enabled { Some(k) } else { None })
        .collect();
    Ok((line_length, LintConfig { disabled }))
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

    // ── unified schema: the format side ignores the strict side's keys ─────────

    /// A single gdstrict.toml carrying *both* parsers' keys must parse on the
    /// format side, which reads `line-length` + `[lint]` and ignores the rest.
    #[test]
    fn unified_file_parses_on_format_side() {
        let dir = scratch("unified-format");
        fs::write(
            dir.join("gdstrict.toml"),
            "line-length = 70\npreset = \"strict\"\n\n[lint]\nfunction-name-case = false\n\n[warnings]\nINTEGER_DIVISION = \"off\"\n",
        )
        .unwrap();
        let mut r = Resolver::new(None, None).unwrap();
        let file = dir.join("a.gd");
        assert_eq!(r.line_length_for(&file).unwrap(), 70);
        let lc = r.lint_config_for(&file).unwrap();
        assert!(!lc.is_enabled("function-name-case"));
        assert!(lc.is_enabled("constant-name-case"));
    }

    // ── strict severity resolution ─────────────────────────────────────────────

    #[test]
    fn severity_defaults_to_strict_when_no_config() {
        let dir = scratch("sev-default");
        let mut sr = SeverityResolver::new(None).unwrap();
        let cfg = sr.severity_for(&dir.join("a.gd")).unwrap();
        assert_eq!(
            cfg.action_for("UNTYPED_DECLARATION"),
            gdstrict_strict::Action::Error
        );
    }

    /// The whole point of the wiring: a discovered profile's per-code override is
    /// honored (here `INTEGER_DIVISION = "off"`) while the strict preset still
    /// promotes the typing family.
    #[test]
    fn severity_honors_discovered_override() {
        let dir = scratch("sev-override");
        fs::write(
            dir.join("gdstrict.toml"),
            "line-length = 80\npreset = \"strict\"\n\n[warnings]\nINTEGER_DIVISION = \"off\"\n",
        )
        .unwrap();
        let nested = dir.join("sub");
        fs::create_dir_all(&nested).unwrap();
        let mut sr = SeverityResolver::new(None).unwrap();
        let cfg = sr.severity_for(&nested.join("a.gd")).unwrap();
        assert_eq!(
            cfg.action_for("INTEGER_DIVISION"),
            gdstrict_strict::Action::Off
        );
        assert_eq!(
            cfg.action_for("UNSAFE_CAST"),
            gdstrict_strict::Action::Error
        );
    }

    /// A config that omits `preset` still enforces strict-by-default.
    #[test]
    fn severity_keeps_strict_when_preset_omitted() {
        let dir = scratch("sev-no-preset");
        fs::write(
            dir.join("gdstrict.toml"),
            "line-length = 80\n\n[warnings]\nINTEGER_DIVISION = \"off\"\n",
        )
        .unwrap();
        let mut sr = SeverityResolver::new(None).unwrap();
        let cfg = sr.severity_for(&dir.join("a.gd")).unwrap();
        assert_eq!(
            cfg.action_for("UNTYPED_DECLARATION"),
            gdstrict_strict::Action::Error
        );
        assert_eq!(
            cfg.action_for("INTEGER_DIVISION"),
            gdstrict_strict::Action::Off
        );
    }

    #[test]
    fn severity_config_flag_forces_profile() {
        let dir = scratch("sev-forced");
        let explicit = dir.join("explicit.toml");
        fs::write(&explicit, "preset = \"strict\"\n[warnings]\nRETURN_VALUE_DISCARDED = \"off\"\n")
            .unwrap();
        let mut sr = SeverityResolver::new(Some(&explicit)).unwrap();
        let cfg = sr.severity_for(&dir.join("a.gd")).unwrap();
        assert_eq!(
            cfg.action_for("RETURN_VALUE_DISCARDED"),
            gdstrict_strict::Action::Off
        );
    }

    #[test]
    fn severity_bad_config_is_an_error() {
        let dir = scratch("sev-bad");
        let explicit = dir.join("bad.toml");
        fs::write(&explicit, "preset = \"bogus\"\n").unwrap();
        assert!(SeverityResolver::new(Some(&explicit)).is_err());
    }
}
