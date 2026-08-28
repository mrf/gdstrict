# gdstrict

> Format, lint, and type-check your GDScript — in one fast binary, gated in CI.

[![CI](https://github.com/mrf/gdstrict/actions/workflows/ci.yml/badge.svg)](https://github.com/mrf/gdstrict/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

```sh
gdstrict check .
```

One command. Your code gets formatted consistently, your style rules get enforced, your untyped and unsafe declarations fail the build, and your gnarliest functions get flagged before they grow another branch.

## What you get

**A deterministic formatter that wraps long lines.** Point it at a file and it comes back canonical — long call chains, argument lists, and array/dictionary literals expanded to one element per line, with a magic trailing comma to force the expanded form. There is one knob, `line-length` (default 100, matching Godot's style guide). No style debates in code review. `format(format(x)) == format(x)` is a hard invariant, gated on every fixture in CI.

**21 lint rules, no Godot required.** Naming conventions, dead code, redundant branches, class member ordering, size limits. Runs in milliseconds on a whole project — fast enough for a pre-commit hook or format-on-save.

**Real strict typing, because it's Godot doing the checking.** gdstrict drives Godot's own `GDScriptAnalyzer` headlessly and turns its warnings into a pass/fail gate. `UNTYPED_DECLARATION`, `INFERRED_DECLARATION`, the whole `UNSAFE_*` family — mapped to `error`, `warn`, or `off` per code in your config. The type checking is exactly as accurate as the engine, because it *is* the engine.

**Cyclomatic complexity, per function.** Nothing else in the GDScript ecosystem measures this. `gdstrict complexity` reports the McCabe complexity of every function in your project, and the `max-complexity` lint rule turns it into a gate. Counting follows ruff's `C901`, so the numbers mean the same thing they do in your Python and TypeScript repos.

## Install

**Linux and macOS:**

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/mrf/gdstrict/releases/latest/download/gdstrict-installer.sh \
  | sh
```

**Windows:**

```pwsh
irm https://github.com/mrf/gdstrict/releases/latest/download/gdstrict-installer.ps1 | iex
```

Or grab a `.tar.gz` / `.zip` from the [Releases page](https://github.com/mrf/gdstrict/releases), or build from source with a [Rust toolchain](https://rustup.rs):

```sh
git clone https://github.com/mrf/gdstrict && cd gdstrict
cargo build --release   # -> target/release/gdstrict
```

Only the strict-typing pass needs a Godot binary. Formatting, linting, and complexity never do.

## Commands

### `format`

```sh
gdstrict format .                        # rewrite in place
gdstrict format --check .                # CI mode: write nothing, exit 1 if anything would change
gdstrict format --diff src/player.gd     # show a unified diff
gdstrict format --line-length 120 .      # override for this run
```

Directories are walked recursively, respecting `.gdignore` and gitignore rules so vendored and generated code stays untouched.

**Exit codes:** `0` clean · `1` a file would change (under `--check`), or an error.

### `lint`

```sh
gdstrict lint .
gdstrict lint --config path/to/gdstrict.toml src/
```

The full naming-convention and code-quality rule set, run against the CST. No Godot needed. See [Lint rules](#lint-rules).

**Exit codes:** `0` no findings · `1` at least one finding, or an error.

### `complexity`

```sh
gdstrict complexity .                    # file:line:column: name  complexity
gdstrict complexity --min 10 src/        # only the functions worth looking at
gdstrict complexity --format json src/   # machine-readable
```

A **report, not a gate** — it exits 0 no matter how bad the numbers are. The gate is the [`max-complexity`](#structure) lint rule.

Complexity starts at 1 and gains 1 for each `if`, `elif`, `for`, `while`, `match` arm, and nested lambda. `else` adds nothing (it's the default path, not a decision), and neither do ternaries or `and`/`or` — the model is statement-level, matching ruff's `C901` / PyCQA `mccabe`. A lambda's branches count toward its enclosing function; methods of an inner `class Inner` are reported as `Inner.method`.

Each JSON record carries the function's line span, which makes [CRAP scores](https://testing.googleblog.com/2011/02/this-code-is-crap.html) (`complexity² × (1 − coverage)³ + complexity`) computable — join `line`..`end_line` against your coverage report's line hits. gdstrict does not read coverage itself.

```json
[
  {
    "file": "src/player.gd",
    "name": "_physics_process",
    "line": 42,
    "column": 0,
    "end_line": 78,
    "complexity": 13
  }
]
```

**Exit codes:** `0` report produced · `1` a file or config could not be read.

### `check`

The aggregate CI gate: format-check, then lint, then strict typing.

```sh
gdstrict check .                         # everything
gdstrict check --godot /path/to/godot .  # point at a specific Godot binary
gdstrict check --no-strict .             # skip the type pass (no Godot needed)
gdstrict check --quiet .                 # summary and exit code only
```

Only `error`-level strict findings fail the gate; `warn`-level findings are printed and pass. Godot discovery order: `--godot <path>` → `$GODOT` → `PATH`.

**Exit codes:** `0` clean · `1` a violation · `2` operational error (bad path, invalid config, strict enabled with no Godot found).

## Configuration

One `gdstrict.toml` configures everything. Every key is optional — omit the file entirely and you get sensible defaults.

```toml
# Maximum line length before the formatter wraps (default: 100).
line-length = 100

# Strict-mode severity preset (only "strict" is known; the default).
preset = "strict"

[lint]
# Disable individual rules. All rules are on by default.
function-name-case = false

# Threshold rules take an integer, which sets their limit and keeps them on:
# max-complexity, max-line-length, function-arguments-number, max-public-methods.
max-complexity = 15

[warnings]
# Per-code severity for the strict pass: "error" | "warn" | "off".
# Overrides beat the preset; unlisted codes get the preset's default.
INTEGER_DIVISION = "off"
RETURN_VALUE_DISCARDED = "warn"
```

Unknown top-level keys are rejected, so a typo like `line_length` fails loudly instead of silently doing nothing.

**Discovery** (highest precedence first): `--line-length` on the CLI → `--config <file>` → the nearest `gdstrict.toml` walking up from each input file (the `black` / `ruff` model) → built-in defaults.

### What the `strict` preset promotes to errors

| Code | What it catches |
|---|---|
| `UNTYPED_DECLARATION` | Variables, parameters, or return types with no static type annotation |
| `INFERRED_DECLARATION` | Variables inferred from a `Variant` value — typed as `Variant`, not the real type |
| `UNSAFE_CAST` | An explicit cast that may fail at runtime |
| `UNSAFE_METHOD_ACCESS` | Calling a method not present on the inferred (Variant) type |
| `UNSAFE_PROPERTY_ACCESS` | Accessing a property not present on the inferred (Variant) type |
| `UNSAFE_CALL_ARGUMENT` | Passing a Variant where a typed argument is expected |
| `RETURN_VALUE_DISCARDED` | Ignoring a function's return value |

Every other Godot warning code defaults to `warn` — surfaced, but not fatal. Promote, demote, or silence any of them with a `[warnings]` override.

## Lint rules

All enabled by default; disable individually via `[lint]`.

### Naming conventions

| Rule | What it checks |
|---|---|
| `function-name-case` | Function names must be `snake_case` (leading `_` for private is fine) |
| `variable-name-case` | Variable names must be `snake_case`, including locals |
| `parameter-name-case` | Function and signal parameter names must be `snake_case` |
| `constant-name-case` | Constant names must be `SCREAMING_SNAKE_CASE` |
| `signal-name-case` | Signal names must be `snake_case` |
| `class-name-case` | Class and inner class names must be `PascalCase` |
| `enum-name-case` | Enum type names must be `PascalCase` |
| `enum-value-case` | Enum member names must be `SCREAMING_SNAKE_CASE` |

### Dead and redundant code

| Rule | What it checks |
|---|---|
| `unused-argument` | Arguments never referenced in the body (prefix `_` to silence) |
| `unnecessary-pass` | A `pass` in a body that has other statements |
| `expression-not-assigned` | An expression statement whose result is discarded (calls and `await` exempt) |
| `no-else-return` | An `else` after an `if` body that always returns |
| `no-elif-return` | An `elif` after an `if` body that always returns |
| `comparison-with-itself` | A comparison with identical operands (e.g. `x == x`) |
| `duplicated-load` | A `load()`/`preload()` for a path that already appeared in the file |

### Structure

| Rule | Default limit | What it checks |
|---|---|---|
| `class-definitions-order` | — | Godot's canonical member order: annotations → `class_name` → `extends` → signals → enums → constants → exported vars → public vars → private vars → onready vars → methods |
| `private-method-call` | — | Calling a private method (leading `_`) on another object |
| `max-line-length` | 100 | Lines longer than `line-length` |
| `function-arguments-number` | 10 | Functions with more parameters than the limit |
| `max-public-methods` | 20 | Classes with more public methods than the limit |
| `max-complexity` | 10 | Functions whose cyclomatic complexity exceeds the limit |

## Using it in CI

### GitHub Action

```yaml
# .github/workflows/gdstrict.yml
name: GDScript quality
on: [push, pull_request]

jobs:
  gdstrict:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: mrf/gdstrict@main
```

That runs `format --check` and `lint` on the repo root. Add the strict-typing pass by installing Godot:

```yaml
      - uses: mrf/gdstrict@main
        with:
          install-godot: 'true'
          godot-version: '4.6.2'   # pin to the version your project targets
          run-check: 'true'
```

**Pin the Godot version.** Its warning output is not a stable API, and gdstrict version-gates its diagnostic parser against tested releases.

| Input | Default | Description |
|---|---|---|
| `version` | `main` | Git ref (branch, tag, SHA) of gdstrict to install |
| `install-method` | `cargo-install` | Only `cargo-install` is supported today |
| `install-godot` | `false` | Install a headless Godot binary (Linux runners only). Required for `run-check` |
| `godot-version` | `4.6.2` | Godot release to install |
| `run-format-check` | `true` | Run `gdstrict format --check` |
| `run-lint` | `true` | Run `gdstrict lint` |
| `run-check` | `false` | Run `gdstrict check`. Requires `install-godot: true` |
| `working-directory` | `.` | Directory to run in. Must be a Godot project root |

Output: `gdstrict-version` — the installed version string.

### pre-commit

```yaml
repos:
  - repo: https://github.com/mrf/gdstrict
    rev: v0.1.0  # pin to a release tag
    hooks:
      - id: gdstrict-format   # fast, no Godot needed — good for every project
      - id: gdstrict-check    # strict typing; needs Godot on PATH
```

If Godot isn't on `PATH`, point the check hook at it:

```yaml
      - id: gdstrict-check
        env:
          GDSTRICT_GODOT: /path/to/godot
```

Both hooks build from source via `cargo install` on first use, then reuse the cached binary.

## Editor integration

### VS Code

1. Install [Run on Save](https://marketplace.visualstudio.com/items?itemName=emeraldwalk.RunOnSave).
2. Copy `docs/editors/vscode/settings.json` from this repo into your project's `.vscode/`.
3. Point the `cmd` at your `gdstrict` binary.

Every `.gd` save now runs `gdstrict format` in the background. The [full guide](docs/editors/vscode.md) covers the check task and other options.

## How it's built

A Cargo workspace producing one binary. The pipeline is `source → parse → { format | lint | strict }`.

```
crates/
  gdstrict-cli/      # args, config discovery, exit codes, output rendering
  gdstrict-syntax/   # tree-sitter-gdscript: source to CST, error recovery, trivia
  gdstrict-format/   # CST to canonical text: document IR + width-aware renderer
  gdstrict-lint/     # syntactic style rules over the CST
  gdstrict-strict/   # headless Godot driver: run the analyzer, parse diagnostics
fixtures/            # golden in/out pairs + idempotency corpus
```

The formatter uses a hand-written Wadler/Prettier-style document IR (`Group`, `Indent`, `Line`, `SoftLine`, `Text`) feeding a width-aware renderer — chosen over a query-based engine because auto-wrapping needs real layout control. Comments, blank lines, doc comments, annotations, backslash continuations, and multiline strings are all treated as significant trivia, with golden tests covering each.

Design rationale and milestones live in [PLAN.md](PLAN.md).

## Contributing

```sh
cargo test --workspace                                   # full suite, incl. the idempotency gate
cargo fmt --all --check                                  # formatting gate
cargo clippy --workspace --all-targets -- -D warnings    # lint gate
```

CI runs all three on Linux and builds/tests on Linux, macOS, and Windows. See [.github/workflows/ci.yml](.github/workflows/ci.yml).

## License

MIT. See [LICENSE](LICENSE).
