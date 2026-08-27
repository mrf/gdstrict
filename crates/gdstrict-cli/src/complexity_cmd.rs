//! The `complexity` subcommand: per-function McCabe complexity, as a report.
//!
//! This is a *report*, not a gate — it exits 0 no matter how complex the code is
//! (only IO/config errors exit 1). The gate is the `max-complexity` lint rule.
//!
//! Its reason for existing is CRAP scores: `c² × (1 − cov)³ + c` needs a `c` per
//! function plus the span to join coverage line hits against, so every record
//! carries `line` / `end_line`. Coverage stays outside gdstrict — whichever tool
//! owns it joins on `(file, line..=end_line)` and does the arithmetic.
//!
//! Records go to **stdout** (machine-readable), errors to stderr.

use std::process::ExitCode;

use serde::Serialize;

use crate::config::Resolver;
use crate::ComplexityArgs;

/// Output shape of one function. Lives here rather than in `gdstrict-lint` so the
/// lint crate stays serde-free; the field names are the public JSON contract.
#[derive(Serialize)]
struct Record {
    file: String,
    name: String,
    /// 1-based line of the `func` keyword.
    line: usize,
    /// 0-based column, matching the `lint` command's positions.
    column: usize,
    /// 1-based last line of the function, inclusive.
    end_line: usize,
    complexity: usize,
}

/// How to render the report.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// `file:line:col: name complexity` — greppable, editor-parsable prefix.
    Text,
    /// A JSON array of records; the shape CRAP tooling consumes.
    Json,
}

/// Run the complexity report over `paths`.
pub fn run(args: &ComplexityArgs, resolver: &mut Resolver) -> ExitCode {
    let (files, walk_errors) = crate::walk::collect_gd_files(&args.paths);

    let mut had_error = false;
    for err in &walk_errors {
        eprintln!("error: {err}");
        had_error = true;
    }

    let mut records: Vec<Record> = Vec::new();

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

        // Resolve config even though nothing here is configurable yet: a bad
        // gdstrict.toml should fail this command the same way it fails the others,
        // rather than being silently ignored.
        if let Err(err) = resolver.lint_config_for(path) {
            eprintln!("error: {err}");
            had_error = true;
            continue;
        }

        for f in gdstrict_lint::complexity::functions(&src) {
            if f.complexity < args.min {
                continue;
            }
            records.push(Record {
                file: display.clone(),
                name: f.name,
                line: f.line,
                column: f.column,
                end_line: f.end_line,
                complexity: f.complexity,
            });
        }
    }

    // `collect_gd_files` walks in a stable order and `functions` returns source
    // order, so records are already sorted by (file, line) — the sort is a cheap
    // guarantee that the report never depends on walk order.
    records.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));

    match args.format {
        Format::Text => {
            for r in &records {
                println!(
                    "{}:{}:{}: {} {}",
                    r.file, r.line, r.column, r.name, r.complexity
                );
            }
        }
        Format::Json => match serde_json::to_string_pretty(&records) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                eprintln!("error: serializing report: {err}");
                had_error = true;
            }
        },
    }

    if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
