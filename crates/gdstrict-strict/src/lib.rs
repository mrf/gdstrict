//! gdstrict-strict: drive Godot's headless analyzer and turn its stderr into
//! structured diagnostics.
//!
//! ## Strategy (Phase 0 spike .2 finding)
//!
//! Godot's `--check-only` surfaces *errors* but not *warnings*; adding `--debug`
//! surfaces warnings but drops into the debugger and **crashes (signal 11)** when a
//! hard parse error is present. So we run two passes:
//!
//! 1. **Errors** — `godot --headless --check-only --path P --script F` (no `--debug`).
//!    Emits `SCRIPT ERROR:` / `ERROR:` lines, exits 1. Never crashes.
//! 2. **Warnings** — only if pass 1 is clean: `... --check-only --debug ...`.
//!    Emits `WARNING:` lines. Safe because no hard error is present to trip the debugger.
//!
//! Output format (Godot 4.6.2), two lines per diagnostic:
//! ```text
//! WARNING: <message>
//!      at: GDScript::reload (res://path.gd:LINE)
//! SCRIPT ERROR: Parse Error: <message>
//!           at: GDScript::reload (res://path.gd:LINE)
//! ```
//!
//! ## Injected warning settings (do not trust project.godot)
//!
//! Godot only emits the unsafe/untyped warning family when it is enabled under
//! `[debug] gdscript/warnings/*` in `project.godot` — and that family is off by
//! default. A target project may have it disabled (or actively set to `0`), which
//! would silently suppress the very diagnostics gdstrict exists to enforce.
//!
//! So gdstrict does not trust the target's config: before each check it writes a
//! Godot `override.cfg` at the project root forcing its required warning set on.
//! Godot reads `override.cfg` on top of `project.godot`, so this wins regardless of
//! the project's settings without mutating `project.godot` itself. The file is
//! restored (or removed) on drop, including on early return. See [`StrictWarnings`].
//!
//! Because the override lives at the shared project root, concurrent checks of the
//! same project (the Phase 2 worker pool) would otherwise race on the file — one
//! check's drop could delete the override another check still needs. [`StrictWarnings`]
//! is therefore refcounted per project: the override is installed once while any check
//! is active and removed (or restored) only when the last one finishes.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;

mod classify;
pub mod codes;
mod config;
pub use classify::{classifier_for, detect_version, ClassifierTable, GodotVersion};
pub use config::{Action, Preset, SeverityConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// res:// path reported by Godot.
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// Best-effort warning code (e.g. "UNSAFE_METHOD_ACCESS"); None for errors or
    /// messages we do not yet map.
    pub code: Option<&'static str>,
    pub message: String,
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Locate a Godot binary: `$GODOT` env var (must be executable), else `godot` on PATH.
/// Returns `None` when neither is found, so callers can skip gracefully.
pub fn find_godot() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GODOT") {
        let pb = PathBuf::from(p);
        if is_executable(&pb) {
            return Some(pb);
        }
    }
    // Search PATH explicitly so None is a real signal, not a deferred spawn failure.
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    find_godot_in_dirs(std::env::split_paths(&path_var))
}

/// Search `dirs` for a `godot` executable; returns the first match or `None`.
/// Extracted from [`find_godot`] so tests can inject a controlled directory list
/// without touching the process environment.
fn find_godot_in_dirs(dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    for dir in dirs {
        let candidate = dir.join("godot");
        if is_executable(&candidate) {
            return Some(candidate);
        }
        // Windows executables have extensions; try common ones.
        #[cfg(windows)]
        for ext in &["exe", "bat", "cmd"] {
            let c = dir.join(format!("godot.{ext}"));
            if is_executable(&c) {
                return Some(c);
            }
        }
    }
    None
}

/// GDScript warning keys gdstrict forces on (value `1` = "Warn") regardless of the
/// target project's config, so the analyzer always emits the unsafe/untyped family.
///
/// gdstrict applies its own severity mapping downstream, so we inject these as plain
/// warnings (`1`) and decide error-vs-warn ourselves rather than letting Godot fail
/// the parse via `2` ("Error"). Version-gate this set alongside the message classifier
/// (see [`classify`]) if a future Godot renames a warning key.
const STRICT_WARNINGS: &[&str] = &[
    "untyped_declaration",
    "inferred_declaration",
    "unsafe_property_access",
    "unsafe_method_access",
    "unsafe_cast",
    "unsafe_call_argument",
    "return_value_discarded",
    "integer_division",
    "unused_variable",
    "shadowed_variable",
];

/// Render the `override.cfg` contents that force [`STRICT_WARNINGS`] on.
fn strict_override_cfg() -> String {
    let mut s = String::from(
        "; Written by gdstrict: strict warning settings injected so the GDScript\n\
         ; analyzer emits the unsafe/untyped family regardless of the target\n\
         ; project's [debug] config. Auto-removed after the check.\n\
         [debug]\n\n\
         gdscript/warnings/enable=true\n",
    );
    for w in STRICT_WARNINGS {
        s.push_str("gdscript/warnings/");
        s.push_str(w);
        s.push_str("=1\n");
    }
    s
}

/// Per-project refcount state for the shared `override.cfg`.
struct OverrideRef {
    /// Number of live [`StrictWarnings`] guards for this project.
    count: usize,
    /// Prior `override.cfg` bytes to restore, or `None` if we created it fresh.
    /// Captured once, by the first guard; restored once, by the last to drop.
    prior: Option<Vec<u8>>,
}

