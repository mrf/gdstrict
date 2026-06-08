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

/// A fresh, unique directory under Cargo's per-test-binary temp dir.
///
/// Deliberately distinct from config.rs's same-named unit-test helper: that one
/// roots under `std::env::temp_dir()` because unit tests have no
/// `CARGO_TARGET_TMPDIR`. Don't "unify" them — the divergent root is load-bearing.
fn scratch(tag: &str) -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    dir.push(format!("cli-{tag}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, contents).expect("write fixture");
}

fn code(output: &std::process::Output) -> i32 {
    output.status.code().expect("process exited via signal")
}

/// A one-line array literal long enough to wrap below ~30 columns but fit at 100.
const LONG_ARRAY: &str = "var items = [aaaaaa, bbbbbb, cccccc, dddddd, eeeeee, ffffff]\n";

#[test]
fn check_passes_on_already_formatted_file() {
    // Canonical form uses tab indentation (gdstrict-format output, GDScript style).
    let formatted = "extends Node\n\nfunc _ready() -> void:\n\tpass\n";
    let file = temp_file("formatted.gd", formatted);
    let out = Command::new(bin())
        .args(["format", "--check"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // --check must not rewrite the file.
    assert_eq!(fs::read_to_string(&file).unwrap(), formatted);
}

#[test]
fn check_fails_on_unformatted_file() {
    let file = temp_file("unformatted.gd", "extends Node   \n\n\n");
    let out = Command::new(bin())
        .args(["format", "--check"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        1,
        "expected exit 1 for a file that would change"
    );
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
    assert!(
        stdout.contains("@@"),
        "expected a unified-diff hunk:\n{stdout}"
    );
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

#[test]
fn config_discovered_from_parent_directory() {
    // gdstrict.toml at the root forces a narrow width; the .gd file lives in a
    // nested dir, so discovery must walk UP to find it.
    let dir = scratch("discover");
    write(&dir.join("gdstrict.toml"), "line-length = 20\n");
    let file = dir.join("sub").join("deep").join("a.gd");
    write(&file, LONG_ARRAY);

    let out = Command::new(bin())
        .arg("format")
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // At width 20 the array wraps to multiple lines.
    let formatted = fs::read_to_string(&file).unwrap();
    assert!(
        formatted.lines().count() > 1,
        "expected wrapping from discovered config, got:\n{formatted}"
    );
}

#[test]
fn line_length_flag_overrides_config() {
    // Config would wrap (line-length 20), but --line-length 200 keeps it on one
    // line, so --check sees no change and exits 0.
    let dir = scratch("override");
    write(&dir.join("gdstrict.toml"), "line-length = 20\n");
    let file = dir.join("a.gd");
    write(&file, LONG_ARRAY);

    let overridden = Command::new(bin())
        .args(["format", "--check", "--line-length", "200"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&overridden),
        0,
        "override should keep the file unchanged; stderr: {}",
        String::from_utf8_lossy(&overridden.stderr)
    );

    // Sanity: without the override the discovered config wraps it (exit 1).
    let discovered = Command::new(bin())
        .args(["format", "--check"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&discovered),
        1,
        "discovered narrow config should force a change"
    );
}

#[test]
fn gdignored_file_is_skipped() {
    let dir = scratch("gdignore");
    write(&dir.join(".gdignore"), "ignored.gd\n");
    let ignored = dir.join("ignored.gd");
    let kept = dir.join("kept.gd");
    write(&ignored, "extends Node   \n");
    write(&kept, "extends Node   \n");

    let out = Command::new(bin())
        .arg("format")
        .arg(&dir)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The ignored file is left untouched; the other is formatted.
    assert_eq!(fs::read_to_string(&ignored).unwrap(), "extends Node   \n");
    assert_eq!(fs::read_to_string(&kept).unwrap(), "extends Node\n");
}

#[test]
fn gd_file_under_gitignored_dir_is_not_formatted() {
    let dir = scratch("gitignore");
    write(&dir.join(".gitignore"), "build/\n");
    let buried = dir.join("build").join("generated.gd");
    let main = dir.join("main.gd");
    write(&buried, "extends Node   \n");
    write(&main, "extends Node   \n");

    let out = Command::new(bin())
        .arg("format")
        .arg(&dir)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The gitignored file is never visited; the tracked file is formatted.
    assert_eq!(fs::read_to_string(&buried).unwrap(), "extends Node   \n");
    assert_eq!(fs::read_to_string(&main).unwrap(), "extends Node\n");
}

#[test]
fn bad_config_path_is_an_error() {
    let dir = scratch("badconfig");
    let file = dir.join("a.gd");
    write(&file, "extends Node\n");
    let out = Command::new(bin())
        .args(["format", "--config", "/no/such/gdstrict.toml"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 1);
}
