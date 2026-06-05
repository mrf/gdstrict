//! `gdstrict` — the strict-mode Godot formatter CLI.
//!
//! Exit codes (CI / pre-commit friendly):
//!   0  success — files written, or nothing would change under `--check`
//!   1  under `--check`, at least one file would change; OR an error occurred

mod diff;
mod format;
mod walk;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gdstrict", version, about = "The strict-mode Godot formatter")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Format GDScript (`.gd`) files.
    Format(FormatArgs),
}

#[derive(Args)]
struct FormatArgs {
    /// Don't write any file; exit 1 if any file would change.
    #[arg(long)]
    check: bool,

    /// Print a unified diff per file instead of writing; never writes.
    #[arg(long)]
    diff: bool,

    /// Files or directories to format (directories are walked recursively).
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Format(args) => run_format(&args),
    }
}

fn run_format(args: &FormatArgs) -> ExitCode {
    let (files, walk_errors) = walk::collect_gd_files(&args.paths);

    let mut had_error = false;
    for err in &walk_errors {
        eprintln!("error: {err}");
        had_error = true;
    }

    // Write in place only in the default mode. `--check` and `--diff` never write.
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

        let formatted = format::format_source(&src);
        if formatted == src {
            continue;
        }
        changed += 1;

        // Diffs go to stdout (pipeable); status lines go to stderr.
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