/// Process-wide registry mapping a (canonicalized) project dir to its [`OverrideRef`].
///
/// The override.cfg lives at the shared project root, so concurrent checks of one
/// project must coordinate: the first guard installs the file and captures the prior
/// state, later guards merely join the refcount, and only the last guard to drop
/// restores/removes the file. The mutex is held only briefly during install and drop —
/// never across the Godot subprocess run — so concurrent checks still proceed in
/// parallel; they just don't clobber each other's override.cfg.
fn override_registry() -> &'static Mutex<HashMap<PathBuf, OverrideRef>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, OverrideRef>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the registry, recovering from a poisoned mutex. A panic in another check while
/// holding the lock must not wedge every subsequent check — the map is plain data and
/// safe to keep using, so we take the inner guard rather than propagating the panic.
fn lock_registry() -> std::sync::MutexGuard<'static, HashMap<PathBuf, OverrideRef>> {
    override_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// RAII guard that installs gdstrict's [`STRICT_WARNINGS`] as a Godot `override.cfg`
/// at the project root for the duration of a check, then restores the prior state on
/// drop. Because Godot reads `override.cfg` on top of `project.godot`, this forces the
/// warning set on without trusting — or mutating — the target's config.
///
/// If an `override.cfg` already exists it is backed up in memory and rewritten on
/// drop; otherwise the file we created is removed. Drop runs on early return too, so
/// the project is left as we found it even when a Godot pass errors.
///
/// **Concurrency:** guards for the same project are refcounted through
/// [`override_registry`]. The first guard writes the file and records the prior bytes;
/// concurrent guards just increment the count and reuse the already-installed override
/// (every guard writes byte-identical strict content, so the file is always correct
/// while any guard is alive). The file is restored/removed only when the last guard for
/// that project drops, so no check can have its override deleted out from under it by
/// another check finishing first.
struct StrictWarnings {
    /// Path to the override.cfg this guard manages.
    path: PathBuf,
    /// Registry key: the canonicalized project dir (falls back to the given path).
    key: PathBuf,
}

impl StrictWarnings {
    /// Join (or start) the refcounted strict `override.cfg` for `project_dir`.
    ///
    /// The first guard for a project captures any prior file and writes the strict
    /// override; later concurrent guards reuse it. Returns once the override is in
    /// place, holding the registry lock only for the install itself.
    fn install(project_dir: &Path) -> std::io::Result<Self> {
        let path = project_dir.join("override.cfg");
        // Canonicalize so different spellings of the same project dir collapse to one
        // refcount entry; fall back to the raw path if the dir can't be canonicalized.
        let key = std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());

        let mut reg = lock_registry();
        match reg.get_mut(&key) {
            // Another check already installed the override; just join its refcount.
            // The on-disk content is identical strict config, so nothing to rewrite.
            Some(state) => {
                state.count += 1;
            }
            // First check for this project: capture prior file and write strict config.
            None => {
                let prior = match std::fs::read(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e),
                };
                std::fs::write(&path, strict_override_cfg())?;
                reg.insert(key.clone(), OverrideRef { count: 1, prior });
            }
        }
        Ok(Self { path, key })
    }
}

