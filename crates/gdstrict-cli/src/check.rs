//! `gdstrict check` — the aggregate CI gate.
//!
//! Runs three independent subsystems over the input `.gd` files and folds their
//! results into one report with a single, unified exit code:
//!
//!   1. **format-check** — reuses the same formatter seam as `format --check`
//!      ([`crate::format`]); a file that would change is a violation.
//!   2. **lint** — reuses `gdstrict-lint`'s default rule set (CST naming rules).
//!   3. **strict** — reuses `gdstrict-strict`, which drives a headless Godot
//!      binary for real type diagnostics. Skipped only on `--no-strict`.
//!
//! ## Exit codes (unified)
//!
//!   0  clean — nothing to report.
//!   1  findings — at least one format/lint/strict *violation* (see below).
//!   2  operational/config error — bad path, unreadable file, invalid config,
//!      or strict requested but no Godot binary is discoverable.
//!
//! ## What counts as a violation (exit 1)
//!
//!   - format: the file is not already in canonical form.
//!   - lint: any diagnostic from the rule set (the catalog has no per-rule
//!     severity downgrade, so every finding fails the gate).
//!   - strict: a diagnostic of [`gdstrict_strict::Severity::Error`] after the
//!     severity profile is applied. Strict *warnings* are printed but do not fail
//!     on their own — that Warn/Error split is the profile's whole point
//!     (`gdstrict_strict::SeverityConfig`).
//!
//! ## Godot discovery & the no-Godot fallback
//!
//! Binary discovery honors `--godot <path>`, then `$GODOT`, then `PATH` (the last
//! two via [`gdstrict_strict::find_godot`]). When strict is enabled (the default)
//! and no binary is found, `check` exits 2 — a missing analyzer is a configuration
//! error, never a silent pass. Pass `--no-strict` to drop the strict pass entirely
//! (and the Godot requirement with it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gdstrict_strict::{Severity, SeverityConfig};

use crate::config::Resolver;
use crate::{format, walk, CheckArgs};

/// Exit code for an operational/configuration error (distinct from "findings").
const EXIT_ERROR: u8 = 2;
/// Exit code when at least one violation was found.
const EXIT_FINDINGS: u8 = 1;

/// Outcome of the run, mapped to a process exit code at the end.
struct Report {
    /// A real violation was found (fails the gate → exit 1).
    violations: usize,
    /// Strict warnings surfaced but not counted as violations (reported only).
    warnings: usize,
    /// An operational/config error occurred (→ exit 2, takes precedence).
    had_error: bool,
}

impl Report {
    fn new() -> Self {
        Self {
            violations: 0,
            warnings: 0,
            had_error: false,
        }
    }

    fn exit_code(&self) -> ExitCode {
        if self.had_error {
            ExitCode::from(EXIT_ERROR)
        } else if self.violations > 0 {
            ExitCode::from(EXIT_FINDINGS)
        } else {
            ExitCode::SUCCESS
        }
    }
}

pub fn run(args: &CheckArgs) -> ExitCode {
    let mut report = Report::new();

    // Parse `--config` / validate `--line-length` once, up front (same seam the
    // `format` command uses), so a bad config is a single error, not per-file.
    let mut resolver = match Resolver::new(args.config.as_deref(), args.line_length) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(EXIT_ERROR);
        }
    };

    // Resolve the Godot binary before walking files, so "strict enabled but no
    // Godot" fails fast as a config error rather than after doing format+lint work.
    let godot = if args.no_strict {
        None
    } else {
        match resolve_godot(args.godot.as_deref()) {
            Ok(path) => Some(path),
            Err(err) => {
                eprintln!("error: {err}");
                eprintln!(
                    "hint: install Godot and put it on PATH, set $GODOT, pass --godot <path>, \
                     or run with --no-strict to skip the strict-typing pass."
                );
                return ExitCode::from(EXIT_ERROR);
            }
        }
    };

    let (files, walk_errors) = walk::collect_gd_files(&args.paths);
    for err in &walk_errors {
        eprintln!("error: {err}");
        report.had_error = true;
    }

    // Memoize the project root (dir containing project.godot) per directory so a
    // large tree is not re-walked for every file.
    let mut project_cache: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    // The strict severity profile. Defaults to the built-in `strict` preset — the
    // whole point of the tool — promoting the untyped/unsafe family to errors.
    let severity = SeverityConfig::strict();

    for path in &files {
        let display = path.display().to_string();
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("error: reading {display}: {err}");
                report.had_error = true;
                continue;
            }
        };

        // ── format-check ──────────────────────────────────────────────────────
        let line_length = match resolver.line_length_for(path) {
            Ok(n) => n,
            Err(err) => {
                eprintln!("error: {err}");
                report.had_error = true;
                continue;
            }
        };
        if format::format_source(&src, line_length) != src {
            report.violations += 1;
            if !args.quiet {
                eprintln!("{display}: would reformat (run `gdstrict format`)");
            }
        }

        // ── lint ────────────────────────────────────────────────────────────────
        for d in gdstrict_lint::lint(&src) {
            report.violations += 1;
            if !args.quiet {
                eprintln!(
                    "{display}:{}:{} [lint:{}] {}",
                    d.line, d.column, d.rule, d.message
                );
            }
        }

        // ── strict ────────────────────────────────────────────────────────────
        if let Some(godot) = &godot {
            run_strict(
                godot,
                path,
                &display,
                &severity,
                &mut project_cache,
                args.quiet,
                &mut report,
            );
        }
    }

    if !args.quiet {
        print_summary(&report);
    }
    report.exit_code()
}

