//! `strict-bench` — wall-clock comparison of strict-check invocation strategies.
//!
//! Phase 3 follow-up to spike .2 (`godot-linter-phase3-strict-6zk.2`). Measures the four
//! ways gdstrict could drive Godot's analyzer over a whole project and prints a Markdown
//! table you can paste into `docs/phase3/bench-strict-invocation.md`:
//!
//! 1. **per-file-serial** — [`check_script`] in a loop: one (or two) engine boots *per
//!    file*. The naive baseline.
//! 2. **per-file-pool** — [`check_scripts`]: the same per-file boots across a bounded
//!    worker pool (the Phase 2 concurrency win).
//! 3. **batch** — [`check_project_batch`]: a throwaway harness `.gd` that `load()`s every
//!    file, so the analyzer sees the whole corpus in **one boot per pass** (two total).
//! 4. **whole-project-native** — `godot --headless --check-only --path P` with no
//!    `--script`. Probed with a timeout to show empirically that Godot has **no** native
//!    whole-project check mode: it ignores "check" and runs the project instead.
//!
//! Engine startup (~0.4s here) dwarfs the actual parse, so the strategy that amortizes
//! boots across files wins. Run it yourself:
//!
//! ```text
//! GODOT=/path/to/godot cargo run -p gdstrict-strict --bin strict-bench --release
//! ```
//!
//! Skips cleanly (exit 0) when no Godot is on `$GODOT`/PATH, so it is safe in CI.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use gdstrict_strict::{check_project_batch, check_script, check_scripts, find_godot};

/// How long to let the native whole-project probe run before declaring it non-terminating.
const NATIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

fn main() -> std::io::Result<()> {
    let Some(godot) = find_godot() else {
        eprintln!("strict-bench: no Godot on $GODOT or PATH — skipping (this is not a failure).");
        return Ok(());
    };
    eprintln!("strict-bench: using godot at {}", godot.display());

    // The real acceptance corpus is the "realistic project size" the issue asks for; the
    // synthetic sizes extrapolate toward CI-scale without needing a giant real project.
    let scratch = std::env::temp_dir().join(format!("gdstrict-bench-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;

    let mut rows: Vec<Row> = Vec::new();

    // 1) Real corpus (acceptance/stagehand_core).
    if let Some((project, rels)) = real_corpus() {
        eprintln!("strict-bench: real corpus = {} files", rels.len());
        rows.push(bench_all(&godot, &project, &rels, "acceptance (real)"));
    } else {
        eprintln!("strict-bench: real corpus not found; skipping it");
    }

    // 2) Synthetic projects at a few sizes to show the boot-amortization curve.
    for &n in &[10usize, 25, 50] {
        let project = scratch.join(format!("synth-{n}"));
        let rels = make_synthetic_project(&project, n)?;
        eprintln!("strict-bench: synthetic corpus = {n} files");
        rows.push(bench_all(
            &godot,
            &project,
            &rels,
            &format!("synthetic ×{n}"),
        ));
    }

    print_table(&rows);

    // Best-effort cleanup of the synthetic scratch tree.
    std::fs::remove_dir_all(&scratch).ok();
    Ok(())
}

/// One benchmarked corpus: timings for each strategy plus the native probe verdict.
struct Row {
    label: String,
    files: usize,
    serial: Duration,
    pool: Duration,
    batch: Duration,
    native: String,
}

/// Run every strategy over one corpus and collect a [`Row`].
fn bench_all(godot: &Path, project: &Path, rels: &[String], label: &str) -> Row {
    let serial = time(|| {
        for rel in rels {
            // Ignore per-file errors: we are timing invocation cost, not asserting output.
            let _ = check_script(godot, project, rel);
        }
    });

    let jobs: Vec<(PathBuf, String)> = rels
        .iter()
        .map(|r| (project.to_path_buf(), r.clone()))
        .collect();
    let pool = time(|| {
        let _ = check_scripts(godot, &jobs);
    });

    let batch = time(|| {
        let _ = check_project_batch(godot, project, rels);
    });

    let native = probe_native_whole_project(godot, project);

    Row {
        label: label.to_string(),
        files: rels.len(),
        serial,
        pool,
        batch,
        native,
    }
}

/// Probe `--check-only --path P` with no `--script`. Returns a human verdict. We expect
/// it to *not* terminate (Godot has no native whole-project check; it runs the project),
/// so we bound it and kill it rather than hang the benchmark.
fn probe_native_whole_project(godot: &Path, project: &Path) -> String {
    let spawn = Command::new(godot)
        .arg("--headless")
        .arg("--check-only")
        .arg("--path")
        .arg(project)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match spawn {
        Ok(c) => c,
        Err(e) => return format!("spawn failed: {e}"),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return format!("exited {} in {:.2}s", status, start.elapsed().as_secs_f64());
            }
            Ok(None) if start.elapsed() >= NATIVE_PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return format!("did not terminate in {NATIVE_PROBE_TIMEOUT:?} (runs project)");
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return format!("wait failed: {e}"),
        }
    }
}