impl Drop for StrictWarnings {
    fn drop(&mut self) {
        // Hold the registry lock across the refcount decrement *and* the filesystem
        // restore so no concurrent install can sneak in between deciding to remove the
        // file and actually removing it.
        let mut reg = lock_registry();
        let Some(state) = reg.get_mut(&self.key) else {
            return;
        };
        state.count -= 1;
        if state.count > 0 {
            // Other checks still need the override; leave it in place.
            return;
        }
        // Last guard for this project: restore prior state, then forget the entry.
        // Best-effort restore; failures here must not mask the check's result.
        let prior = reg.remove(&self.key).and_then(|s| s.prior);
        match prior {
            Some(bytes) => {
                let _ = std::fs::write(&self.path, bytes);
            }
            None => {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

/// Run the full strict check on a single script within a project.
///
/// `project_dir` is the dir containing project.godot; `script_rel` is the script
/// path relative to it (as Godot expects after `--path`).
///
/// gdstrict's required warning set is injected via an `override.cfg` for the duration
/// of the check (see [`StrictWarnings`]), so unsafe/untyped warnings are emitted
/// regardless of what the target `project.godot` enables or disables.
pub fn check_script(
    godot: &Path,
    project_dir: &Path,
    script_rel: &str,
) -> std::io::Result<Vec<Diagnostic>> {
    // Force our warning settings on for both passes; restored when `_warnings` drops
    // (including on the `?` early returns below).
    let _warnings = StrictWarnings::install(project_dir)?;

    // Detect the binary's version once (cached per path) so the message classifier
    // uses the table that matches this Godot. `None` falls back to the newest table.
    let version = detect_version(godot);

    // Pass 1: errors (no --debug — safe).
    let errs = run(godot, project_dir, script_rel, false)?;
    let mut diags = parse_diagnostics(&errs, version);
    let has_error = diags.iter().any(|d| d.severity == Severity::Error);

    // Pass 2: warnings — only when there are no hard errors, to avoid the
    // --debug debugger crash.
    if !has_error {
        let warns = run(godot, project_dir, script_rel, true)?;
        diags.extend(parse_diagnostics(&warns, version));
    }
    Ok(diags)
}

/// Default size of the strict worker pool: `max(1, cpu - 2)`.
///
/// Each strict check spawns one or two Godot subprocesses (~0.15s each, Phase 0),
/// so on a big project a serial sweep dominates wall-clock. We leave two cores to
/// the OS / Godot itself and the calling process rather than saturating every core.
pub fn default_worker_count() -> usize {
    let cpus = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    cpus.saturating_sub(2).max(1)
}

/// Run [`check_script`] over many jobs concurrently via a bounded pool of
/// [`default_worker_count`] worker threads, returning one result per job **in input
/// order** regardless of completion order. Each job is a `(project_dir, script_rel)`
/// pair, exactly as [`check_script`] takes them.
///
/// Concurrency is safe by construction against the same project: [`StrictWarnings`]
/// refcounts the shared `override.cfg` through a process-wide registry (the onh fix),
/// so overlapping checks of one project share a single correct override and only the
/// last to finish restores it. This pool simply drives many such checks at once.
///
/// **Crash isolation:** a panic inside one job's check is caught and turned into an
/// `io::Error` for that job alone — every other job still runs and reports. (A Godot
/// subprocess that crashes does not panic the worker: `Command::output` returns `Ok`
/// with the failing status, and `check_script` reads its stderr like any other run.)
pub fn check_scripts(
    godot: &Path,
    jobs: &[(PathBuf, String)],
) -> Vec<std::io::Result<Vec<Diagnostic>>> {
    parallel_map(jobs.len(), default_worker_count(), |i| {
        let (project_dir, script_rel) = &jobs[i];
        check_script(godot, project_dir, script_rel)
    })
    .into_iter()
    // The panic payload is intentionally dropped here: callers only need to know the
    // job failed, and `check_script`'s real errors are already `io::Error`s. The full
    // payload survives in `parallel_map`'s `thread::Result` for anyone who wants it.
    .map(|r| r.unwrap_or_else(|_| Err(std::io::Error::other("strict check panicked"))))
    .collect()
}

/// Check **every** script in a project with a single engine boot per pass via a
/// throwaway GDScript harness that `load()`s each file in one process, instead of
/// spawning Godot once per file. This is the batch strategy benchmarked in
/// `docs/phase3/bench-strict-invocation.md` and chosen as the default for CI-scale
/// projects: Godot's ~0.4s engine startup dominates per-file invocation, so amortizing
/// it across the whole corpus is an order of magnitude faster than per-file.
///
/// `script_rels` are project-relative paths (e.g. `"core/foo.gd"`), exactly as
/// [`check_script`] takes them. Returns the combined diagnostics for the whole batch,
/// each carrying the `res://` file it was reported against (use [`Diagnostic::file`] to
/// demultiplex back to a source file).
///
/// ## Two passes, same crash-avoidance as [`check_script`]
///
/// 1. **Errors** — boot once, no `--debug`, `load()` every file. Surfaces
///    `SCRIPT ERROR:` parse errors safely; `--debug` is never on, so a hard error can
///    never trip the debugger into the signal-11 crash (spike .2 finding).
/// 2. **Warnings** — boot once with `--debug`, `load()` only the files that produced
///    **no** error in pass 1. Because every file in this pass parsed cleanly, the
///    debugger has nothing to break on — the same invariant per-file mode relies on,
///    just batched.
///
/// ## Tradeoff vs [`check_scripts`] (per-file pool)
///
/// Batch trades per-file process isolation for speed: a pathological file that hard-
/// crashes the engine (segfault, not a parse error) takes the whole pass down with it,
/// whereas the per-file pool loses only that one job. Pass 1 is crash-proof; the
/// residual risk lives in pass 2 and is the same class of risk per-file `--debug` runs
/// carry. Callers that need bulletproof isolation on an untrusted corpus can fall back
/// to [`check_scripts`]; for trusted CI corpora batch is the right default.
pub fn check_project_batch(
    godot: &Path,
    project_dir: &Path,
    script_rels: &[String],
) -> std::io::Result<Vec<Diagnostic>> {
    if script_rels.is_empty() {
        return Ok(Vec::new());
    }

    // Force the strict warning set on for both passes; restored on drop (incl. `?`).
    let _warnings = StrictWarnings::install(project_dir)?;

    // Pass 1: errors over the whole corpus, no --debug (safe).
    let err_stderr = run_harness(godot, project_dir, script_rels, false)?;
    // Merge-resolution: 6zk.5 added a version param to parse_diagnostics; batch mode
    // does no version detection, so pass None (fallback classifier). See follow-up issue.
    let mut diags = parse_diagnostics(&err_stderr, None);

    // Files that produced a hard error must be excluded from the --debug pass, or the
    // debugger break would crash it. Errors carry the `res://` file they hit.
    let errored: std::collections::HashSet<&str> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.file.as_str())
        .collect();
    let clean: Vec<String> = script_rels
        .iter()
        .filter(|rel| !errored.contains(format!("res://{rel}").as_str()))
        .cloned()
        .collect();

    // Pass 2: warnings over the error-free files, --debug (safe — nothing to break on).
    if !clean.is_empty() {
        let warn_stderr = run_harness(godot, project_dir, &clean, true)?;
        diags.extend(parse_diagnostics(&warn_stderr, None));
    }
    Ok(diags)
}

/// Run a generated batch harness over `script_rels` in one engine boot and return its
/// stderr. The harness `load()`s each `res://<rel>` so the analyzer compiles (and, under
/// `debug`, warns on) every file in a single process. The harness is written to a unique
/// temp path and removed before returning — it lives outside the project tree, so it
/// never pollutes the source corpus (Godot accepts an absolute `--script` alongside
/// `--path <project>`).
fn run_harness(
    godot: &Path,
    project_dir: &Path,
    script_rels: &[String],
    debug: bool,
) -> std::io::Result<String> {
    let harness_path = unique_harness_path();
    std::fs::write(&harness_path, batch_harness_source(script_rels))?;
    // Remove the harness no matter how `run` returns.
    let _cleanup = HarnessFile(&harness_path);

    let mut cmd = Command::new(godot);
    cmd.arg("--headless")
        .arg("--path")
        .arg(project_dir)
        .arg("--script")
        .arg(&harness_path);
    if debug {
        cmd.arg("--debug");
    }
    capture_stderr(&mut cmd)
}

/// Run a built command and return its stderr as a lossy `String`. Godot writes all
/// analyzer diagnostics to stderr, so every invocation path funnels through here.
fn capture_stderr(cmd: &mut Command) -> std::io::Result<String> {
    let out = cmd.output()?;
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

/// RAII cleanup for the throwaway harness file: best-effort remove on drop.
struct HarnessFile<'a>(&'a Path);

impl Drop for HarnessFile<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// A process-unique path for a throwaway harness `.gd` in the OS temp dir. Uniqueness is
/// `pid` + a monotonic counter so concurrent/back-to-back batches never collide (no
/// `Math.random`/clock needed — those are unavailable in some sandboxes anyway).
fn unique_harness_path() -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("gdstrict-batch-{pid}-{n}.gd"))
}

/// Generate the fully-typed `SceneTree` harness that `load()`s each script. It is typed
/// to the hilt on purpose: an untyped harness would itself trip `UNTYPED_DECLARATION`
/// and pollute the diagnostics with warnings about the harness instead of the corpus.
fn batch_harness_source(script_rels: &[String]) -> String {
    let mut s = String::from(
        "# Generated by gdstrict (check_project_batch). Loads every project script in\n\
         # one engine boot so the analyzer compiles/warns on all of them at once.\n\
         extends SceneTree\n\n\
         func _init() -> void:\n\
         \tvar files: PackedStringArray = PackedStringArray([\n",
    );
    for rel in script_rels {
        // Godot string literals: escape backslash and quote so odd paths stay valid.
        let escaped = rel.replace('\\', "\\\\").replace('"', "\\\"");
        s.push_str("\t\t\"res://");
        s.push_str(&escaped);
        s.push_str("\",\n");
    }
    s.push_str(
        "\t])\n\
         \tfor f: String in files:\n\
         \t\tvar res: Resource = load(f)\n\
         \t\tif res == null:\n\
         \t\t\tpush_error(\"gdstrict: failed to load \" + f)\n\
         \tquit()\n",
    );
    s
}

/// Apply `work(i)` for every `i in 0..n` across a bounded pool of `workers` threads,
/// returning the results in index order: `result[i]` is the outcome of `work(i)`.
///
/// A panic in one `work(i)` is caught (its slot becomes `Err(payload)`) and never
/// aborts the other items — this is the crash-isolation guarantee the strict pool
/// relies on. `workers` is clamped to `1..=max(1, n)`, so passing a huge worker count
/// or `n == 0` is always safe.
///
/// Generic and Godot-free so the pool's order-preservation, bound, and panic isolation
/// can be unit-tested directly (see tests).
fn parallel_map<T, F>(n: usize, workers: usize, work: F) -> Vec<thread::Result<T>>
where
    F: Fn(usize) -> T + Sync,
    T: Send,
{
    if n == 0 {
        return Vec::new();
    }
    let workers = workers.clamp(1, n);
    // Shared cursor hands out the next job index; each slot is written exactly once
    // (by the worker that claimed that index), so distinct slots never contend.
    let cursor = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<thread::Result<T>>>> = (0..n).map(|_| Mutex::new(None)).collect();

    thread::scope(|scope| {
        for _ in 0..workers {
            let cursor = &cursor;
            let slots = &slots;
            let work = &work;
            scope.spawn(move || loop {
                let i = cursor.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let outcome = catch_unwind(AssertUnwindSafe(|| work(i)));
                *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
            });
        }
    });

    slots
        .into_iter()
        .map(|m| {
            m.into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .expect("every slot is filled before the scope joins")
        })
        .collect()
}

fn run(godot: &Path, project_dir: &Path, script_rel: &str, debug: bool) -> std::io::Result<String> {
    let mut cmd = Command::new(godot);
    cmd.arg("--headless")
        .arg("--check-only")
        .arg("--path")
        .arg(project_dir)
        .arg("--script")
        .arg(script_rel);
    if debug {
        cmd.arg("--debug");
    }
    capture_stderr(&mut cmd)
}

/// Parse Godot stderr into diagnostics. Pairs each `WARNING:` / `*ERROR:` header
/// line with the following `at: ... (res://file:line)` locator line.
///
/// `version` is the detected Godot version (from [`detect_version`]); it selects the
/// version-gated message classifier so warning codes match the running release. Pass
/// `None` when the version is unknown — the newest classifier table is used.
pub fn parse_diagnostics(stderr: &str, version: Option<GodotVersion>) -> Vec<Diagnostic> {
    let table = classifier_for(version);
    let lines: Vec<&str> = stderr.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start();
        let (severity, message) = if let Some(rest) = line.strip_prefix("WARNING: ") {
            (Severity::Warning, rest.to_string())
        } else if let Some(rest) = line.strip_prefix("SCRIPT ERROR: ") {
            (Severity::Error, rest.to_string())
        } else if let Some(rest) = line.strip_prefix("ERROR: ") {
            (Severity::Error, rest.to_string())
        } else {
            i += 1;
            continue;
        };
        // Look for the locator on the next non-empty line.
        let mut file = String::new();
        let mut ln = 0usize;
        if let Some(next) = lines.get(i + 1) {
            if let Some((f, l)) = parse_locator(next) {
                file = f;
                ln = l;
            }
        }
        out.push(Diagnostic {
            severity,
            code: if severity == Severity::Warning {
                table.classify(&message)
            } else {
                None
            },
            file,
            line: ln,
            message,
        });
        i += 1;
    }
    out
}