/// Run the strict pass for a single file: locate its enclosing Godot project,
/// invoke the analyzer, apply the severity profile, and fold results into `report`.
fn run_strict(
    godot: &Path,
    path: &Path,
    display: &str,
    severity: &SeverityConfig,
    project_cache: &mut HashMap<PathBuf, Option<PathBuf>>,
    quiet: bool,
    report: &mut Report,
) {
    let Some(project_dir) = find_project_root(path, project_cache) else {
        // No enclosing project.godot — Godot's analyzer needs a project, so this
        // file simply cannot be type-checked. This is an input limitation, not a
        // violation: note it (unless quiet) and skip without failing the gate.
        if !quiet {
            eprintln!("{display}: skipping strict (no project.godot found above this file)");
        }
        return;
    };

    let Ok(script_rel) = path.strip_prefix(&project_dir) else {
        // Should not happen — find_project_root returns an ancestor — but never panic.
        if !quiet {
            eprintln!("{display}: skipping strict (could not relativize against project root)");
        }
        return;
    };
    let script_rel = script_rel.to_string_lossy();

    let diags = match gdstrict_strict::check_script(godot, &project_dir, &script_rel) {
        Ok(d) => severity.apply(d),
        Err(err) => {
            eprintln!("error: running strict check on {display}: {err}");
            report.had_error = true;
            return;
        }
    };

    for d in diags {
        let label = match d.severity {
            Severity::Error => "strict:error",
            Severity::Warning => "strict:warning",
        };
        let code = d.code.unwrap_or("");
        let code = if code.is_empty() {
            String::new()
        } else {
            format!(" {code}")
        };
        match d.severity {
            Severity::Error => report.violations += 1,
            Severity::Warning => report.warnings += 1,
        }
        if !quiet {
            eprintln!("{display}:{} [{label}]{code} {}", d.line, d.message);
        }
    }
}

/// Resolve the Godot binary honoring `--godot`, then `$GODOT`, then `PATH`.
///
/// An explicit `--godot` that does not exist is a hard error (we do not silently
/// fall back to PATH — the user named a specific binary and meant it).
fn resolve_godot(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(format!("--godot path does not exist: {}", p.display()));
    }
    gdstrict_strict::find_godot()
        .ok_or_else(|| "strict mode is enabled but no Godot binary was found".to_string())
}

/// Walk up from `file` to the nearest directory containing `project.godot`.
/// Results are memoized per starting directory. Returns `None` if no project
/// root is found before the filesystem root.
fn find_project_root(
    file: &Path,
    cache: &mut HashMap<PathBuf, Option<PathBuf>>,
) -> Option<PathBuf> {
    let start = file.parent().unwrap_or_else(|| Path::new("."));
    if let Some(hit) = cache.get(start) {
        return hit.clone();
    }
    let mut found = None;
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join("project.godot").is_file() {
            found = Some(d.to_path_buf());
            break;
        }
        dir = d.parent();
    }
    cache.insert(start.to_path_buf(), found.clone());
    found
}

fn print_summary(report: &Report) {
    if report.had_error {
        // The specific errors were already printed; just flag the overall result.
        eprintln!("check: errors occurred (see above)");
        return;
    }
    if report.violations == 0 && report.warnings == 0 {
        eprintln!("check: clean");
        return;
    }
    let mut parts = Vec::new();
    if report.violations > 0 {
        parts.push(format!("{} violation(s)", report.violations));
    }
    if report.warnings > 0 {
        parts.push(format!("{} warning(s)", report.warnings));
    }
    eprintln!("check: {}", parts.join(", "));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_godot_must_exist() {
        let err = resolve_godot(Some(Path::new("/no/such/godot/binary"))).unwrap_err();
        assert!(err.contains("--godot"), "got: {err}");
    }

    #[test]
    fn find_project_root_walks_up() {
        // Build a scratch project: <dir>/project.godot and <dir>/sub/a.gd.
        let mut dir = std::env::temp_dir();
        dir.push("gdstrict-check-projroot");
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join("project.godot"), "config_version=5\n").unwrap();
        let file = sub.join("a.gd");
        std::fs::write(&file, "extends Node\n").unwrap();

        let mut cache = HashMap::new();
        assert_eq!(find_project_root(&file, &mut cache).as_deref(), Some(&*dir));
        // Second lookup is served from the cache (same answer).
        assert_eq!(find_project_root(&file, &mut cache).as_deref(), Some(&*dir));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_project_root_none_when_absent() {
        let mut dir = std::env::temp_dir();
        dir.push("gdstrict-check-noproj");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("loose.gd");
        std::fs::write(&file, "extends Node\n").unwrap();

        let mut cache = HashMap::new();
        assert_eq!(find_project_root(&file, &mut cache), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
