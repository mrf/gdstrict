//! The formatting seam.
//!
//! The CLI delegates all source-level formatting to this single function so the
//! rest of the binary (path walking, `--check`, `--diff`, exit codes) is fully
//! decoupled from the formatter engine.
//!
//! TODO(godot-linter-phase1-formatter-i7d.5): wire this to
//! `gdstrict_format::format(src)` once the CST -> Doc lowering lands in
//! gdstrict-format. That crate currently exposes only the document IR + renderer
//! (`gdstrict_format::doc`), not a source-level `format(&str) -> String`. Until
//! then we apply a minimal, idempotent normalization so `--check`/`--diff` carry
//! real semantics:
//!   * normalize CRLF / lone-CR line endings to LF
//!   * strip trailing whitespace from each line
//!   * end the file with exactly one trailing newline (empty files stay empty)
//!
//! Every one of these is a transform the real formatter must also perform, so
//! swapping in the engine later only tightens output — it never loosens these
//! guarantees, and existing `--check` expectations stay valid.

/// Format a single GDScript source string. Idempotent: `format_source(format_source(s)) == format_source(s)`.
pub fn format_source(src: &str) -> String {
    // Normalize line endings first so trailing-whitespace stripping is uniform.
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");

    let mut out = String::with_capacity(normalized.len());
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }

    // Collapse trailing blank lines to a single terminating newline. Keep a
    // wholly-empty (or whitespace-only) file empty rather than emitting "\n".
    let body = out.trim_end_matches('\n');
    if body.is_empty() {
        String::new()
    } else {
        let mut s = String::with_capacity(body.len() + 1);
        s.push_str(body);
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::format_source;

    #[test]
    fn strips_trailing_whitespace() {
        assert_eq!(format_source("extends Node   \n"), "extends Node\n");
    }

    #[test]
    fn normalizes_crlf() {
        assert_eq!(format_source("a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn ensures_single_trailing_newline() {
        assert_eq!(format_source("a\nb"), "a\nb\n");
        assert_eq!(format_source("a\nb\n\n\n"), "a\nb\n");
    }

    #[test]
    fn empty_stays_empty() {
        assert_eq!(format_source(""), "");
        assert_eq!(format_source("   \n\n"), "");
    }

    #[test]
    fn idempotent() {
        let messy = "extends Node \r\n\tfunc f():   \n\t\tpass\n\n\n";
        let once = format_source(messy);
        assert_eq!(format_source(&once), once);
    }
}
