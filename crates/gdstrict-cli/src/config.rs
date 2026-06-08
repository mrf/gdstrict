//! Config discovery + resolution (black/ruff style).
//!
//! A `gdstrict.toml` carries two independent knobs:
//!
//! - `line-length` (default 100) — controls formatter wrapping width.
//! - `[lint]` table — per-rule enable/disable for the `lint` command.
//!
//! Precedence, highest first:
//!   1. `--line-length <n>` — overrides line length for every file.
//!   2. `--config <file>` — use this exact `gdstrict.toml` for every file,
//!      skipping per-file discovery.
//!   3. The nearest `gdstrict.toml` found by walking UP the directory tree from
//!      each input file (black/ruff style); defaults apply when none is found.
//!
//! Example `gdstrict.toml`:
//!
//! ```toml
//! line-length = 100
//!
//! [lint]
//! function-name-case = false
//! constant-name-case = false
//! ```
//!
//! In the `[lint]` table, `false` disables a rule; `true` (or omitting the key)
//! keeps it enabled.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
        let mut found_ll = DEFAULT_LINE_LENGTH;
        let mut found_lc = LintConfig::default();
        let mut dir = Some(start);
        while let Some(d) = dir {
            let candidate = d.join(CONFIG_FILENAME);
            if candidate.is_file() {
                let (ll, lc) = load(&candidate)?;
                found_ll = ll;
                found_lc = lc;
                break;
            }
            dir = d.parent();
        }
        self.cache.insert(start.to_path_buf(), found_ll);
        self.lint_cache.insert(start.to_path_buf(), found_lc.clone());
        Ok((found_ll, found_lc))
    }
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
}