/// Parse `   at: GDScript::reload (res://path.gd:42)` → ("res://path.gd", 42).
///
/// Handles three forms:
///   file:line          — last segment is the line
///   file:line:column   — second-to-last is line, last is column (column is discarded)
///   res://file:line    — the `://` in the scheme is not confused for a line separator
///
/// Strategy: split on `:`, walk from the right. If the last segment is numeric
/// AND the second-to-last is also numeric, treat them as line:column and take the
/// second-to-last as the line. Otherwise the last numeric segment is the line.
fn parse_locator(line: &str) -> Option<(String, usize)> {
    let line = line.trim_start();
    if !line.starts_with("at:") {
        return None;
    }
    let open = line.rfind('(')?;
    let close = line.rfind(')')?;
    let inner = &line[open + 1..close];

    let parts: Vec<&str> = inner.split(':').collect();
    let n = parts.len();
    if n < 2 {
        return None;
    }

    // Last segment must be a number (line or column).
    let last: usize = parts[n - 1].trim().parse().ok()?;

    // If the second-to-last is also a number, this is file:line:column.
    if n >= 3 {
        if let Ok(line_num) = parts[n - 2].trim().parse::<usize>() {
            let path = parts[..n - 2].join(":");
            return Some((path, line_num));
        }
    }

    // Simple file:line (path may itself contain ':' e.g. res://).
    let path = parts[..n - 1].join(":");
    Some((path, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_locator unit tests ---

    #[test]
    fn locator_plain_file_line() {
        assert_eq!(
            parse_locator("     at: GDScript::reload (path/to/file.gd:10)"),
            Some(("path/to/file.gd".into(), 10))
        );
    }

    #[test]
    fn locator_file_line_column() {
        // Column is discarded; line must be 42, not 5.
        assert_eq!(
            parse_locator("     at: GDScript::reload (path/to/file.gd:42:5)"),
            Some(("path/to/file.gd".into(), 42))
        );
    }

    #[test]
    fn locator_res_scheme_line() {
        // res:// contains '://' — path must not be split on the scheme colon.
        assert_eq!(
            parse_locator("     at: GDScript::reload (res://path.gd:7)"),
            Some(("res://path.gd".into(), 7))
        );
    }

    #[test]
    fn locator_res_scheme_line_column() {
        assert_eq!(
            parse_locator("     at: GDScript::reload (res://path.gd:42:5)"),
            Some(("res://path.gd".into(), 42))
        );
    }

    #[test]
    fn locator_no_line_returns_none() {
        assert_eq!(
            parse_locator("     at: GDScript::reload (res://path.gd)"),
            None
        );
    }

    #[test]
    fn locator_not_at_line_returns_none() {
        assert_eq!(parse_locator("WARNING: something happened"), None);
    }

    // --- parse_diagnostics integration tests ---

    #[test]
    fn parses_warning_block() {
        let stderr = "WARNING: Variable \"thing\" has no static type.\n     at: GDScript::reload (res://unsafe.gd:7)\n";
        let d = parse_diagnostics(stderr, Some(GodotVersion::new(4, 6, 2)));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].file, "res://unsafe.gd");
        assert_eq!(d[0].line, 7);
        assert_eq!(d[0].code, Some(crate::codes::UNTYPED_DECLARATION));
    }

    #[test]
    fn parses_error_block() {
        let stderr = "SCRIPT ERROR: Parse Error: Function \"x()\" not found in base self.\n          at: GDScript::reload (res://broken.gd:4)\n";
        let d = parse_diagnostics(stderr, None);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].line, 4);
    }

    #[test]
    fn override_cfg_lists_strict_warnings_as_warn() {
        let cfg = strict_override_cfg();
        assert!(cfg.contains("[debug]"));
        assert!(cfg.contains("gdscript/warnings/enable=true"));
        // Every strict key is forced to 1 ("Warn"), never 0/2.
        for w in STRICT_WARNINGS {
            assert!(
                cfg.contains(&format!("gdscript/warnings/{w}=1")),
                "missing strict warning {w} in:\n{cfg}"
            );
        }
        assert!(!cfg.contains("=0"), "no strict warning should be disabled");
    }

    /// The guard creates override.cfg when absent and removes it on drop, leaving
    /// the project exactly as it was. No Godot needed.
    #[test]
    fn guard_creates_and_removes_override() {
        let dir = scratch_dir("create");
        let override_path = dir.join("override.cfg");
        assert!(!override_path.exists());
        {
            let _g = StrictWarnings::install(&dir).unwrap();
            let written = std::fs::read_to_string(&override_path).unwrap();
            assert!(written.contains("gdscript/warnings/unsafe_method_access=1"));
        }
        assert!(
            !override_path.exists(),
            "override.cfg must be removed on drop"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The guard restores a pre-existing override.cfg byte-for-byte on drop.
    #[test]
    fn guard_restores_existing_override() {
        let dir = scratch_dir("restore");
        let override_path = dir.join("override.cfg");
        let original = b"[display]\nwindow/size/mode=3\n";
        std::fs::write(&override_path, original).unwrap();
        {
            let _g = StrictWarnings::install(&dir).unwrap();
            // While installed, our settings are in place, not the user's.
            let live = std::fs::read_to_string(&override_path).unwrap();
            assert!(live.contains("gdscript/warnings/untyped_declaration=1"));
        }
        let restored = std::fs::read(&override_path).unwrap();
        assert_eq!(restored, original, "prior override.cfg must be restored");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Refcount race regression: while one guard is still alive, a second guard's
    /// drop must NOT remove the shared override.cfg. With the old non-refcounted guard
    /// the first drop deleted the file out from under the surviving check, silently
    /// suppressing its warnings. No Godot needed.
    #[test]
    fn concurrent_guards_keep_override_until_last_drop() {
        let dir = scratch_dir("concurrent");
        let override_path = dir.join("override.cfg");
        assert!(!override_path.exists());

        let g1 = StrictWarnings::install(&dir).unwrap();
        assert!(override_path.exists(), "first guard installs the override");
        {
            let g2 = StrictWarnings::install(&dir).unwrap();
            assert!(override_path.exists());
            // First guard drops while the second is still active.
            drop(g1);
            assert!(
                override_path.exists(),
                "override.cfg must survive while another check still holds a guard"
            );
            let live = std::fs::read_to_string(&override_path).unwrap();
            assert!(
                live.contains("gdscript/warnings/unsafe_method_access=1"),
                "surviving guard must still see strict content, got:\n{live}"
            );
            drop(g2);
        }
        assert!(
            !override_path.exists(),
            "override.cfg is removed only after the last guard drops"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Stress the refcount under real threads: many guards install/hold/drop against one
    /// project concurrently, and no thread may ever observe the override missing or with
    /// non-strict content while it holds a guard. This is the property the worker pool
    /// relies on — warnings are never lost to a racing drop. No Godot needed.
    #[test]
    fn many_threads_never_observe_missing_override() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = Arc::new(scratch_dir("threads"));
        let lost = Arc::new(AtomicBool::new(false));

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let dir = Arc::clone(&dir);
                let lost = Arc::clone(&lost);
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let _g = StrictWarnings::install(&dir).unwrap();
                        // While we hold a guard the override must be present & strict.
                        match std::fs::read_to_string(dir.join("override.cfg")) {
                            Ok(s) if s.contains("gdscript/warnings/unsafe_method_access=1") => {}
                            _ => lost.store(true, Ordering::SeqCst),
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(
            !lost.load(Ordering::SeqCst),
            "a concurrent guard saw the override.cfg missing or non-strict"
        );
        assert!(
            !dir.join("override.cfg").exists(),
            "override.cfg must be gone once every guard has dropped"
        );
        std::fs::remove_dir_all(&*dir).ok();
    }

    /// Drift guard: the hostile inject fixture must explicitly set *every*
    /// [`STRICT_WARNINGS`] key to `0`, so the live injection test genuinely proves we
    /// override a project that disables the full set (not just Godot defaults). Fails
    /// if the const and the fixture drift apart. No Godot needed.
    #[test]
    fn inject_fixture_disables_every_strict_warning() {
        let project_godot = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/strict_inject_project/project.godot"
        );
        let text = std::fs::read_to_string(project_godot).unwrap();
        for w in STRICT_WARNINGS {
            assert!(
                text.contains(&format!("gdscript/warnings/{w}=0")),
                "inject fixture must disable strict warning `{w}` (set it to 0); \
                 STRICT_WARNINGS and the fixture have drifted"
            );
        }
    }

    // --- batch harness generation (no Godot) ---

    /// The generated harness must be fully typed (no untyped/inferred decls that would
    /// emit their own warnings) and must list every script as a `res://` literal.
    #[test]
    fn batch_harness_is_typed_and_lists_every_script() {
        let rels = vec!["core/foo.gd".to_string(), "ui/bar.gd".to_string()];
        let src = batch_harness_source(&rels);
        // Typed throughout — these exact typed decls are what keep the harness from
        // tripping UNTYPED_DECLARATION/INFERRED_DECLARATION on itself.
        assert!(src.contains("var files: PackedStringArray"));
        assert!(src.contains("for f: String in files"));
        assert!(src.contains("var res: Resource = load(f)"));
        // Every script is referenced as a res:// literal.
        assert!(src.contains("\"res://core/foo.gd\""));
        assert!(src.contains("\"res://ui/bar.gd\""));
    }

    /// Paths with quotes/backslashes must be escaped so the harness stays valid GDScript.
    #[test]
    fn batch_harness_escapes_paths() {
        let rels = vec!["weird\\\"name.gd".to_string()];
        let src = batch_harness_source(&rels);
        assert!(
            src.contains("\"res://weird\\\\\\\"name.gd\""),
            "backslash and quote must be escaped, got:\n{src}"
        );
    }

    /// Distinct calls hand out distinct harness paths so concurrent batches never
    /// clobber each other's harness file.
    #[test]
    fn unique_harness_paths_differ() {
        let a = unique_harness_path();
        let b = unique_harness_path();
        assert_ne!(a, b, "harness paths must be unique across calls");
        assert!(a.to_string_lossy().ends_with(".gd"));
    }

    /// Live batch check over the warning fixture: one engine boot must surface the same
    /// warning family per-file mode does. Skipped when no Godot is available.
    #[test]
    fn live_batch_surfaces_warnings() {
        let Some(godot) = runnable_godot() else {
            eprintln!("no runnable godot; skipping");
            return;
        };
        let project = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/strict_project"
        ));
        let diags = check_project_batch(&godot, project, &["unsafe.gd".to_string()]).unwrap();
        let codes: Vec<_> = diags.iter().filter_map(|d| d.code).collect();
        assert!(
            codes.contains(&crate::codes::UNSAFE_METHOD_ACCESS),
            "batch lost warnings: {diags:#?}"
        );
        assert!(codes.contains(&crate::codes::UNTYPED_DECLARATION));
        assert!(codes.contains(&crate::codes::INTEGER_DIVISION));
        // Note: no override.cfg-leak assertion here — this shares the `strict_project`
        // fixture with `live_strict_extraction`, whose still-live refcounted guard can
        // legitimately keep override.cfg present while this test runs. The no-leak
        // property is covered on private dirs by `guard_creates_and_removes_override`
        // and `concurrent_check_script_does_not_lose_warnings`.
    }

    /// An empty corpus is a no-op: no harness, no Godot boot, empty result.
    #[test]
    fn batch_empty_corpus_is_noop() {
        // No Godot needed — empties short-circuit before any boot. Use a path that need
        // not exist; install() is never reached.
        let godot = PathBuf::from("/nonexistent/godot");
        let project = PathBuf::from("/nonexistent/project");
        let out = check_project_batch(&godot, &project, &[]).unwrap();
        assert!(out.is_empty());
    }

    // --- find_godot_in_dirs unit tests (no env-var mutation, thread-safe) ---

    #[test]
    fn find_godot_absent_when_dir_has_no_godot() {
        // Genuinely empty dir — no godot file at all. (The non-executable-file case
        // is platform-dependent and covered by `find_godot_skips_non_executable_file`.)
        let dir = scratch_dir("find-absent");
        assert_eq!(find_godot_in_dirs([dir.clone()]), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_godot_absent_when_path_is_empty() {
        // An empty iterator must yield None, not fall back to a bare "godot".
        assert_eq!(find_godot_in_dirs(std::iter::empty()), None);
    }

    #[test]
    fn find_godot_present_when_executable_exists() {
        let (dir, fake) = make_fake_godot("find-present", true);
        assert_eq!(find_godot_in_dirs([dir.clone()]), Some(fake));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_godot_skips_non_executable_file() {
        // `_fake` is consumed only by the Windows branch; underscore keeps the Unix
        // build clippy-clean (unused otherwise) while staying usable there.
        let (dir, _fake) = make_fake_godot("find-non-exec", false);
        // On Unix, mode 0o644 has no execute bit — must not be found.
        #[cfg(unix)]
        assert_eq!(find_godot_in_dirs([dir.clone()]), None);
        // On Windows every existing file is treated as executable.
        #[cfg(not(unix))]
        assert_eq!(find_godot_in_dirs([dir.clone()]), Some(_fake));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Creates a scratch dir with a `godot` file inside it. When `executable`
    /// is true the file gets the execute bit set (Unix) so `is_executable` accepts it.
    /// Returns `(dir, godot_path)`.
    fn make_fake_godot(tag: &str, executable: bool) -> (PathBuf, PathBuf) {
        let dir = scratch_dir(tag);
        let fake = dir.join("godot");
        std::fs::write(&fake, b"#!/bin/sh\necho godot 4.0\n").unwrap();
        // `executable` only matters on Unix; on Windows existence implies executable.
        #[cfg(not(unix))]
        let _ = executable;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        (dir, fake)
    }

    /// A Godot binary that is present *and* actually runnable, or `None` to skip a
    /// live test. Folds the `find_godot()` + `--version` checks the Godot-gated tests
    /// share so they don't drift apart.
    fn runnable_godot() -> Option<PathBuf> {
        let godot = find_godot()?;
        if Command::new(&godot).arg("--version").output().is_err() {
            return None;
        }
        Some(godot)
    }

    /// Per-test scratch dir under the target dir (no external temp-dir crate).
    fn scratch_dir(tag: &str) -> PathBuf {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("strict-inject-test-{tag}"));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// Phase 0 spike .2 — live extraction against a real Godot binary.
    /// Skipped when `find_godot()` returns None (godot not on PATH and $GODOT unset).
    #[test]
    fn live_strict_extraction() {
        let Some(godot) = find_godot() else {
            eprintln!("no godot on PATH; skipping");
            return;
        };
        let project = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/strict_project");
        let diags = check_script(&godot, Path::new(project), "unsafe.gd").unwrap();
        let codes: Vec<_> = diags.iter().filter_map(|d| d.code).collect();
        assert!(
            codes.contains(&crate::codes::UNSAFE_METHOD_ACCESS),
            "expected UNSAFE_METHOD_ACCESS, got {diags:#?}"
        );
        assert!(codes.contains(&crate::codes::UNTYPED_DECLARATION));
        assert!(codes.contains(&crate::codes::INTEGER_DIVISION));
    }

    /// Acceptance (clean half): the fully-typed fixture project must yield **no**
    /// strict-family warning codes, so `check` can exit 0 on it. Mirrors
    /// `live_strict_extraction` (the unsafe half) at the analyzer layer and pins the
    /// clean fixture against regressions. Skipped when no Godot is available.
    #[test]
    fn live_clean_project_has_no_strict_warnings() {
        let Some(godot) = find_godot() else {
            eprintln!("no godot on PATH; skipping");
            return;
        };
        let project = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/strict_clean_project"
        );
        let diags = check_script(&godot, Path::new(project), "clean.gd").unwrap();
        // No hard parse errors, and none of the strict-family codes the preset
        // promotes (the only thing that could fail the gate).
        let offending: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error || d.code.is_some())
            .collect();
        assert!(
            offending.is_empty(),
            "clean fixture must emit no strict diagnostics, got {offending:#?}"
        );
    }

    /// Injection acceptance test: the fixture project's project.godot *actively
    /// disables* the unsafe/untyped family, yet gdstrict must still surface those
    /// warnings because it injects its own override.cfg. Proves we do not trust the
    /// target's config. Skipped when no Godot is available.
    #[test]
    fn live_injection_overrides_hostile_project() {
        let Some(godot) = runnable_godot() else {
            eprintln!("no runnable godot; skipping");
            return;
        };
        let project = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/strict_inject_project"
        );
        let override_path = Path::new(project).join("override.cfg");

        let diags = check_script(&godot, Path::new(project), "unsafe.gd").unwrap();
        let codes: Vec<_> = diags.iter().filter_map(|d| d.code).collect();
        // These are all set to 0 in the fixture's project.godot.
        assert!(
            codes.contains(&crate::codes::UNSAFE_METHOD_ACCESS),
            "injection failed: project disables this warning, got {diags:#?}"
        );
        assert!(codes.contains(&crate::codes::UNTYPED_DECLARATION));
        assert!(codes.contains(&crate::codes::INTEGER_DIVISION));

        // The guard must leave no trace in the source tree after the check.
        assert!(
            !override_path.exists(),
            "override.cfg leaked into the project after check_script"
        );
    }

    /// Concurrency acceptance: many `check_script` calls against the *same* project at
    /// once must each still surface the injected warnings — none may lose them to a
    /// racing override.cfg drop. The fixture's project.godot disables the unsafe family,
    /// so a check that ran without a live override would come back clean. Skipped when
    /// no Godot is available.
    #[test]
    fn concurrent_check_script_does_not_lose_warnings() {
        let Some(godot) = runnable_godot() else {
            eprintln!("no runnable godot; skipping");
            return;
        };
        // Copy the hostile fixture into a private scratch project so this test owns its
        // own override.cfg / registry key — otherwise it would share the shared project
        // root (and refcount) with `live_injection_overrides_hostile_project` when the
        // suite runs them in parallel, and each test's post-check "no leak" assertion
        // would race the other's still-live guard.
        let fixture = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/strict_inject_project"
        ));
        let project = scratch_dir("concurrent-check");
        std::fs::copy(fixture.join("project.godot"), project.join("project.godot")).unwrap();
        std::fs::copy(fixture.join("unsafe.gd"), project.join("unsafe.gd")).unwrap();

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let godot = godot.clone();
                let project = project.clone();
                std::thread::spawn(move || check_script(&godot, &project, "unsafe.gd").unwrap())
            })
            .collect();
        for h in handles {
            let diags = h.join().unwrap();
            let codes: Vec<_> = diags.iter().filter_map(|d| d.code).collect();
            assert!(
                codes.contains(&crate::codes::UNSAFE_METHOD_ACCESS),
                "a concurrent check lost its injected warnings: {diags:#?}"
            );
        }

        assert!(
            !project.join("override.cfg").exists(),
            "override.cfg leaked after concurrent checks"
        );
        std::fs::remove_dir_all(&project).ok();
    }

    // --- bounded worker pool (parallel_map / check_scripts) ---

    /// `default_worker_count` never returns 0 (it would mean "no workers" → no work
    /// ever runs) and stays within the machine's parallelism.
    #[test]
    fn worker_count_is_at_least_one() {
        let n = default_worker_count();
        assert!(n >= 1, "worker count must be >= 1, got {n}");
        let cpus = thread::available_parallelism()
            .map(|c| c.get())
            .unwrap_or(1);
        assert!(
            n <= cpus.max(1),
            "worker count {n} exceeds cpu count {cpus}"
        );
    }

    /// Results come back in input order, `result[i] == work(i)`, no matter which
    /// worker finished first.
    #[test]
    fn parallel_map_preserves_input_order() {
        let out = parallel_map(6, 3, |i| i * i);
        let got: Vec<usize> = out.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(got, vec![0, 1, 4, 9, 16, 25]);
    }

    /// An empty job list does no work and returns an empty vec (no panics, no threads).
    #[test]
    fn parallel_map_empty_is_noop() {
        let out = parallel_map(0, 4, |_i: usize| -> usize { panic!("must not run") });
        assert!(out.is_empty());
    }

    /// A panic in one job is isolated: that slot is `Err`, every other slot still ran
    /// and holds its correct value. This is the crash-isolation guarantee.
    #[test]
    fn parallel_map_isolates_panics() {
        let out = parallel_map(5, 2, |i| {
            assert_ne!(i, 2, "boom on index 2");
            i
        });
        assert!(out[2].is_err(), "panicking job must surface as Err");
        for (i, slot) in out.iter().enumerate() {
            if i == 2 {
                continue;
            }
            assert_eq!(
                *slot.as_ref().unwrap(),
                i,
                "non-panicking job {i} must still produce its value"
            );
        }
    }

    /// The pool honors its bound (never more than `workers` items in flight) while
    /// still running more than one at a time. A brief sleep widens the overlap window
    /// so the observed concurrency is reliably > 1 without being flaky.
    #[test]
    fn parallel_map_is_bounded_and_concurrent() {
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;

        const WORKERS: usize = 4;
        let inflight = AtomicUsize::new(0);
        let max_seen = AtomicUsize::new(0);

        let out = parallel_map(16, WORKERS, |i| {
            let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(now, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(10));
            inflight.fetch_sub(1, Ordering::SeqCst);
            i
        });

        assert_eq!(out.len(), 16);
        let peak = max_seen.load(Ordering::SeqCst);
        assert!(
            peak <= WORKERS,
            "pool exceeded its bound: {peak} in flight, cap {WORKERS}"
        );
        assert!(
            peak >= 2,
            "pool never ran two jobs at once (peak {peak}); not actually concurrent"
        );
    }

    /// Live end-to-end: drive `check_scripts` over every script in the acceptance
    /// corpus — many files sharing ONE project root — and confirm each job comes back
    /// (in order) and the shared override.cfg is gone afterward. Exercises the bounded
    /// pool on top of the refcounted override contract. Skipped when no Godot is found.
    #[test]
    fn check_scripts_runs_corpus_concurrently() {
        let Some(godot) = runnable_godot() else {
            eprintln!("no runnable godot; skipping");
            return;
        };
        let project = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/acceptance"
        ));
        let mut rels: Vec<String> = std::fs::read_dir(project.join("stagehand_core"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| format!("stagehand_core/{}", e.file_name().to_string_lossy()))
            .filter(|n| n.ends_with(".gd"))
            .collect();
        rels.sort();
        assert!(
            rels.len() >= 2,
            "need multiple scripts to exercise concurrency"
        );

        let jobs: Vec<(PathBuf, String)> = rels
            .iter()
            .map(|r| (project.to_path_buf(), r.clone()))
            .collect();
        let results = check_scripts(&godot, &jobs);

        assert_eq!(results.len(), jobs.len(), "one result per job, in order");
        for (job, res) in jobs.iter().zip(&results) {
            assert!(
                res.is_ok(),
                "strict check failed for {}: {:?}",
                job.1,
                res.as_ref().err()
            );
        }
        assert!(
            !project.join("override.cfg").exists(),
            "override.cfg leaked after concurrent check_scripts"
        );
    }
}
