//! gdstrict-strict: drive Godot's headless analyzer and turn its stderr into
//! structured diagnostics.
//!
//! ## Strategy (Phase 0 spike .2 finding)
//!
//! Godot's `--check-only` surfaces *errors* but not *warnings*; adding `--debug`
//! surfaces warnings but drops into the debugger and **crashes (signal 11)** when a
//! hard parse error is present. So we run two passes:
//!
//! 1. **Errors** — `godot --headless --check-only --path P --script F` (no `--debug`).
//!    Emits `SCRIPT ERROR:` / `ERROR:` lines, exits 1. Never crashes.
//! 2. **Warnings** — only if pass 1 is clean: `... --check-only --debug ...`.
//!    Emits `WARNING:` lines. Safe because no hard error is present to trip the debugger.
//!
//! Output format (Godot 4.6.2), two lines per diagnostic:
//! ```text
//! WARNING: <message>
//!      at: GDScript::reload (res://path.gd:LINE)
//! SCRIPT ERROR: Parse Error: <message>
//!           at: GDScript::reload (res://path.gd:LINE)
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// res:// path reported by Godot.
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// Best-effort warning code (e.g. "UNSAFE_METHOD_ACCESS"); None for errors or
    /// messages we do not yet map.
    pub code: Option<&'static str>,
    pub message: String,
}

/// Locate a Godot binary: `$GODOT`, else `godot` on PATH.
pub fn find_godot() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GODOT") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    // Trust PATH resolution at spawn time.
    Some(PathBuf::from("godot"))
}

/// Run the full strict check on a single script within a project.
///
/// `project_dir` is the dir containing project.godot; `script_rel` is the script
/// path relative to it (as Godot expects after `--path`).
pub fn check_script(
    godot: &Path,
    project_dir: &Path,
    script_rel: &str,
) -> std::io::Result<Vec<Diagnostic>> {
    // Pass 1: errors (no --debug — safe).
    let errs = run(godot, project_dir, script_rel, false)?;
    let mut diags = parse_diagnostics(&errs);
    let has_error = diags.iter().any(|d| d.severity == Severity::Error);

    // Pass 2: warnings — only when there are no hard errors, to avoid the
    // --debug debugger crash.
    if !has_error {
        let warns = run(godot, project_dir, script_rel, true)?;
        diags.extend(parse_diagnostics(&warns));
    }
    Ok(diags)
}

fn run(godot: &Path, project_dir: &Path, script_rel: &str, debug: bool) -> std::io::Result<String> {
    let mut cmd = Command::new(godot);
    cmd.arg("--headless")
        .arg("--check-only")
        .arg("--path")
        .arg(project_dir)
        .arg("--script")
        .arg(script_rel);
    if debug {
        cmd.arg("--debug");
    }
    let out = cmd.output()?;
    // Diagnostics go to stderr.
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Parse Godot stderr into diagnostics. Pairs each `WARNING:` / `*ERROR:` header
/// line with the following `at: ... (res://file:line)` locator line.
pub fn parse_diagnostics(stderr: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = stderr.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start();
        let (severity, message) = if let Some(rest) = line.strip_prefix("WARNING: ") {
            (Severity::Warning, rest.to_string())
        } else if let Some(rest) = line.strip_prefix("SCRIPT ERROR: ") {
            (Severity::Error, rest.to_string())
        } else if let Some(rest) = line.strip_prefix("ERROR: ") {
            (Severity::Error, rest.to_string())
        } else {
            i += 1;
            continue;
        };
        // Look for the locator on the next non-empty line.
        let mut file = String::new();
        let mut ln = 0usize;
        if let Some(next) = lines.get(i + 1) {
            if let Some((f, l)) = parse_locator(next) {
                file = f;
                ln = l;
            }
        }
        out.push(Diagnostic {
            severity,
            code: if severity == Severity::Warning {
                classify_warning(&message)
            } else {
                None
            },
            file,
            line: ln,
            message,
        });
        i += 1;
    }
    out
}

/// Parse `   at: GDScript::reload (res://path.gd:42)` → ("res://path.gd", 42).
///
/// Handles three forms:
///   file:line          — last segment is the line
///   file:line:column   — second-to-last is line, last is column (column is discarded)
///   res://file:line    — the `://` in the scheme is not confused for a line separator
///
/// Strategy: split on `:`, walk from the right. If the last segment is numeric
/// AND the second-to-last is also numeric, treat them as line:column and take the
/// second-to-last as the line. Otherwise the last numeric segment is the line.
fn parse_locator(line: &str) -> Option<(String, usize)> {
    let line = line.trim_start();
    if !line.starts_with("at:") {
        return None;
    }
    let open = line.rfind('(')?;
    let close = line.rfind(')')?;
    let inner = &line[open + 1..close];

    let parts: Vec<&str> = inner.split(':').collect();
    let n = parts.len();
    if n < 2 {
        return None;
    }

    // Last segment must be a number (line or column).
    let last: usize = parts[n - 1].trim().parse().ok()?;

    // If the second-to-last is also a number, this is file:line:column.
    if n >= 3 {
        if let Ok(line_num) = parts[n - 2].trim().parse::<usize>() {
            let path = parts[..n - 2].join(":");
            return Some((path, line_num));
        }
    }

    // Simple file:line (path may itself contain ':' e.g. res://).
    let path = parts[..n - 1].join(":");
    Some((path, last))
}

