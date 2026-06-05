//! Config discovery + line-length resolution (black/ruff style).
//!
//! The only configurable knob is `line-length` (default 100, per the Godot
//! style guide). It can come from three places, highest precedence first:
//!
//!   1. `--line-length <n>` — overrides everything.
//!   2. `--config <file>` — use this exact `gdstrict.toml` for every file,
//!      skipping per-file discovery.
//!   3. the nearest `gdstrict.toml` found by walking UP the directory tree from
//!      each input file (black/ruff style); the default when none is found.
//!
//! A `gdstrict.toml` is a tiny TOML file with a single recognized top-level key:
//!
//! ```toml
//! line-length = 100
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Default line width (Godot style guide / black default). Aliased to the
/// formatter engine's default so the two can never silently drift apart.
pub const DEFAULT_LINE_LENGTH: usize = gdstrict_format::DEFAULT_WIDTH;

/// File name discovered when walking up the tree.
const CONFIG_FILENAME: &str = "gdstrict.toml";

/// On-disk shape of a `gdstrict.toml`. Only `line-length` is recognized; unknown
/// keys are ignored so the format can grow without breaking older binaries.
#[derive(Deserialize)]
struct RawConfig {
    #[serde(rename = "line-length")]
    line_length: Option<usize>,
}

/// Resolves the effective line length for each file, applying the precedence
/// described in the module docs. Discovery results are memoized per directory so
/// formatting a large tree does not re-walk for every file.
pub struct Resolver {
    /// `--line-length`: when set, used verbatim for every file.
    cli_line_length: Option<usize>,
    /// `--config`: a single resolved length used for every file (no discovery).
    forced_line_length: Option<usize>,
    /// Memoized discovery results keyed by the directory we searched from.
    cache: HashMap<PathBuf, usize>,
}

impl Resolver {
    /// Build a resolver from the CLI overrides. Parses `--config` eagerly so a
    /// bad config path/contents is reported once, up front, rather than per file.
    pub fn new(config: Option<&Path>, cli_line_length: Option<usize>) -> Result<Self, String> {
        if let Some(n) = cli_line_length {
            validate(n, "--line-length")?;
        }
        let forced_line_length = match config {
            Some(path) => Some(load(path)?),
            None => None,
        };
        Ok(Self {
            cli_line_length,
            forced_line_length,
            cache: HashMap::new(),
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
        self.discover(start)
    }

    /// Walk up from `start` to the nearest `gdstrict.toml`; default if none.
    fn discover(&mut self, start: &Path) -> Result<usize, String> {
        if let Some(&n) = self.cache.get(start) {
            return Ok(n);
        }
        let mut found = DEFAULT_LINE_LENGTH;
        let mut dir = Some(start);
        while let Some(d) = dir {
            let candidate = d.join(CONFIG_FILENAME);
            if candidate.is_file() {
                found = load(&candidate)?;
                break;
            }
            dir = d.parent();
        }
        self.cache.insert(start.to_path_buf(), found);
        Ok(found)
    }
}

/// Read and parse a `gdstrict.toml`, returning its line-length (the default when
/// the key is absent). Errors carry the path so the user can find the bad file.
fn load(path: &Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("reading config {}: {e}", path.display()))?;
    let raw: RawConfig =
        toml::from_str(&text).map_err(|e| format!("parsing config {}: {e}", path.display()))?;
    match raw.line_length {
        Some(n) => {
            validate(n, &format!("line-length in {}", path.display()))?;
            Ok(n)
        }
        None => Ok(DEFAULT_LINE_LENGTH),
    }
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

    /// A fresh temp dir for a unit test. Unit tests have no `CARGO_TARGET_TMPDIR`,
    /// so we namespace under the system temp dir by crate + tag.
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
        // A discoverable config that must be ignored in favor of --config.
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
        assert_eq!(r.line_length_for(&dir.join("x.gd")).unwrap(), DEFAULT_LINE_LENGTH);
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
}
