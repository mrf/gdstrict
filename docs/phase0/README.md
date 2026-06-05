# Phase 0 — de-risking spikes

Epic: `godot-linter-phase0-spikes-75m`. Goal: prove the three load-bearing
assumptions in [PLAN.md](../../PLAN.md) before committing to Phase 1.

| Spike | Question | Verdict | Notes |
|---|---|---|---|
| [.1 Grammar currency](spike-1-grammar-currency.md) | Does `tree-sitter-gdscript` parse modern Godot 4.x? | **GO** | 5/5 fixtures clean incl. typed `Dictionary[K,V]` that broke gdformat |
| [.2 Strict extraction](spike-2-strict-extraction.md) | Can we extract analyzer warnings headlessly, without crashing? | **GO** | Two-pass strategy (errors then warnings) sidesteps the `--debug` crash; ~0.15s/run |
| [.3 Formatter doc-IR](spike-3-formatter-docir.md) | Can a doc-IR auto-wrap on line length (vs Topiary)? | **GO** | Width-driven flat/break + magic trailing comma working |

**All three green → proceed to Phase 1 (formatter MVP).**

## Crates stood up
- `gdstrict-syntax` — tree-sitter wrapper (`parse`, `defects`, `parses_clean`).
- `gdstrict-strict` — Godot headless driver (`check_script`, `parse_diagnostics`, `classify_warning`).
- `gdstrict-format` — layout engine (`doc::render`, `doc::call`, magic trailing comma).

## Run everything
```
cargo test                  # all crates; strict live test auto-skips without Godot
GODOT=/path/to/godot cargo test -p gdstrict-strict   # include live extraction
```
