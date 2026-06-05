# gdstrict — the "black for Godot" formatter + strict-mode checker

> A fast, opinionated, zero-config GDScript formatter **and** a strict-typing
> enforcement layer that makes "GDScript strict mode" a real, CI-enforceable thing.
> Rust + tree-sitter. Wraps Godot's own analyzer for true type checking.
>
> Working name: **`gdstrict`** (provisional).

---

## 1. Context — why this exists

GDScript has no `black`/`ruff`/`mypy`. The existing pieces are partial and don't
compose into a strict-mode story:

| Tool | Role | Gap |
|---|---|---|
| **gdformat** (Scony `gdtoolkit`, Python/lark) | de-facto formatter | Python-slow; comment-handling + idempotency bugs; only `--line-length` configurable; syntax support has lagged Godot releases before — e.g. typed `Dictionary[K,V]` broke in issue #363 |
| **gdlint** (gdtoolkit) | style linter | purely syntactic/AST — **zero** type or semantic analysis |
| **GDQuest/GDScript-formatter** (Rust + tree-sitter + Topiary) | modern fast formatter + style linter | fast and useful today, but its Topiary-based formatter deliberately **doesn't auto-wrap** on max line length; style linting exists, but no type/semantic strict-mode layer |
| **Godot `GDScriptAnalyzer`** | the *only* real type checker (`UNTYPED_DECLARATION`, `INFERRED_DECLARATION`, `UNSAFE_*`) | locked in the engine; `--check-only` doesn't surface warnings cleanly; awkward in CI; no batch/programmatic API (proposal #12548 still open) |

**The gap:** a single standalone tool that (1) formats deterministically like `black`
with automatic line wrapping and (2) enforces strict typing by failing the build on
untyped/unsafe code. Nothing ships that combined workflow today.

**Locked design decisions:**

- **Scope:** formatter **and** strict checker (full vision).
- **Strict engine:** **wrap Godot's own analyzer** — accurate, far less work than
  reimplementing the type system.
- **Stack:** **Rust + tree-sitter**, one Rust CLI binary. Format/lint can be fully
  standalone; strict mode shells out to Godot.
- **Posture:** **fresh standalone tool**, new brand.

**Outcome:** `gdstrict format .` rewrites every `.gd` to one canonical style (idempotent,
zero-config). `gdstrict check .` returns non-zero if any file violates formatting *or*
strict-typing rules — droppable into a pre-commit hook and CI. Format/lint are standalone;
strict mode requires a Godot binary.

---

## 2. Architecture

Cargo workspace → one distributable Rust CLI binary. Strict-mode checks require a
separately installed Godot binary.

```
gdstrict/
  Cargo.toml                  # workspace
  crates/
    gdstrict-cli/             # clap args, config discovery, exit codes, output rendering
    gdstrict-syntax/          # tree-sitter-gdscript wrapper: source → CST, error recovery, trivia
    gdstrict-format/          # CST → canonical text (the "black" engine)
    gdstrict-lint/            # syntactic style rules on the CST (deferred/non-core)
    gdstrict-strict/          # Godot-analyzer driver: run godot headless, parse diagnostics
  fixtures/                   # golden in/out pairs + idempotency corpus
```

**Pipeline:** `source → gdstrict-syntax (parse) → { format | lint | strict }`.
Format never needs Godot (fast path). Strict shells out. Lint is useful but not the
initial differentiator because GDQuest already covers style linting.

### 2.1 `gdstrict-syntax` — parser
- Depends on **`tree-sitter` + `tree-sitter-gdscript`** (the grammar GDQuest relies on).
- Must cover Godot 4.4/4.5 syntax: typed `Dictionary[K,V]`, `Array[T]`, `@annotations`,
  lambdas, `await`/`signal`, match patterns, `static` vars/functions.
- **Comments & significant whitespace are first-class trivia around the CST** — comment
  handling is exactly where formatters break, so it gets golden tests from day one.
  This includes blank lines, doc comments, inline comments, annotations, backslash
  continuations, multiline strings, and indentation-sensitive blocks.
- **Risk:** if the grammar lags Godot (the same typed-dict gap that bit gdformat), we
  upstream grammar fixes or vendor a patched copy. De-risked in Phase 0.

### 2.2 `gdstrict-format` — the "black" engine
- **Contract copied from black:** deterministic, **idempotent**, near-zero config. The
  only knob is `line-length` (default **100**, per Godot style guide).
- **Auto-wraps on line length** — the key thing GDQuest's formatter refuses to do, and
  the whole point of "black for Godot." Long call chains, argument lists, and
  array/dictionary literals wrap to one element per line with a **magic trailing comma**
  (a trailing comma forces the expanded form, black/ruff-style).
