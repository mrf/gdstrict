//! Path collection: turn user-supplied paths into a concrete list of `.gd`
//! files to format.
//!
//! Directories are walked recursively. The ignore rules are deliberately simple
//! for now (hidden entries + `target/`); a proper `.gdstrictignore` / gitignore
//! integration is deferred — see `.orchestrator-findings.jsonl`.

use std::path::{Path, PathBuf};

use walkdir::{DirEntry, WalkDir};

/// Resolve the input paths to a deduplicated, sorted list of `.gd` files.
///
/// Returns `(files, errors)`. `errors` collects non-fatal-per-path problems
/// (missing paths, directory-traversal IO errors) that the caller maps to a
/// non-zero exit code. Explicitly-named non-`.gd` files are skipped silently.
pub fn collect_gd_files(inputs: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();

    for input in inputs {
        if input.is_file() {
            // Honor an explicitly-named file, but only if it is GDScript.
            if has_gd_ext(input) {
                files.push(input.clone());
            }
        } else if input.is_dir() {
            for entry in WalkDir::new(input)
                .into_iter()
                .filter_entry(|e| e.depth() == 0 || !is_ignored(e))
            {
                match entry {
                    Ok(e) if e.file_type().is_file() && has_gd_ext(e.path()) => {
                        files.push(e.path().to_path_buf());
                    }
                    Ok(_) => {}
                    Err(err) => errors.push(err.to_string()),
                }
            }
        } else {
            errors.push(format!("{}: no such file or directory", input.display()));
        }
    }

    files.sort();
    files.dedup();
    (files, errors)
}

/// Skip hidden entries (name starts with `.`) and any directory named `target`.
fn is_ignored(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() && name == "target" {
        return true;
    }
    name.starts_with('.')
}

fn has_gd_ext(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("gd"))
}
