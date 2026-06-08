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
//!
//! ## Injected warning settings (do not trust project.godot)
//!
//! Godot only emits the unsafe/untyped warning family when it is enabled under
//! `[debug] gdscript/warnings/*` in `project.godot` — and that family is off by
//! default. A target project may have it disabled (or actively set to `0`), which
//! would silently suppress the very diagnostics gdstrict exists to enforce.
//!
//! So gdstrict does not trust the target's config: before each check it writes a
//! Godot `override.cfg` at the project root forcing its required warning set on.
//! Godot reads `override.cfg` on top of `project.godot`, so this wins regardless of
//! the project's settings without mutating `project.godot` itself. The file is
//! restored (or removed) on drop, including on early return. See [`StrictWarnings`].

use std::path::{Path, PathBuf};
use std::process::Command;

mod config;
pub use config::{parse as parse_config, Action, ConfigError, Preset, SeverityConfig};

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

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Locate a Godot binary: `$GODOT` env var (must be executable), else `godot` on PATH.
/// Returns `None` when neither is found, so callers can skip gracefully.
pub fn find_godot() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GODOT") {
        let pb = PathBuf::from(p);
        if is_executable(&pb) {
            return Some(pb);
        }
    }
    // Search PATH explicitly so None is a real signal, not a deferred spawn failure.
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    find_godot_in_dirs(std::env::split_paths(&path_var))
}

/// Search `dirs` for a `godot` executable; returns the first match or `None`.
/// Extracted from [`find_godot`] so tests can inject a controlled directory list
/// without touching the process environment.
fn find_godot_in_dirs(dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    for dir in dirs {
        let candidate = dir.join("godot");
        if is_executable(&candidate) {
            return Some(candidate);
        }
        // Windows executables have extensions; try common ones.
        #[cfg(windows)]
        for ext in &["exe", "bat", "cmd"] {
            let c = dir.join(format!("godot.{ext}"));
            if is_executable(&c) {
                return Some(c);
            }
        }
    }
    None
}

/// GDScript warning keys gdstrict forces on (value `1` = "Warn") regardless of the
/// target project's config, so the analyzer always emits the unsafe/untyped family.
///
/// gdstrict applies its own severity mapping downstream, so we inject these as plain
/// warnings (`1`) and decide error-vs-warn ourselves rather than letting Godot fail
/// the parse via `2` ("Error"). Version-gate this set alongside [`classify_warning`].
const STRICT_WARNINGS: &[&str] = &[
    "untyped_declaration",
    "inferred_declaration",
    "unsafe_property_access",
    "unsafe_method_access",
    "unsafe_cast",
    "unsafe_call_argument",
    "return_value_discarded",
    "integer_division",
    "unused_variable",
    "shadowed_variable",
];

/// Render the `override.cfg` contents that force [`STRICT_WARNINGS`] on.
fn strict_override_cfg() -> String {
    let mut s = String::from(
        "; Written by gdstrict: strict warning settings injected so the GDScript\n\
         ; analyzer emits the unsafe/untyped family regardless of the target\n\
         ; project's [debug] config. Auto-removed after the check.\n\
         [debug]\n\n\
         gdscript/warnings/enable=true\n",
    );
    for w in STRICT_WARNINGS {
        s.push_str("gdscript/warnings/");
        s.push_str(w);
        s.push_str("=1\n");
    }
    s
}

/// RAII guard that installs gdstrict's [`STRICT_WARNINGS`] as a Godot `override.cfg`
/// at the project root for the duration of a check, then restores the prior state on
/// drop. Because Godot reads `override.cfg` on top of `project.godot`, this forces the
/// warning set on without trusting — or mutating — the target's config.
///
/// If an `override.cfg` already exists it is backed up in memory and rewritten on
/// drop; otherwise the file we created is removed. Drop runs on early return too, so
/// the project is left as we found it even when a Godot pass errors.
///
/// Concurrency caveat: the override lives at the shared project root, so two checks of
/// the same project at once race on the file. The current API is one script per call;
/// the Phase 2 concurrent worker pool must serialize the install or isolate per check.
struct StrictWarnings {
    path: PathBuf,
    /// Prior `override.cfg` bytes to restore, or `None` if we created it fresh.
    prior: Option<Vec<u8>>,
}