- **Idempotency is a hard invariant:** `format(format(x)) == format(x)`, gated in CI by
  double-formatting every fixture. This is gdformat's recurring bug class.
- **Implementation:** hand-written **Wadler/Prettier-style document IR** (`Group`,
  `Indent`, `Line`, `SoftLine`, `Text`) + a width-aware renderer. Chosen over Topiary
  because auto-wrapping needs real layout control and Topiary's query model is weak there.

### 2.3 `gdstrict-lint` — syntactic style rules (deferred)
Reimplement useful style rules on our CST only after the formatter and strict extractor
prove out. GDQuest already ships style linting, so this is not the first wedge.

- **Naming:** snake_case funcs/vars/args, PascalCase classes/enums, CONSTANT_CASE consts,
  signal naming.
- **Dead/redundant code:** `unused-argument`, `unnecessary-pass`,
  `expression-not-assigned`, `no-else-return`, `no-elif-return`, `comparison-with-itself`,
  `duplicated-load`.
- **Structure:** `class-definitions-order`, `private-method-call`, `max-line-length`,
  `function-arguments-number`, `max-public-methods`.

### 2.4 `gdstrict-strict` — the strict-mode layer (the novel part)
Drives Godot headlessly to extract **real** analyzer diagnostics:

```
godot --headless --check-only --debug --path <project> --script <file.gd>
```

The Godot docs only promise `--check-only` parses for errors; warning output is not a
stable public API. Phase 0 must prove that warnings are extractable on pinned Godot
versions before this becomes a committed implementation path.

Parses stderr lines like
`(UNSAFE_METHOD_ACCESS): The method "is_empty()" is not present on the inferred type
"Variant" ...` into structured `{ file, line, col, code, message, severity }`.

A `gdstrict.toml` strict profile maps each Godot warning → `error | warn | off`, giving
the **project-wide warnings-as-errors** enforcement Godot lacks out of the box. The
`strict` preset turns on `UNTYPED_DECLARATION`, `INFERRED_DECLARATION`, the `UNSAFE_*`
family, `RETURN_VALUE_DISCARDED`, and similar.

**Caveats engineered around (grounded in research):**

