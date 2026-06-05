//! Unified-diff rendering for `--diff`, backed by the `similar` crate.

use similar::TextDiff;

/// Render a git-style unified diff between `original` and `formatted` for
/// `path`. Returns an empty string when the inputs are identical.
pub fn unified_diff(original: &str, formatted: &str, path: &str) -> String {
    if original == formatted {
        return String::new();
    }
    let diff = TextDiff::from_lines(original, formatted);
    diff.unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::unified_diff;

    #[test]
    fn identical_inputs_produce_no_diff() {
        assert_eq!(unified_diff("a\n", "a\n", "x.gd"), "");
    }

    #[test]
    fn change_produces_headers_and_hunk() {
        let d = unified_diff("a \n", "a\n", "x.gd");
        assert!(d.contains("--- a/x.gd"), "missing - header:\n{d}");
        assert!(d.contains("+++ b/x.gd"), "missing + header:\n{d}");
        assert!(d.contains("@@"), "missing hunk marker:\n{d}");
    }
}
