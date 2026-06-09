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

/// A `gdstrict` command whose environment has **no** discoverable Godot: `$GODOT`
/// is removed and `$PATH` is emptied, so `find_godot()` returns `None` regardless
/// of the host machine. The binary itself is launched by absolute path (Cargo's
/// `CARGO_BIN_EXE_*`), so the empty `PATH` does not affect spawning it — only the
/// child's own Godot discovery. Used by the no-Godot fallback tests; the env pair
/// is the load-bearing contract, so it lives in one place.
fn no_godot_cmd() -> Command {
    let mut cmd = Command::new(bin());
    cmd.env_remove("GODOT").env("PATH", "");
    cmd
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

// ─── check command ──────────────────────────────────────────────────────────────

/// A clean, canonically-formatted, lint-passing file: `check --no-strict` exits 0.
#[test]
fn check_no_strict_passes_clean_file() {
    let formatted = "extends Node\n\nfunc _ready() -> void:\n\tpass\n";
    let file = temp_file("check_clean.gd", formatted);
    let out = Command::new(bin())
        .args(["check", "--no-strict"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("check: clean"));
}

/// An unformatted file is a violation: `check --no-strict` exits 1 and says so.
#[test]
fn check_no_strict_fails_on_unformatted() {
    let file = temp_file("check_unformatted.gd", "extends Node   \n\n\n");
    let out = Command::new(bin())
        .args(["check", "--no-strict"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 1, "expected findings exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("would reformat"), "stderr: {stderr}");
    // check must never rewrite files.
    assert_eq!(fs::read_to_string(&file).unwrap(), "extends Node   \n\n\n");
}

/// A naming-rule violation is a finding: `check --no-strict` exits 1 with a lint line.
#[test]
fn check_no_strict_reports_lint_findings() {
    // PascalCase function name violates `function-name-case`. Keep it otherwise
    // canonical so format-check does not also fire (we want to prove lint alone fails).
    let file = temp_file("check_lint.gd", "func DoThing() -> void:\n\tpass\n");
    let out = Command::new(bin())
        .args(["check", "--no-strict"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 1, "expected findings exit code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[lint:function-name-case]"),
        "stderr: {stderr}"
    );
}

/// `--quiet` suppresses per-finding lines but still sets the exit code.
#[test]
fn check_quiet_suppresses_findings_but_keeps_exit_code() {
    let file = temp_file("check_quiet.gd", "extends Node   \n\n\n");
    let out = Command::new(bin())
        .args(["check", "--no-strict", "--quiet"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("would reformat"),
        "--quiet should not print findings; stderr: {stderr}"
    );
}

/// Strict is enabled by default; an explicit `--godot` that does not exist is a
/// configuration error (exit 2), distinct from a findings exit (1).
#[test]
fn check_missing_godot_is_config_error() {
    let file = temp_file("check_cfgerr.gd", "extends Node\n");
    let out = Command::new(bin())
        .args(["check", "--godot", "/no/such/godot/binary"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        2,
        "missing Godot while strict is enabled must be a config error (exit 2); stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A non-existent input path is an operational error (exit 2).
#[test]
fn check_missing_path_is_config_error() {
    let out = Command::new(bin())
        .args(["check", "--no-strict", "/no/such/path/here.gd"])
        .output()
        .expect("run gdstrict");
    assert_eq!(code(&out), 2);
}

/// Phase 2 acceptance (PLAN.md §3): the untyped/unsafe fixture project → `check`
/// exits non-zero **with the exact expected warning codes**. The strict preset
/// promotes the untyped/unsafe family to errors (exit 1).
///
/// This also pins the canonical config contract: `check` discovers and honors the
/// project's own `gdstrict.toml`. That fixture sets `INTEGER_DIVISION = "off"`, so
/// the per-project override wins over the preset and the code is **suppressed
/// entirely** — it must not surface as either a warning or an error. (Before
/// per-project severity was wired in, `check` used the hardcoded built-in preset
/// and INTEGER_DIVISION leaked through as a warning; that is the bug this fixes.)
/// Skipped when no Godot is discoverable — the no-Godot path is asserted below.
#[test]
fn check_strict_flags_unsafe_fixture_with_exact_codes() {
    if gdstrict_strict::find_godot().is_none() {
        eprintln!("no godot on PATH and $GODOT unset; skipping live strict check");
        return;
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/strict_project/unsafe.gd");
    let out = Command::new(bin())
        .args(["check"])
        .arg(&fixture)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        1,
        "strict should flag the unsafe fixture; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Exact promoted-to-error codes the fixture must surface. Each appears as a
    // `[strict:error] <CODE>` line; assert on the code so the test pins the
    // analyzer contract, not just "something failed".
    for code in [
        "UNTYPED_DECLARATION",
        "UNSAFE_METHOD_ACCESS",
        "UNSAFE_CAST",
        "RETURN_VALUE_DISCARDED",
    ] {
        assert!(
            stderr.contains(&format!("[strict:error] {code}")),
            "expected strict error {code}; stderr: {stderr}"
        );
    }
    // INTEGER_DIVISION appears in the fixture source, but the project's
    // gdstrict.toml sets it `off`. Honoring that per-project override suppresses it
    // completely: it must not appear as a warning OR an error.
    assert!(
        !stderr.contains("INTEGER_DIVISION"),
        "fixture gdstrict.toml sets INTEGER_DIVISION=off; the override must suppress it; stderr: {stderr}"
    );
}

/// Phase 2 acceptance (PLAN.md §3): the clean, fully-typed fixture project →
/// `check` exits 0. Every declaration is typed and every access statically safe,
/// so gdstrict's injected strict warning set produces nothing. This is the
/// exit-0 half that proves the gate does not fire on conforming code.
/// Skipped when no Godot is discoverable.
#[test]
fn check_strict_passes_clean_typed_fixture() {
    if gdstrict_strict::find_godot().is_none() {
        eprintln!("no godot on PATH and $GODOT unset; skipping live strict check");
        return;
    }
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/strict_clean_project/clean.gd");
    let out = Command::new(bin())
        .args(["check"])
        .arg(&fixture)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        0,
        "clean typed fixture must pass strict; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("check: clean"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Phase 2 acceptance (PLAN.md §3) — the no-Godot fallback path: when strict is
/// enabled (the default) but **no** Godot binary is discoverable, `check` exits 2
/// (a configuration error), never a silent pass. This is the discovery-returns-None
/// branch, distinct from `check_missing_godot_is_config_error` (explicit `--godot`
/// to a missing path). We force discovery to fail by clearing `$GODOT` and `$PATH`
/// for the child, so `find_godot()` finds nothing regardless of the host machine.
#[test]
fn check_no_godot_discovered_is_config_error() {
    let file = temp_file("check_nogodot.gd", "extends Node\n");
    let out = no_godot_cmd()
        .args(["check"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        2,
        "strict enabled + no Godot found must be a config error (exit 2); stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The companion to the fallback test: with `--no-strict` the same no-Godot
/// environment is fine — strict is dropped along with its Godot requirement, so a
/// clean file exits 0. Proves the exit-2 above is strictly about the strict pass.
#[test]
fn check_no_strict_passes_without_godot() {
    let formatted = "extends Node\n\nfunc _ready() -> void:\n\tpass\n";
    let file = temp_file("check_nogodot_nostrict.gd", formatted);
    let out = no_godot_cmd()
        .args(["check", "--no-strict"])
        .arg(&file)
        .output()
        .expect("run gdstrict");
    assert_eq!(
        code(&out),
        0,
        "--no-strict must not require Godot; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── lint subcommand ───────────────────────────────────────────────────────────

#[test]
fn lint_exits_zero_on_clean_file() {
    let file = temp_file(
        "lint_clean.gd",
        "func do_thing() -> void:\n\tpass\n",
    );
    let out = Command::new(bin())
        .args(["lint"])
        .arg(&file)
        .output()
        .expect("run gdstrict lint");
    assert_eq!(
        code(&out),
        0,
        "clean file should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_exits_nonzero_on_violation() {
    let file = temp_file(
        "lint_violation.gd",
        "func DoThing() -> void:\n\tpass\n",
    );
    let out = Command::new(bin())
        .args(["lint"])
        .arg(&file)
        .output()
        .expect("run gdstrict lint");
    assert_eq!(
        code(&out),
        1,
        "file with PascalCase function should exit 1"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("function-name-case"),
        "stderr should name the violated rule; got: {stderr}"
    );
    assert!(
        stderr.contains("DoThing"),
        "stderr should name the offending identifier; got: {stderr}"
    );
}

#[test]
fn lint_reports_line_and_column() {
    let file = temp_file("lint_location.gd", "func BadName() -> void:\n\tpass\n");
    let out = Command::new(bin())
        .args(["lint"])
        .arg(&file)
        .output()
        .expect("run gdstrict lint");
    assert_eq!(code(&out), 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Line 1, column 5 (after "func ").
    assert!(
        stderr.contains("1:5"),
        "expected 1:5 location in output; got: {stderr}"
    );
}

#[test]
fn lint_disabled_rule_not_reported() {
    let dir = scratch("lint-disable-rule");
    // Disable the function-name-case rule via config.
    write(
        &dir.join("gdstrict.toml"),
        "[lint]\nfunction-name-case = false\n",
    );
    let file = dir.join("bad.gd");
    write(&file, "func DoThing() -> void:\n\tpass\n");

    let out = Command::new(bin())
        .args(["lint"])
        .arg(&file)
        .output()
        .expect("run gdstrict lint");
    assert_eq!(
        code(&out),
        0,
        "disabled rule should not trigger; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lint_config_flag_overrides_discovery() {
    let dir = scratch("lint-config-flag");
    // Discoverable config enables everything (no disabled rules).
    write(&dir.join("gdstrict.toml"), "# no lint overrides\n");

    // Explicit config disables function-name-case.
    let explicit = dir.join("explicit.toml");
    write(&explicit, "[lint]\nfunction-name-case = false\n");

    let file = dir.join("a.gd");
    write(&file, "func DoThing() -> void:\n\tpass\n");

    // With --config pointing to the explicit file: rule is off → exit 0.
    let out = Command::new(bin())
        .args(["lint", "--config"])
        .arg(&explicit)
        .arg(&file)
        .output()
        .expect("run gdstrict lint --config");
    assert_eq!(
        code(&out),
        0,
        "explicit config disabling rule should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Without --config: discovery finds the discoverable config (no disables) → exit 1.
    let out2 = Command::new(bin())
        .args(["lint"])
        .arg(&file)
        .output()
        .expect("run gdstrict lint (no --config)");
    assert_eq!(
        code(&out2),
        1,
        "without explicit config, rule is enabled → violation → exit 1"
    );
}

#[test]
fn lint_walks_directory_recursively() {
    let dir = scratch("lint-walk");
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).expect("create subdir");
    write(&dir.join("root.gd"), "func do_thing() -> void:\n\tpass\n");
    write(&sub.join("nested.gd"), "func DoThing() -> void:\n\tpass\n");

    let out = Command::new(bin())
        .args(["lint"])
        .arg(&dir)
        .output()
        .expect("run gdstrict lint on directory");
    assert_eq!(
        code(&out),
        1,
        "violation in nested file should propagate; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nested.gd"),
        "stderr should mention the offending file; got: {stderr}"
    );
}

#[test]
fn lint_missing_path_is_an_error() {
    let out = Command::new(bin())
        .args(["lint", "/no/such/path/here.gd"])
        .output()
        .expect("run gdstrict lint");
    assert_eq!(code(&out), 1);
}