impl StrictWarnings {
    /// Write the strict `override.cfg` into `project_dir`, capturing any prior file.
    fn install(project_dir: &Path) -> std::io::Result<Self> {
        let path = project_dir.join("override.cfg");
        let prior = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        std::fs::write(&path, strict_override_cfg())?;
        Ok(Self { path, prior })
    }
}

impl Drop for StrictWarnings {
    fn drop(&mut self) {
        // Best-effort restore; failures here must not mask the check's result.
        match &self.prior {
            Some(bytes) => {
                let _ = std::fs::write(&self.path, bytes);
            }
            None => {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

/// Run the full strict check on a single script within a project.
///
/// `project_dir` is the dir containing project.godot; `script_rel` is the script
/// path relative to it (as Godot expects after `--path`).
///
/// gdstrict's required warning set is injected via an `override.cfg` for the duration
/// of the check (see [`StrictWarnings`]), so unsafe/untyped warnings are emitted
/// regardless of what the target `project.godot` enables or disables.
pub fn check_script(
    godot: &Path,
    project_dir: &Path,
    script_rel: &str,
) -> std::io::Result<Vec<Diagnostic>> {
    // Force our warning settings on for both passes; restored when `_warnings` drops
    // (including on the `?` early returns below).
    let _warnings = StrictWarnings::install(project_dir)?;

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
        assert_eq!(
            parse_locator("     at: GDScript::reload (res://path.gd)"),
            None
        );
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

    #[test]
    fn override_cfg_lists_strict_warnings_as_warn() {
        let cfg = strict_override_cfg();
        assert!(cfg.contains("[debug]"));
        assert!(cfg.contains("gdscript/warnings/enable=true"));
        // Every strict key is forced to 1 ("Warn"), never 0/2.
        for w in STRICT_WARNINGS {
            assert!(
                cfg.contains(&format!("gdscript/warnings/{w}=1")),
                "missing strict warning {w} in:\n{cfg}"
            );
        }
        assert!(!cfg.contains("=0"), "no strict warning should be disabled");
    }

    /// The guard creates override.cfg when absent and removes it on drop, leaving
    /// the project exactly as it was. No Godot needed.
    #[test]
    fn guard_creates_and_removes_override() {
        let dir = scratch_dir("create");
        let override_path = dir.join("override.cfg");
        assert!(!override_path.exists());
        {
            let _g = StrictWarnings::install(&dir).unwrap();
            let written = std::fs::read_to_string(&override_path).unwrap();
            assert!(written.contains("gdscript/warnings/unsafe_method_access=1"));
        }
        assert!(
            !override_path.exists(),
            "override.cfg must be removed on drop"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The guard restores a pre-existing override.cfg byte-for-byte on drop.
    #[test]
    fn guard_restores_existing_override() {
        let dir = scratch_dir("restore");
        let override_path = dir.join("override.cfg");
        let original = b"[display]\nwindow/size/mode=3\n";
        std::fs::write(&override_path, original).unwrap();
        {
            let _g = StrictWarnings::install(&dir).unwrap();
            // While installed, our settings are in place, not the user's.
            let live = std::fs::read_to_string(&override_path).unwrap();
            assert!(live.contains("gdscript/warnings/untyped_declaration=1"));
        }
        let restored = std::fs::read(&override_path).unwrap();
        assert_eq!(restored, original, "prior override.cfg must be restored");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Drift guard: the hostile inject fixture must explicitly set *every*
    /// [`STRICT_WARNINGS`] key to `0`, so the live injection test genuinely proves we
    /// override a project that disables the full set (not just Godot defaults). Fails
    /// if the const and the fixture drift apart. No Godot needed.
    #[test]
    fn inject_fixture_disables_every_strict_warning() {
        let project_godot = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/strict_inject_project/project.godot"
        );
        let text = std::fs::read_to_string(project_godot).unwrap();
        for w in STRICT_WARNINGS {
            assert!(
                text.contains(&format!("gdscript/warnings/{w}=0")),
                "inject fixture must disable strict warning `{w}` (set it to 0); \
                 STRICT_WARNINGS and the fixture have drifted"
            );
        }
    }

    // --- find_godot_in_dirs unit tests (no env-var mutation, thread-safe) ---

    #[test]
    fn find_godot_absent_when_dir_has_no_godot() {
        let (dir, _fake) = make_fake_godot("find-absent", false);
        assert_eq!(find_godot_in_dirs([dir.clone()]), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_godot_absent_when_path_is_empty() {
        // An empty iterator must yield None, not fall back to a bare "godot".
        assert_eq!(find_godot_in_dirs(std::iter::empty()), None);
    }

    #[test]
    fn find_godot_present_when_executable_exists() {
        let (dir, fake) = make_fake_godot("find-present", true);
        assert_eq!(find_godot_in_dirs([dir.clone()]), Some(fake));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_godot_skips_non_executable_file() {
        let (dir, fake) = make_fake_godot("find-non-exec", false);
        // On Unix, mode 0o644 has no execute bit — must not be found.
        #[cfg(unix)]
        assert_eq!(find_godot_in_dirs([dir.clone()]), None);
        // On Windows every existing file is treated as executable.
        #[cfg(not(unix))]
        assert_eq!(find_godot_in_dirs([dir.clone()]), Some(fake));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Creates a scratch dir with a `godot` file inside it. When `executable`
    /// is true the file gets the execute bit set (Unix) so `is_executable` accepts it.
    /// Returns `(dir, godot_path)`.
    fn make_fake_godot(tag: &str, executable: bool) -> (PathBuf, PathBuf) {
        let dir = scratch_dir(tag);
        let fake = dir.join("godot");
        std::fs::write(&fake, b"#!/bin/sh\necho godot 4.0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        (dir, fake)
    }

    /// Per-test scratch dir under the target dir (no external temp-dir crate).
    fn scratch_dir(tag: &str) -> PathBuf {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("strict-inject-test-{tag}"));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// Phase 0 spike .2 — live extraction against a real Godot binary.
    /// Skipped when `find_godot()` returns None (godot not on PATH and $GODOT unset).
    #[test]
    fn live_strict_extraction() {
        let Some(godot) = find_godot() else {
            eprintln!("no godot on PATH; skipping");
            return;
        };
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

    /// Injection acceptance test: the fixture project's project.godot *actively
    /// disables* the unsafe/untyped family, yet gdstrict must still surface those
    /// warnings because it injects its own override.cfg. Proves we do not trust the
    /// target's config. Skipped when no Godot is available.
    #[test]
    fn live_injection_overrides_hostile_project() {
        let Some(godot) = find_godot() else {
            eprintln!("no godot; skipping");
            return;
        };
        if Command::new(&godot).arg("--version").output().is_err() {
            eprintln!("godot not runnable; skipping");
            return;
        }
        let project = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/strict_inject_project"
        );
        let override_path = Path::new(project).join("override.cfg");

        let diags = check_script(&godot, Path::new(project), "unsafe.gd").unwrap();
        let codes: Vec<_> = diags.iter().filter_map(|d| d.code).collect();
        // These are all set to 0 in the fixture's project.godot.
        assert!(
            codes.contains(&"UNSAFE_METHOD_ACCESS"),
            "injection failed: project disables this warning, got {diags:#?}"
        );
        assert!(codes.contains(&"UNTYPED_DECLARATION"));
        assert!(codes.contains(&"INTEGER_DIVISION"));

        // The guard must leave no trace in the source tree after the check.
        assert!(
            !override_path.exists(),
            "override.cfg leaked into the project after check_script"
        );
    }
}
