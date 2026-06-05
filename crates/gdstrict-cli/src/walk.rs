//! Path collection: turn user-supplied paths into a concrete list of `.gd`
//! files to format.
//!
//! Directories are walked recursively via ripgrep's `ignore` crate, which gives
//! us full gitignore semantics for free: `.gitignore`, `.git/info/exclude`, the
//! global gitignore, and nested ignore files are all honored, plus a custom
//! `.gdignore` for project rules that should apply to the formatter but not git.
//! Hidden entries (dotfiles) and `target/` are skipped, matching the previous
//! minimal walker. Explicitly-named files are always honored, even if an ignore
//! rule would exclude them (black/ruff behavior).

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

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
            // Ignore rules deliberately do not apply to explicit file inputs.
            if has_gd_ext(input) {
                files.push(input.clone());
            }
        } else if input.is_dir() {
            let mut builder = WalkBuilder::new(input);
            builder
                .hidden(true) // skip dotfiles / dot-directories
                .parents(true) // honor ignore files in parent directories
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .ignore(true)
                // Apply .gitignore even when the tree is not a git checkout, so
                // ignores behave the same in and out of a repository.
                .require_git(false)
                // Project-specific ignores that should not live in .gitignore.
                .add_custom_ignore_filename(".gdignore");
            // Prune Rust build output; .gd files never live there.
            builder.filter_entry(|e| e.file_name() != "target");

            for result in builder.build() {
                match result {
                    Ok(entry) => {
                        let is_file = entry.file_type().is_some_and(|t| t.is_file());
                        if is_file && has_gd_ext(entry.path()) {
                            files.push(entry.path().to_path_buf());
                        }
                    }
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

fn has_gd_ext(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gd"))
}
