//! The `lint` subcommand: syntactic style rules over GDScript files (no Godot needed).
//!
//! Runs every enabled rule from [`gdstrict_lint::rules::default_rules`] over each
//! file, renders findings to stderr, and exits non-zero on any violation or IO error.

use std::path::PathBuf;
use std::process::ExitCode;

use gdstrict_lint::Severity;

use crate::config::Resolver;

/// Run the lint command over `paths` using `resolver` for config discovery.
pub fn run(paths: &[PathBuf], resolver: &mut Resolver) -> ExitCode {
    let (files, walk_errors) = crate::walk::collect_gd_files(paths);

    let mut had_error = false;
    for err in &walk_errors {
        eprintln!("error: {err}");
        had_error = true;
    }

    let mut total_violations = 0usize;

    for path in &files {
        let display = path.display().to_string();
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("error: reading {display}: {err}");
                had_error = true;
                continue;
            }
        };

        let lint_config = match resolver.lint_config_for(path) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("error: {err}");
                had_error = true;
                continue;
            }
        };

        let rules: Vec<Box<dyn gdstrict_lint::Rule>> = gdstrict_lint::rules::default_rules()
            .into_iter()
            .filter(|r| lint_config.is_enabled(r.id()))
            .collect();

        let diags = gdstrict_lint::lint_with(&src, &rules);
        total_violations += diags.len();

        for d in &diags {
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            // GCC/Clang/rustc-compatible format: editors parse "file:line:col:" for
            // jump-to-location. Any future strict-mode render path must use the same
            // format so editor integrations work uniformly across both diagnostic streams.
            eprintln!("{display}:{}:{}: {sev}[{}]: {}", d.line, d.column, d.rule, d.message);
        }
    }

    if had_error || total_violations > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
