//! The formatting seam.
//!
//! The CLI delegates all source-level formatting to this single function so the
//! rest of the binary (path walking, config resolution, `--check`, `--diff`,
//! exit codes) is fully decoupled from the formatter engine.
//!
//! This now wires through to `gdstrict_format::format_width`, which lands the
//! CST -> Doc lowering from the formatter crate. The resolved line length (from
//! `gdstrict.toml` / `--line-length`) is threaded in here as the target width.
//!
//! Commands go through [`format_converged`], which proves the output is a fixed
//! point before anyone acts on it. A formatter bug that never converges (the
//! comment-in-literal comma, ks-xa1kd) otherwise surfaces as a permanent "would
//! reformat" that `gdstrict format` can never clear.

/// Format a single GDScript source string at the given line width.
///
/// Idempotent: `format_source(format_source(s, w), w) == format_source(s, w)`.
pub fn format_source(src: &str, line_length: usize) -> String {
    gdstrict_format::format_width(src, line_length)
}

/// The formatter failed to converge: a second pass over its own output changed
/// it again. This is an internal formatter bug, never a property of the input,
/// so callers report it as an error rather than a plain "would reformat".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonConvergent {
    /// Output of the first pass.
    pub once: String,
    /// Output of the second pass (differs from `once`).
    pub twice: String,
}

impl std::fmt::Display for NonConvergent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "internal formatter error: formatting is not idempotent (a second pass \
             changed the output again); the file was not modified. Please report \
             this as a gdstrict bug with the file contents."
        )
    }
}

impl std::error::Error for NonConvergent {}

/// Format and verify convergence: when the first pass changed the source, run a
/// second pass and require it to be a no-op. See [`converge`].
pub fn format_converged(src: &str, line_length: usize) -> Result<String, NonConvergent> {
    converge(src, |s| format_source(s, line_length))
}

/// Run `fmt` over `src` and prove the result is a fixed point.
///
/// If the first pass returns `src` unchanged, it is trivially a fixed point (the
/// formatter is a pure function, so `fmt(once) == fmt(src) == once`) and no second
/// pass is spent — clean files cost one format, not two. Otherwise the output is
/// formatted once more and must come back byte-identical.
fn converge(src: &str, fmt: impl Fn(&str) -> String) -> Result<String, NonConvergent> {
    let once = fmt(src);
    if once == src {
        return Ok(once);
    }
    let twice = fmt(&once);
    if twice == once {
        Ok(once)
    } else {
        Err(NonConvergent { once, twice })
    }
}

#[cfg(test)]
mod tests {
    use super::{converge, format_converged, format_source};

    const W: usize = gdstrict_format::DEFAULT_WIDTH;

    #[test]
    fn converge_accepts_a_fixed_point() {
        let out = converge("a  ", |s| s.trim_end().to_string()).unwrap();
        assert_eq!(out, "a");
        // Already-canonical input is accepted without a second pass changing it.
        assert_eq!(converge("a", |s| s.trim_end().to_string()).unwrap(), "a");
    }

    #[test]
    fn converge_rejects_a_formatter_that_keeps_changing() {
        // A "formatter" that appends a comma on every pass — the exact shape of the
        // comment-in-literal bug (`# c` -> `# c,` -> `# c,,`).
        let err = converge("# c", |s| format!("{s},")).unwrap_err();
        assert_eq!(err.once, "# c,");
        assert_eq!(err.twice, "# c,,");
        assert!(
            err.to_string().contains("not idempotent"),
            "message must name the failure: {err}"
        );
    }

    #[test]
    fn format_converged_matches_format_source_on_real_input() {
        let messy = "extends Node   \n\n\nfunc f():\n\tpass\n\n\n";
        assert_eq!(format_converged(messy, W).unwrap(), format_source(messy, W));
    }

    #[test]
    fn strips_trailing_whitespace() {
        assert_eq!(format_source("extends Node   \n", W), "extends Node\n");
    }

    #[test]
    fn collapses_trailing_blank_lines() {
        assert_eq!(format_source("extends Node\n\n\n", W), "extends Node\n");
    }

    #[test]
    fn idempotent() {
        let messy = "extends Node   \n\n\nfunc f():\n\tpass\n\n\n";
        let once = format_source(messy, W);
        assert_eq!(format_source(&once, W), once);
    }

    #[test]
    fn line_length_controls_wrapping() {
        let src = "var items = [aaaaaa, bbbbbb, cccccc, dddddd, eeeeee, ffffff]\n";
        // Wide: stays on one line.
        assert_eq!(format_source(src, 100).lines().count(), 1);
        // Narrow: wraps one element per line (magic-trailing-comma expansion).
        assert!(format_source(src, 20).lines().count() > 1);
    }
}
