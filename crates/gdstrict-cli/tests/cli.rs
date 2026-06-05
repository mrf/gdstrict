//! Integration tests that exercise the built `gdstrict` binary end-to-end.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Path to the compiled binary under test (set by Cargo).
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_gdstrict")
}

/// A unique temp file under Cargo's per-test-binary temp dir.
fn temp_file(name: &str, contents: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    path.push(name);
    fs::write(&path, contents).expect("write temp fixture");
    path
}

fn code(output: &std::process::Output) -> i32 {
    output.status.code().expect("process exited via signal")
}

#[test]
fn check_passes_on_already_formatted_file() {
    let file = temp_file(
        "formatted.gd",
        "extends Node\n\nfunc _ready() -> void:\n\tpass\n",
    );
    let out = Command::new(bin())
        .args(["format", "--check"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 0, "stderr: {}", String::from_utf8_lossy(&out.stderr));
    // --check must not rewrite the file.
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "extends Node\n\nfunc _ready() -> void:\n\tpass\n"
    );
}

#[test]
fn check_fails_on_unformatted_file() {
    let file = temp_file("unformatted.gd", "extends Node   \n\n\n");
    let out = Command::new(bin())
        .args(["format", "--check"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 1, "expected exit 1 for a file that would change");
    // Still must not have written anything.
    assert_eq!(fs::read_to_string(&file).unwrap(), "extends Node   \n\n\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("would reformat"));
}

#[test]
fn format_writes_in_place_and_is_idempotent() {
    let file = temp_file("rewrite.gd", "extends Node   \n\n\n");
    let out = Command::new(bin())
        .arg("format")
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 0);
    assert_eq!(fs::read_to_string(&file).unwrap(), "extends Node\n");

    // Second run changes nothing, and --check now passes.
    let out2 = Command::new(bin())
        .args(["format", "--check"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out2), 0);
}

#[test]
fn diff_prints_unified_diff_without_writing() {
    let file = temp_file("diffme.gd", "extends Node   \n");
    let out = Command::new(bin())
        .args(["format", "--diff"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("@@"), "expected a unified-diff hunk:\n{stdout}");
    // --diff never writes.
    assert_eq!(fs::read_to_string(&file).unwrap(), "extends Node   \n");
}

#[test]
fn missing_path_is_an_error() {
    let out = Command::new(bin())
        .args(["format", "/no/such/path/here.gd"])
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 1);
}
