# Spike .3 — doc-IR formatter prototype with auto-wrapping

**Issue:** `godot-linter-phase0-spikes-75m.3`
**Verdict: GO.** Use a hand-written Wadler/Prettier document IR, not Topiary.

## What was built
`crates/gdstrict-format/src/doc.rs` — a minimal but real layout engine:
- `Doc`: `Text`, `Concat`, `Line`, `SoftLine`, `HardLine`, `Indent`, `Group`, `IfBreak`.
- `render(doc, width)` — the standard Prettier print loop: a work stack of
  `(indent, mode, doc)` plus a `fits` lookahead that decides each `Group` flat-vs-break.
- `trailing_comma()` = `IfBreak{ break_: ",", flat: "" }` — the magic trailing comma.
- `call(callee, args)` — builds the wrapping test case.

## The decision: doc-IR over Topiary
Topiary (GDQuest's choice) is query-driven and **deliberately does not auto-wrap on max
line length** — the single feature that defines "black for Godot." The doc-IR makes
wrapping a first-class, width-driven decision. Proven by `boundary_respects_width`: the
*same* `Doc` renders

```
foo(alpha, beta, gamma)            # width 100 — fits, stays flat
foo(                               # width 20 — doesn't fit, breaks
    alpha,
    beta,
    gamma,
)
```

That is the core capability GDQuest's formatter lacks, working in ~150 lines.

## Magic trailing comma
When a group breaks, `IfBreak` emits the trailing comma; when flat, it emits nothing.
Verified by `long_call_wraps_one_per_line_with_trailing_comma`. (Phase 1 also wires the
*input* trailing comma to *force* breaking, black/ruff-style.)

## Idempotency
`render` is a pure function of `(doc, width)` → repeated renders are byte-identical
(`render_is_deterministic`). Full source-level idempotency (`format(format(x)) == x`)
additionally requires the CST→Doc lowering, which is Phase 1; the IR imposes no obstacle.

## Tests (5/5 green)
`short_call_stays_flat`, `empty_call`, `long_call_wraps_one_per_line_with_trailing_comma`,
`boundary_respects_width`, `render_is_deterministic`.

## Follow-ups for Phase 1
- Lower the tree-sitter CST → `Doc` (the bulk of the formatter).
- Comment/trivia attachment into the IR (the gdformat failure mode).
- Force-break on input magic trailing comma; blank-line normalization; tab vs space
  (GDScript is tab-indented by convention — make `INDENT` emit a tab in GDScript mode).