| Caveat | Mitigation |
|---|---|
| `--check-only --debug` can crash on scripts referencing autoload/addon singletons | Run with `--path` at project root so autoloads resolve; catch per-file crashes and report them as tool diagnostics without masking other files |
| One script per invocation = slow on big projects | Bounded concurrent worker pool; benchmark vs. a single throwaway `.gd` "harness" that batch-validates via engine internals; pick fastest in Phase 2 |
| No stable programmatic API yet (proposal #12548) | Parse human-readable stderr; **version-gate** the parser and test against pinned Godot releases |
| Godot binary must be discoverable | `--godot <path>` flag → `GODOT` env var → PATH. If strict mode is enabled and Godot is absent, `check` exits non-zero as a tool/configuration error. Strict is skipped only when the user explicitly passes `--no-strict` or config disables it |

### 2.5 CLI surface
```
gdstrict format [paths...]        # rewrite in place
gdstrict format --check [paths]   # exit 1 if any file would change (CI / pre-commit)
gdstrict format --diff [paths]    # print unified diff, no writes
gdstrict lint   [paths...]        # syntactic rules only (deferred; no Godot)
gdstrict check  [paths...]        # format-check + strict (+lint once implemented)
```
Flags: `--line-length`, `--godot <path>`, `--config <file>`, `--strict` (preset),
`--quiet`, `--no-strict`.
Config discovery: nearest `gdstrict.toml` walking up from each file (black/ruff style).
Respects `.gdignore` / gitignore.

---

## 3. Milestones & acceptance criteria

### Phase 0 — Spikes (de-risk before committing real work)
1. **Grammar currency:** round-trip a corpus of real Godot 4.5 files (typed dicts,
   annotations, lambdas, match) through `tree-sitter-gdscript`. *Accept:* zero parse
   errors on the corpus, or a documented list of grammar gaps + upstream-fix plan.
2. **Lossless trivia model:** prove the parser wrapper can preserve and reattach comments,
   blank lines, annotations, backslash continuations, multiline strings, and significant
   indentation while rewriting a small set of expressions. *Accept:* golden fixtures cover
   those cases and double-formatting is stable.
3. **Strict extraction:** prove `godot --headless --check-only --debug` emits parseable
   `UNSAFE_*`/`UNTYPED_DECLARATION` lines on a sample project. *Accept:* a Rust function
   that returns structured diagnostics from a known-bad file; per-file latency measured;
   missing-Godot and crash paths have explicit non-zero tool diagnostics.
4. **Formatter prototype:** one wrapping case (long call chain) through a throwaway
   doc-IR. *Accept:* confirms doc-IR over Topiary; idempotent on that case.

### Phase 1 — Formatter MVP *(ships value alone — the first product wedge)*
- `gdstrict-syntax` + `gdstrict-format` for the common subset.
- `format`, `format --check`, `format --diff`.
- Golden fixtures + idempotency gate in CI.
- *Accept:* formats a real Godot game repo; output still parses via tree-sitter **and**
  `godot --check-only`; double-format is a no-op.

### Phase 2 — Strict mode
- `gdstrict-strict` Godot driver, stderr diagnostic parser, warning→severity map,
  concurrency, `check` aggregation, explicit missing-Godot behavior.
- *Accept:* fixture project with untyped/unsafe code → `check` exits non-zero with exact
  warning codes; clean typed project → exit 0; no-Godot path exits non-zero unless strict
  is explicitly disabled.

### Phase 3 — Syntactic linter (optional once core wedge works)
- `gdstrict-lint` rule set + `lint` command + `gdstrict.toml` config.
- *Accept:* per-rule unit tests (positive + negative) mirroring gdlint/GDQuest behavior,
  with a clear reason for every rule that diverges.

### Phase 4 — Distribution / DX
- pre-commit hook, GitHub Action, prebuilt binaries (cargo-dist) for Linux/macOS/Windows,
  VS Code task integration, docs site.

---

## 4. Critical files to create
- `Cargo.toml` (workspace) + `crates/*/Cargo.toml` & `src/lib.rs` per crate.
- `crates/gdstrict-cli/src/main.rs` — command dispatch, exit codes, output.
- `crates/gdstrict-syntax/src/lib.rs` — tree-sitter wrapper, CST + trivia model.
- `crates/gdstrict-format/src/doc.rs` — document IR + width-aware renderer (formatter core).
- `crates/gdstrict-strict/src/godot.rs` — headless invocation + stderr diagnostic parser.
- `crates/gdstrict-lint/src/lib.rs` — syntactic style rules on the CST (deferred).
- `fixtures/format/*.{in,out}.gd` — golden tests; `fixtures/idempotency/` — double-format gate.

## 5. Reuse / prior art (don't reinvent)
- `tree-sitter-gdscript` grammar — don't hand-write a parser.
- gdlint and GDQuest's rule catalogs — references for `gdstrict-lint` if/when we add it.
- black/ruff's documented contract (line-length, magic trailing comma, idempotency) —
  spec for our style; proven UX.
- Godot's `@GDScript` warning taxonomy — the strict-mode rule IDs.

## 6. Verification
- **Formatter:** golden `*.in.gd → *.out.gd`; CI double-format idempotency test; format a
  large real Godot repo and assert output still parses (tree-sitter + `godot --check-only`).
- **Linter:** optional/deferred per-rule unit tests, positive/negative.
- **Strict:** untyped/unsafe fixture project → non-zero exit with exact codes; clean typed
  project → exit 0; pin Godot version in CI; missing Godot is a non-zero tool/config
  error unless strict is explicitly disabled.
- **End-to-end:** install as a pre-commit hook on a sample repo; confirm it blocks a commit
  introducing an untyped var and passes once typed.

## 7. Open risks
1. **tree-sitter-gdscript currency** — a major technical risk; if the grammar lags Godot we
   inherit the same class of syntax gaps that has bitten other tools. Mitigated by Phase 0
   spike + upstreaming fixes.
2. **Lossless formatting over trivia** — comments, blank lines, doc comments, multiline
   strings, and indentation-sensitive code are where formatter quality is won or lost.
   Mitigated by a dedicated Phase 0 spike and fixture-first development.
3. **Godot stderr is an unstable contract** until #12548 lands — version-gate the parser,
   test against pinned releases.
4. **Strict mode needs a Godot binary in CI** — heavier than a pure static tool, but the
   accepted tradeoff for real type checking; format still works without it.
5. **Scope creep from style linting** — GDQuest already covers much of this space. Keep
   lint optional until formatter auto-wrap and strict extraction are proven.

---

## Appendix — sources
- gdtoolkit — https://github.com/Scony/godot-gdscript-toolkit
- gdlint rules — https://github.com/Scony/godot-gdscript-toolkit/wiki/3.-Linter
- GDQuest formatter — https://github.com/GDQuest/GDScript-formatter
- typed-dict syntax lag example (issue #363) — https://github.com/Scony/godot-gdscript-toolkit/issues/363
- Godot command line `--check-only` docs — https://docs.godotengine.org/en/stable/tutorials/editor/command_line_tutorial.html
- Godot static typing — https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html
- warnings via CLI — https://forum.godotengine.org/t/getting-gdscript-warnings-through-the-command-line/124343
- expose validate() (proposal #12548) — https://github.com/godotengine/godot-proposals/issues/12548
