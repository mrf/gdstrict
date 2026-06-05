//! Phase 1 ship-gate acceptance test (PLAN.md sec 3).
//!
//! Formats a real Godot game corpus — the vendored `stagehand_core` addon
//! (~1973 lines of typed, tab-indented Godot 4.6 GDScript: `class_name`,
//! `extends`, `##` doc comments, cross-file `preload`s) — and asserts the two
//! hard invariants on every file:
//!
//!   1. PARSE-SAFE  — `format(src)` re-parses cleanly via the tree-sitter
//!      grammar in `gdstrict-syntax` (zero defects). Broken output = real bug.
//!   2. IDEMPOTENT  — `format(format(src)) == format(src)`.
//!
//! The corpus is committed under `fixtures/acceptance/` so this test never
//! depends on any path outside the repo at runtime.
//!
//! A third, best-effort gate — `godot --headless --check-only` reports no NEW
//! errors on the formatted corpus vs the unformatted corpus — is NOT run here
//! (cargo test must not require the Godot binary). It was verified manually at
//! landing time: all 11 files report 0 errors both before and after formatting
//! when laid out at their real `res://addons/stagehand/core/` path. See the
//! commit message / `acceptance_godot_check.sh` to reproduce.

use std::path::{Path, PathBuf};

/// Absolute path to the vendored acceptance corpus directory.
fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/acceptance/stagehand_core")
}

/// Collect every `.gd` file in the vendored corpus, sorted for stable output.
fn corpus_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .expect("read fixtures/acceptance/stagehand_core")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("gd"))
        .collect();
    files.sort();
    files
}

/// HARD GATE 1: formatted output of every corpus file re-parses with zero
/// defects. A failure here is a real formatter bug on real GDScript — record it
/// as a finding (do not edit the corpus to dodge it).
#[test]
fn corpus_formats_parse_safe() {
    let files = corpus_files();
    assert!(!files.is_empty(), "no .gd fixtures found in {}", corpus_dir().display());
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let formatted = gdstrict_format::format(&src);
        assert!(
            gdstrict_syntax::parses_clean(&formatted),
            "{}: formatted output has parse defects: {:?}",
            path.display(),
            gdstrict_syntax::defects(&gdstrict_syntax::parse(&formatted)),
        );
    }
}

/// HARD GATE 2: formatting is idempotent on every corpus file.
#[test]
fn corpus_formats_idempotent() {
    let files = corpus_files();
    assert!(!files.is_empty(), "no .gd fixtures found in {}", corpus_dir().display());
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let once = gdstrict_format::format(&src);
        let twice = gdstrict_format::format(&once);
        assert_eq!(
            once,
            twice,
            "{}: not idempotent (double-format changed the output)",
            path.display(),
        );
    }
}