/// Time a closure's wall-clock duration.
fn time<F: FnOnce()>(f: F) -> Duration {
    let start = Instant::now();
    f();
    start.elapsed()
}

/// Locate the real acceptance corpus shipped in the repo, returning
/// `(project_dir, project-relative script paths)`.
fn real_corpus() -> Option<(PathBuf, Vec<String>)> {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/acceptance");
    let dir = project.join("stagehand_core");
    if !dir.is_dir() {
        return None;
    }
    let mut rels: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".gd"))
        .map(|n| format!("stagehand_core/{n}"))
        .collect();
    rels.sort();
    if rels.is_empty() {
        return None;
    }
    Some((project, rels))
}

/// Build a synthetic Godot project of `n` self-contained typed scripts (each provoking a
/// couple of analyzer warnings) and return their project-relative paths. Self-contained
/// files keep the size knob clean — no cross-file `class_name`/`preload` breakage as the
/// count grows.
fn make_synthetic_project(project: &Path, n: usize) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(project)?;
    std::fs::write(
        project.join("project.godot"),
        "config_version=5\n\
         [application]\n\
         config/name=\"gdstrict-bench-synth\"\n\
         config/features=PackedStringArray(\"4.6\")\n",
    )?;
    let mut rels = Vec::with_capacity(n);
    for i in 0..n {
        let name = format!("s{i}.gd");
        // Untyped `var` → UNTYPED_DECLARATION; `/` on ints → INTEGER_DIVISION.
        std::fs::write(
            project.join(&name),
            format!(
                "extends Node\n\n\
                 func _ready() -> void:\n\
                 \tvar value_{i} = {i} + 1\n\
                 \tvar half_{i} = value_{i} / 2\n\
                 \tprint(half_{i})\n"
            ),
        )?;
        rels.push(name);
    }
    Ok(rels)
}

/// Print the results as a Markdown table (paste-ready for the bench doc).
fn print_table(rows: &[Row]) {
    println!("\n## Results (this machine)\n");
    println!(
        "| corpus | files | per-file-serial | per-file-pool | batch | speedup (serial→batch) |"
    );
    println!("|---|---:|---:|---:|---:|---:|");
    for r in rows {
        let speedup = if r.batch.as_secs_f64() > 0.0 {
            r.serial.as_secs_f64() / r.batch.as_secs_f64()
        } else {
            0.0
        };
        println!(
            "| {} | {} | {:.2}s | {:.2}s | {:.2}s | {:.1}× |",
            r.label,
            r.files,
            r.serial.as_secs_f64(),
            r.pool.as_secs_f64(),
            r.batch.as_secs_f64(),
            speedup,
        );
    }
    println!("\n**whole-project-native probe** (`--check-only --path P`, no `--script`):\n");
    for r in rows {
        println!("- {} ({} files): {}", r.label, r.files, r.native);
    }
}
