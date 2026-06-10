//! `gdstrict` — the strict-mode Godot formatter and linter CLI.
//!
//! Exit codes (CI / pre-commit friendly):
//!   0  success — files written, or nothing would change under `--check`
//!   1  under `--check`, at least one file would change; OR an error occurred
//!
//! The `check` command (see [`check`]) has its own three-way exit scheme
//! (0 clean / 1 findings / 2 config error) documented in that module.

mod check;
mod config;
mod diff;
mod format;
mod lint_cmd;
mod walk;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "gdstrict",
    version,
    about = "The strict-mode Godot formatter and linter"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Format GDScript (`.gd`) files.
    Format(FormatArgs),

    /// Aggregate gate: format-check + lint + strict, with one unified exit code.
    ///
    /// Exit 0 = clean, 1 = findings, 2 = config error (e.g. strict enabled but no
    /// Godot binary found). Strict is on by default; pass `--no-strict` to skip it.
    Check(CheckArgs),
    /// Run syntactic lint rules over GDScript (`.gd`) files (no Godot needed).
    Lint(LintArgs),
}

#[derive(Args)]
struct FormatArgs {
    /// Don't write any file; exit 1 if any file would change.
    #[arg(long)]
    check: bool,

    /// Print a unified diff per file instead of writing; never writes.
    #[arg(long)]
    diff: bool,

    /// Use this exact gdstrict.toml instead of discovering one per file.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Maximum line length before wrapping (overrides any config file).
    #[arg(long, value_name = "N")]
    line_length: Option<usize>,

    /// Files or directories to format (directories are walked recursively).
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
}

#[derive(Args)]
pub struct CheckArgs {
    /// Path to the Godot binary (overrides `$GODOT` and `PATH`). Must exist.
    #[arg(long, value_name = "PATH")]
    pub godot: Option<PathBuf>,

    /// Skip the strict-typing pass entirely (no Godot binary required).
    #[arg(long)]
    pub no_strict: bool,

    /// Suppress per-finding output; only the summary line and the exit code remain.
    #[arg(long)]
    pub quiet: bool,

    /// Use this exact gdstrict.toml instead of discovering one per file.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Maximum line length for the format-check (overrides any config file).
    #[arg(long, value_name = "N")]
    pub line_length: Option<usize>,

    /// Files or directories to check (directories are walked recursively).
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,
}

#[derive(Args)]
struct LintArgs {
    /// Use this exact gdstrict.toml instead of discovering one per file.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Files or directories to lint (directories are walked recursively).
    /// Defaults to the current directory when omitted.
    #[arg(default_value = ".", value_name = "PATH")]
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Format(args) => run_format(&args),
        Command::Check(args) => check::run(&args),
        Command::Lint(args) => run_lint(&args),
    }
}

fn run_format(args: &FormatArgs) -> ExitCode {
    let mut resolver = match config::Resolver::new(args.config.as_deref(), args.line_length) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(1);
        }
    };

    let (files, walk_errors) = walk::collect_gd_files(&args.paths);

    let mut had_error = false;
    for err in &walk_errors {
        eprintln!("error: {err}");
        had_error = true;
    }

    let write = !args.check && !args.diff;
    let mut changed = 0usize;

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

        let line_length = match resolver.line_length_for(path) {
            Ok(n) => n,
            Err(err) => {
                eprintln!("error: {err}");
                had_error = true;
                continue;
            }
        };

        let formatted = format::format_source(&src, line_length);
        if formatted == src {
            continue;
        }
        changed += 1;

        if args.diff {
            print!("{}", diff::unified_diff(&src, &formatted, &display));
        }
        if args.check {
            eprintln!("would reformat: {display}");
        }
        if write {
            if let Err(err) = std::fs::write(path, &formatted) {
                eprintln!("error: writing {display}: {err}");
                had_error = true;
                continue;
            }
            eprintln!("reformatted: {display}");
        }
    }

    if had_error {
        return ExitCode::from(1);
    }
    if args.check && changed > 0 {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run_lint(args: &LintArgs) -> ExitCode {
    let mut resolver = match config::Resolver::new(args.config.as_deref(), None) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::from(1);
        }
    };
    lint_cmd::run(&args.paths, &mut resolver)
}