/// Best-effort message → warning code. Templates are stable per Godot version;
/// version-gate this map when bumping the supported Godot release.
fn classify_warning(msg: &str) -> Option<&'static str> {
    let m = msg;
    if m.contains("has no static type") {
        Some("UNTYPED_DECLARATION")
    } else if m.contains("is not present on the inferred type") && m.contains("method") {
        Some("UNSAFE_METHOD_ACCESS")
    } else if m.contains("is not present on the inferred type") {
        Some("UNSAFE_PROPERTY_ACCESS")
    } else if m.starts_with("Casting") && m.contains("unsafe") {
        Some("UNSAFE_CAST")
    } else if m.contains("returns a value that will be discarded") {
        Some("RETURN_VALUE_DISCARDED")
    } else if m.starts_with("Integer division") {
        Some("INTEGER_DIVISION")
    } else if m.contains("inferred from a Variant value") {
        Some("INFERRED_DECLARATION")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_locator unit tests ---

    #[test]
    fn locator_plain_file_line() {
        assert_eq!(
            parse_locator("     at: GDScript::reload (path/to/file.gd:10)"),
            Some(("path/to/file.gd".into(), 10))
        );
    }

    #[test]
    fn locator_file_line_column() {
        // Column is discarded; line must be 42, not 5.
        assert_eq!(
            parse_locator("     at: GDScript::reload (path/to/file.gd:42:5)"),
            Some(("path/to/file.gd".into(), 42))
        );
    }

    #[test]
    fn locator_res_scheme_line() {
        // res:// contains '://' — path must not be split on the scheme colon.
        assert_eq!(
            parse_locator("     at: GDScript::reload (res://path.gd:7)"),
            Some(("res://path.gd".into(), 7))
        );
    }

    #[test]
    fn locator_res_scheme_line_column() {
        assert_eq!(
            parse_locator("     at: GDScript::reload (res://path.gd:42:5)"),
            Some(("res://path.gd".into(), 42))
        );
    }

    #[test]
    fn locator_no_line_returns_none() {
        assert_eq!(parse_locator("     at: GDScript::reload (res://path.gd)"), None);
    }

    #[test]
    fn locator_not_at_line_returns_none() {
        assert_eq!(parse_locator("WARNING: something happened"), None);
    }

    // --- parse_diagnostics integration tests ---

    #[test]
    fn parses_warning_block() {
        let stderr = "WARNING: Variable \"thing\" has no static type.\n     at: GDScript::reload (res://unsafe.gd:7)\n";
        let d = parse_diagnostics(stderr);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].file, "res://unsafe.gd");
        assert_eq!(d[0].line, 7);
        assert_eq!(d[0].code, Some("UNTYPED_DECLARATION"));
    }

    #[test]
    fn parses_error_block() {
        let stderr = "SCRIPT ERROR: Parse Error: Function \"x()\" not found in base self.\n          at: GDScript::reload (res://broken.gd:4)\n";
        let d = parse_diagnostics(stderr);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].line, 4);
    }

    /// Phase 0 spike .2 — live extraction against a real Godot binary.
    /// Skipped automatically when no Godot is available (e.g. CI without the engine).
    #[test]
    fn live_strict_extraction() {
        let Some(godot) = find_godot() else {
            eprintln!("no godot; skipping");
            return;
        };
        // Resolve godot or bail (PATH "godot" may not exist).
        let probe = Command::new(&godot).arg("--version").output();
        if probe.is_err() {
            eprintln!("godot not runnable; skipping");
            return;
        }
        let project = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/strict_project");
        let diags = check_script(&godot, Path::new(project), "unsafe.gd").unwrap();
        let codes: Vec<_> = diags.iter().filter_map(|d| d.code).collect();
        assert!(
            codes.contains(&"UNSAFE_METHOD_ACCESS"),
            "expected UNSAFE_METHOD_ACCESS, got {diags:#?}"
        );
        assert!(codes.contains(&"UNTYPED_DECLARATION"));
        assert!(codes.contains(&"INTEGER_DIVISION"));
    }
}
