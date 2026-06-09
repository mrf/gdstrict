# Phase 3 — strict-check invocation strategy benchmark

**Issue:** `godot-linter-phase3-strict-6zk.2` (follow-up to
[spike .2 — strict extraction](../phase0/spike-2-strict-extraction.md), which flagged
"one script per invocation = slow on big projects; benchmark vs a single whole-project
load and a throwaway `.gd` batch harness in Phase 3").

**Verdict: use the batch harness ([`check_project_batch`]) as the default for CI-scale
projects.** It is **5–40× faster** than per-file invocation and the gap widens with
project size, because it amortizes Godot's ~0.4s engine boot across the whole corpus
instead of paying it per file.

## The three candidates

| Strategy | Code | Engine boots | Viable? |
|---|---|---|---|
| **per-file** | `check_script` / `check_scripts` | up to `2 × files` | yes (current) |
| **whole-project `--check-only`** | — | n/a | **no — not a real Godot mode** |
| **batch harness** | `check_project_batch` | `2` total | yes (**chosen**) |

### Why "whole-project `--check-only`" is not viable

`--check-only` is documented (and behaves) as *per-script*: `godot --help` says
"Only parse for errors and quit **(use with `--script`)**." Run without `--script`,
`godot --headless --check-only --path P` does **not** check the project — it ignores the
flag and tries to *run* the main scene:

```text
$ godot --headless --check-only --path <project>      # no --script
# …hangs running the project, or exits 1 with "no main scene" — never emits
# per-file diagnostics.
```

The benchmark probes this directly (`probe_native_whole_project`) and records the
outcome per corpus: either "did not terminate (runs project)" or a bare non-zero exit
with no diagnostics. There is **no native single-invocation whole-project check** in
Godot 4.6 — the only way to validate a whole project in one engine boot is a harness
that `load()`s each script itself, i.e. the batch strategy. (Proposal
[#12548](https://github.com/godotengine/godot-proposals/issues/12548) would add a real
`validate()` API; until then, batch is the single-boot realization.)

## The batch harness

[`check_project_batch`] generates a fully-typed throwaway `SceneTree` script that
`load()`s every project file in one process, then runs the **same two-pass strategy**
spike .2 established for per-file mode — just batched:

1. **Errors** — one boot, **no** `--debug`, `load()` every file. Surfaces `SCRIPT ERROR:`
   lines safely; with `--debug` off, a hard parse error can never trip the debugger into
   the signal-11 crash.
2. **Warnings** — one boot **with** `--debug`, `load()` only the files that produced no
   error in pass 1. Every file in this pass parsed cleanly, so the debugger has nothing
   to break on — the exact invariant per-file mode relies on.

Warnings come back with the identical `at: GDScript::reload (res://file.gd:LINE)` locator
format the existing [`parse_diagnostics`] already handles, so attribution back to the
source file is free. The harness is written to a unique temp path (not the project tree)
and removed on drop, and the strict-warning `override.cfg` is installed exactly as in
per-file mode (see [`check_script`]).

## Results

Measured by `crates/gdstrict-strict/src/bin/strict-bench.rs` on **Godot 4.6.2.stable**,
AMD Ryzen 7 3700X (8 cores), Linux/WSL2. `per-file-pool` uses the default bounded pool
(`max(1, cpus − 2)` = 6 workers). The real corpus is `fixtures/acceptance/stagehand_core`
(~2k LOC); synthetic corpora are `N` self-contained typed files (each provoking a couple
of analyzer warnings) to trace the size curve toward CI scale.

| corpus | files | per-file-serial | per-file-pool | batch | speedup (serial→batch) |
|---|---:|---:|---:|---:|---:|
| acceptance (real) | 11 | 3.76s | 1.11s | 0.66s | 5.7× |
| synthetic ×10 | 10 | 4.67s | 1.20s | 0.39s | 12.0× |
| synthetic ×25 | 25 | 12.52s | 2.93s | 0.73s | 17.1× |
| synthetic ×50 | 50 | 24.15s | 5.20s | 0.59s | 41.1× |

The shape is the whole story:

- **per-file-serial** is `O(files)` engine boots — linear, and the slowest by far.
- **per-file-pool** divides that by the worker count — a real win (~4–5×) but still
  `O(files / workers)` boots, so it keeps climbing with project size.
- **batch** is **flat at ~0.6s** regardless of file count: two boots total. At 50 files
  it is already 41× faster than serial and ~9× faster than the pool, and the advantage
  only grows.

(Absolute numbers are machine-dependent; the ratios and the flat-vs-linear shape are the
portable conclusion. Re-run with `GODOT=… cargo run -p gdstrict-strict --bin strict-bench
--release`.)

## Tradeoff: batch gives up per-file process isolation

Per-file mode runs each file in its own Godot process, so a file that *hard-crashes* the
engine (a segfault from a pathological input — not a parse error) loses only that one
job; the pool keeps going. Batch shares one process per pass, so such a crash would take
the whole pass down.

This risk is bounded:

- **Pass 1 is crash-proof** (`--debug` off — the spike .2 crash cannot occur).
- **Pass 2** only loads files that already parsed cleanly, so it carries the *same* class
  of residual risk a per-file `--debug` run carries — no more.
- A caller that needs bulletproof isolation on an *untrusted* corpus can fall back to
  [`check_scripts`]; a batch that crashes can be retried per-file to localize the
  offender.

For trusted CI corpora — the target use case — the order-of-magnitude speedup is the
right default, with per-file pooling kept as the isolation-preserving fallback.

## Reproduce

```text
# Skips cleanly (exit 0) when no Godot is found, so it is CI-safe.
GODOT=/path/to/godot cargo run -p gdstrict-strict --bin strict-bench --release
```

The strategies themselves are unit- and Godot-integration-tested in
`crates/gdstrict-strict/src/lib.rs` (`live_batch_surfaces_warnings`,
`batch_harness_is_typed_and_lists_every_script`, …).

[`check_project_batch`]: ../../crates/gdstrict-strict/src/lib.rs
[`check_script`]: ../../crates/gdstrict-strict/src/lib.rs
[`check_scripts`]: ../../crates/gdstrict-strict/src/lib.rs
[`parse_diagnostics`]: ../../crates/gdstrict-strict/src/lib.rs
