//! The formatting seam.
//!
//! The CLI delegates all source-level formatting to this single function so the
//! rest of the binary (path walking, config resolution, `--check`, `--diff`,
//! exit codes) is fully decoupled from the formatter engine.
//!
//! This now wires through to `gdstrict_format::format_width`, which lands the
//! CST -> Doc lowering from the formatter crate. The resolved line length (from
//! `gdstrict.toml` / `--line-length`) is threaded in here as the target width.

/// Format a single GDScript source string at the given line width.
///
/// Idempotent: `format_source(format_source(s, w), w) == format_source(s, w)`.
pub fn format_source(src: &str, line_length: usize) -> String {
    gdstrict_format::format_width(src, line_length)
}

#[cfg(test)]
mod tests {
    use super::format_source;

    const W: usize = gdstrict_format::DEFAULT_WIDTH;

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
