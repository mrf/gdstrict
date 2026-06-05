# Spike .2 — strict-mode warning extraction from Godot headless

**Issue:** `godot-linter-phase0-spikes-75m.2`
**Verdict: GO.** Godot's analyzer warnings are extractable and parseable, and the
known crash is fully avoidable.

## The core finding: a two-pass strategy

| Flag combo | Emits | Exit | Crash? |
|---|---|---|---|
| `--check-only` (no `--debug`) | `SCRIPT ERROR:` / `ERROR:` lines | 1 on error | **No** |
| `--check-only --debug` | `WARNING:` lines (untyped/unsafe/etc.) | 0 | **Yes, if a hard parse error is present** (debugger break → signal 11) |
| `--check-only` (no `--debug`) | **nothing** for warnings | 0 | — |

So neither flag alone is enough, and `--debug` alone is dangerous. The working approach:

1. **Pass 1 — errors:** `godot --headless --check-only --path P --script F` (no `--debug`).
   Safe; gives errors + exit code.
2. **Pass 2 — warnings:** only when pass 1 found **no errors**, re-run with `--debug`.
   Because no hard error is present, the debugger never breaks → no crash.

This converts the research-flagged crash caveat from a risk into a non-issue.

## Output format (Godot 4.6.2)
Two lines per diagnostic — header + locator:
```text
WARNING: Variable "thing" has no static type.
     at: GDScript::reload (res://unsafe.gd:7)
SCRIPT ERROR: Parse Error: Function "x()" not found in base self.
          at: GDScript::reload (res://broken.gd:4)
```
Parsed into `{ severity, file, line, code, message }`. Diagnostics go to **stderr**.

## Warning codes are NOT in the output
The CLI prints human messages, not codes like `UNSAFE_METHOD_ACCESS`. We recover the
code with a message→code classifier (`classify_warning`). Templates are stable per
Godot version → **version-gate the classifier** (already a planned mitigation). The LSP
path would give codes directly but is heavier; deferred.

## Verified extraction (8/8 on the fixture)
`fixtures/strict_project/unsafe.gd` → UNTYPED_DECLARATION, UNSAFE_METHOD_ACCESS,
UNSAFE_PROPERTY_ACCESS (untyped var), UNSAFE_CAST, RETURN_VALUE_DISCARDED,
INTEGER_DIVISION all surfaced and classified. Warnings must be enabled in
`project.godot` under `[debug] gdscript/warnings/*` (off by default for the unsafe family).

## Latency
~**0.15 s** per invocation (warm). Worst case ~0.3 s/file (both passes). Serial over a
big project is slow but **embarrassingly parallel** — a bounded worker pool handles it.
Follow-up (Phase 3): benchmark a single whole-project invocation vs. per-file, and a
throwaway `.gd` batch-validation harness.

## Caveats / follow-ups
- Only line numbers (no columns) in CLI output.
- Warnings require explicit `project.godot` enablement; `gdstrict` should inject/override
  the warning settings it cares about rather than trusting the project's config.
- Pin Godot version in CI; the message templates and flag behavior are the unstable
  contract (proposal #12548 would replace this with a real API).

## Reusable artifacts produced
- `crates/gdstrict-strict` — `check_script` (two-pass), `parse_diagnostics`,
  `classify_warning`, `find_godot`, `Diagnostic`/`Severity`.
- `fixtures/strict_project/` — project.godot with warnings enabled + `unsafe.gd`.
