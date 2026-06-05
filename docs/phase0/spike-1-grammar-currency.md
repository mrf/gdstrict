# Spike .1 — tree-sitter-gdscript grammar currency

**Issue:** `godot-linter-phase0-spikes-75m.1`
**Verdict: GO.** The grammar is current enough to build on.

## Setup
- Crate: `tree-sitter-gdscript = "6.1.0"` (PrestonKnopp), `tree-sitter = "0.24.7"`.
- Exposes `LANGUAGE: LanguageFn`; load via `parser.set_language(&LANGUAGE.into())`.
- Tested against syntax through **Godot 4.6.2** (the version installed locally,
  newer than the 4.5 target).

## Method
`crates/gdstrict-syntax` parses each fixture in `fixtures/grammar/` and walks the
CST for `ERROR` / `MISSING` nodes (`grammar_currency` test). A fixture passes only
with zero defects.

## Result: 5/5 fixtures clean
| Fixture | Exercises | Result |
|---|---|---|
| `typed_collections.gd` | `Dictionary[K,V]`, `Array[T]`, nested `Array[Array[int]]`, typed loop vars | clean |
| `annotations.gd` | `@tool`, `@export_*`, `@onready`, `@warning_ignore`, `%Label`, `$Node` | clean |
| `lambdas_signals_await.gd` | typed/untyped lambdas, `signal` with typed args, `.connect`, `await` | clean |
| `match_and_patterns.gd` | `match`, `when` guards, array/dict destructuring patterns, bindings | clean |
| `static_and_misc.gd` | `static var/func`, `@abstract`, `const Array[Color]`, `\` continuation, `"""` strings, ternary, `as`/`is` | clean |

## Key finding
Typed `Dictionary[String, Tween]` — the construct that **broke gdformat (issue #363)** —
parses into genuine structure (`type (subscript (identifier) arguments: ...)`), not
error-recovered text. Nested generics nest correctly. The tree-sitter grammar is
materially more current than gdtoolkit's lark grammar, which validates the stack choice.

## Caveats / follow-ups
- "Zero ERROR/MISSING" is the bar for a spike; it does not prove every node has the
  *ideal* shape for formatting. Edge cases will surface when the formatter consumes the
  CST — handle them as fixtures then.
- Pin the grammar version; re-run `grammar_currency` when bumping Godot or the grammar.
- Larger real-world corpus (a full game repo) should be run before Phase 1 ships, but is
  not needed to greenlight the approach.

## Reusable artifacts produced
- `crates/gdstrict-syntax` — `parse`, `defects`, `parses_clean`, `ParseDefect`.
- `examples/sexp.rs` — `cargo run -p gdstrict-syntax --example sexp -- <file.gd>`.
