# gdstrict

> A fast, opinionated, zero-config GDScript formatter and a strict-typing enforcement layer that makes "GDScript strict mode" a real, CI-enforceable thing. Built in Rust on tree-sitter, it wraps Godot's own analyzer for true type checking.

[![CI](https://github.com/mrf/gdstrict/actions/workflows/ci.yml/badge.svg)](https://github.com/mrf/gdstrict/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Why this exists

Most mature language ecosystems have a settled answer to "how should this code look, and is it actually well typed." Python has `black`, `ruff`, and `mypy`. GDScript has excellent tools too, and gdstrict stands on their shoulders. Each was built for a specific job, and no single one covers the full format-plus-strict-typing workflow:

- **gdformat** (from Scony's `gdtoolkit`, written in Python on lark) is the de-facto formatter and the reason most GDScript gets formatted at all. It focuses on a single line-length knob; being a separate reimplementation of the grammar, it occasionally trails new Godot syntax (typed `Dictionary[K, V]` is one example that took a release to catch up).
- **gdlint** (also gdtoolkit) is a mature style linter. By design it works at the syntactic level, so type-aware and semantic checks are out of its scope.
- **GDQuest's GDScript-formatter** (Rust, tree-sitter, Topiary) is fast and a pleasure to use. Its Topiary-based engine intentionally leaves long lines as written rather than auto-wrapping them, and it stays focused on formatting and style rather than type-aware strict mode.
- **Godot's own `GDScriptAnalyzer`** is the real type checker in the ecosystem, and gdstrict's strict mode is built directly on top of it. It knows `UNTYPED_DECLARATION`, `INFERRED_DECLARATION`, and the whole `UNSAFE_*` family. It lives inside the engine, though, and surfacing its warnings cleanly in a batch CI run takes some driving, which is exactly the gap gdstrict's strict-mode layer fills.

The opportunity is a single standalone tool that does three things well:

1. Formats deterministically, like `black`, with automatic line wrapping.
2. Enforces naming conventions and code quality rules without needing Godot.
3. Enforces strict typing by failing the build on untyped or unsafe code.

## Philosophy

gdstrict is built on a few firm opinions.

**Formatting is not a matter of taste.** The formatter is deterministic, idempotent, and close to zero-config, the same contract that made `black` win. There is exactly one knob, `line-length` (default 100, matching Godot's style guide). You do not configure style. You adopt it, and you stop arguing about it in code review.

**Idempotency is a hard invariant, not a nice-to-have.** `format(format(x))` must equal `format(x)` for every input. This is gated in CI by double-formatting every fixture. Idempotency is genuinely hard to get right, so gdstrict treats the invariant as load-bearing from day one.

**Auto-wrapping is the whole point.** Long call chains, argument lists, and array or dictionary literals wrap to one element per line, with a magic trailing comma that forces the expanded form (the `black` and `ruff` behavior). Auto-wrapping is the capability the other fast formatters intentionally leave out, and providing it is a core reason gdstrict exists.

**Strict mode should mean something real.** Reimplementing GDScript's type system would be a multi-year trap. Instead, gdstrict drives the engine's own analyzer headlessly and parses its diagnostics. The type checking is exactly as accurate as Godot itself, because it is Godot. A `gdstrict.toml` profile maps each warning to `error`, `warn`, or `off`, which gives you the project-wide warnings-as-errors enforcement the engine does not offer out of the box.

**Comments and whitespace are first-class.** Comment handling is exactly where formatters break, so the parser treats comments, blank lines, doc comments, annotations, backslash continuations, and multiline strings as significant trivia, with golden tests covering them.

## Status

| Capability | Crate | Status |
|---|---|---|
| Formatter (parse, document IR, width-aware wrapping) | `gdstrict-format` | Working. Exposed via `format`. |
| Syntactic lint rules (naming conventions, dead code, structure) | `gdstrict-lint` | Working. Exposed via `lint`. |
| CLI (`format`, `check`, `lint`, config discovery) | `gdstrict-cli` | Working. |
| Strict-mode driver (headless Godot, diagnostic extraction, warning-to-severity mapping) | `gdstrict-strict` | Working. Exposed via `check`. |

## Install

### Prebuilt binaries (recommended)

Static binaries for Linux (x86_64 and aarch64), macOS (Intel and Apple Silicon), and Windows (x86_64) are published with every tagged release.

**Linux and macOS** — shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/mrf/gdstrict/releases/latest/download/gdstrict-installer.sh \
  | sh
```

**Windows** — PowerShell installer:

```pwsh
irm https://github.com/mrf/gdstrict/releases/latest/download/gdstrict-installer.ps1 | iex
```

**Direct download** — grab a `.tar.gz` (Linux/macOS) or `.zip` (Windows) from the [Releases page](https://github.com/mrf/gdstrict/releases), extract, and put the `gdstrict` binary on your `PATH`.

### Build from source

Requires a [Rust toolchain](https://rustup.rs).

```sh
git clone https://github.com/mrf/gdstrict
cd gdstrict
cargo build --release
# binary lands at target/release/gdstrict
```

Strict mode (`check` without `--no-strict`) additionally requires a Godot binary on your machine. The formatter and linter never need Godot.

## Commands

### `format` — rewrite GDScript files to canonical style

```sh
# Rewrite every .gd file under the current directory in place.
gdstrict format .

# CI / pre-commit mode: write nothing, exit 1 if any file would change.
gdstrict format --check .

# Show a unified diff per file without writing anything.
gdstrict format --diff src/player.gd

# Override the line length for this run.
gdstrict format --line-length 120 .
```

Directories are walked recursively. gdstrict respects `.gdignore` and gitignore rules so it does not touch vendored or generated code.

**Exit codes:** `0` — success (files written, or nothing would change under `--check`). `1` — under `--check`, at least one file would change; or an error occurred.

### `lint` — syntactic style rules (no Godot needed)

```sh
# Lint every .gd file under the current directory.
gdstrict lint .

# Use a specific config file.
gdstrict lint --config path/to/gdstrict.toml src/
```

Runs the full naming-convention and code-quality rule set against the CST. No Godot binary required. See [Lint rules](#lint-rules) for the complete catalog.

**Exit codes:** `0` — no findings. `1` — at least one finding or an error occurred.

### `check` — aggregate CI gate (format + lint + strict)

```sh
# Full check: format-check + lint + strict type-checking.
gdstrict check .

# Strict requires a Godot binary. Point at one explicitly:
gdstrict check --godot /path/to/godot .

# Or skip the strict-typing pass entirely (no Godot needed):
gdstrict check --no-strict .

# Suppress per-finding output; only the summary and exit code remain.
gdstrict check --quiet .
```

Runs three passes in one shot:

1. **format-check** — same as `format --check`; a file that would change is a violation.
2. **lint** — full syntactic rule set.
3. **strict** — headless Godot analysis; diagnostics are mapped to `error`/`warn`/`off` by the severity profile from `gdstrict.toml`. Only `error`-level findings fail the gate; `warn`-level findings are printed but do not cause exit 1.

Godot binary discovery order: `--godot <path>` → `$GODOT` env var → `PATH`.

**Exit codes:** `0` — clean. `1` — at least one format/lint/strict violation. `2` — operational or configuration error (bad path, invalid config, or strict enabled but no Godot binary found).

## pre-commit hooks

gdstrict ships a [`pre-commit`](https://pre-commit.com/) hook definition. Add it to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/mrf/gdstrict
    rev: v0.1.0  # pin to a release tag
    hooks:
      # Fail if any staged .gd file is not formatted.
      - id: gdstrict-format

      # Fail on untyped or unsafe GDScript declarations (requires Godot on PATH).
      - id: gdstrict-check
```

`gdstrict-format` does not need Godot and is fast — suitable for every project. `gdstrict-check` shells out to the Godot binary; enable it only if you have Godot installed in CI.

If Godot is not on `PATH`, point `gdstrict-check` at it via the `GDSTRICT_GODOT` environment variable:

```yaml
      - id: gdstrict-check
        env:
          GDSTRICT_GODOT: /path/to/godot
```

Both hooks are built from source on first use via `cargo install`. Subsequent runs reuse the cached binary.

## Configuration

A `gdstrict.toml` is the one place a project configures every gdstrict subsystem. All keys are optional; omitting the file entirely is valid and applies built-in defaults.

```toml
# Maximum line length before the formatter wraps (default: 100).
line-length = 100

# Strict-mode severity preset (only "strict" is known; default when key is absent: strict).
preset = "strict"

[lint]
# Disable individual lint rules by setting them to false.
# All rules are enabled by default.
function-name-case = false
constant-name-case = false

[warnings]
# Per-code severity overrides for the strict pass.
# Valid values: "error" | "warn" | "off"
# Overrides beat the preset; unrecognized codes get the preset's default (warn).
INTEGER_DIVISION = "off"
RETURN_VALUE_DISCARDED = "warn"
```

Unknown top-level keys are rejected (a typo like `line_length` instead of `line-length` fails loudly rather than silently doing nothing).

**Config discovery** (highest precedence first):

1. `--line-length <n>` on the command line overrides line length for every file.
2. `--config <file>` — use this exact file for every input file, skipping discovery.
3. The nearest `gdstrict.toml` found by walking up the directory tree from each input file (the `black` / `ruff` discovery model).
4. Built-in defaults: `line-length = 100`, `preset = "strict"`.

### The `strict` preset

The built-in `strict` preset is applied by default whenever `gdstrict.toml` is absent or has no `preset` key. It promotes the following Godot warning codes to **errors** (failing `check` exit 1):

| Code | What it catches |
|---|---|
| `UNTYPED_DECLARATION` | Variables, parameters, or return types with no static type annotation |
| `INFERRED_DECLARATION` | Variables inferred from a `Variant` value — typed as `Variant`, not the real type |
| `UNSAFE_CAST` | An explicit cast that may fail at runtime |
| `UNSAFE_METHOD_ACCESS` | Calling a method not present on the inferred (Variant) type |
| `UNSAFE_PROPERTY_ACCESS` | Accessing a property not present on the inferred (Variant) type |
| `UNSAFE_CALL_ARGUMENT` | Passing a Variant where a typed argument is expected |
| `RETURN_VALUE_DISCARDED` | Ignoring a function's return value |

All other Godot warning codes default to **warn** under the `strict` preset (they are surfaced but do not fail the gate). You can demote, silence, or promote any code with a `[warnings]` override.

## Lint rules

All rules are enabled by default and can be disabled individually via the `[lint]` config table.

### Naming conventions

| Rule | What it checks |
|---|---|
| `function-name-case` | Function names must be `snake_case` (leading `_` for private is fine) |
| `variable-name-case` | Variable names must be `snake_case`, including local variables |
| `parameter-name-case` | Function and signal parameter names must be `snake_case` |
| `constant-name-case` | Constant names must be `SCREAMING_SNAKE_CASE` |
| `signal-name-case` | Signal names must be `snake_case` |
| `class-name-case` | Class and inner class names must be `PascalCase` |
| `enum-name-case` | Enum type names must be `PascalCase` |
| `enum-value-case` | Enum member names must be `SCREAMING_SNAKE_CASE` |

### Dead and redundant code

| Rule | What it checks |
|---|---|
| `unused-argument` | Function arguments never referenced in the body (prefix `_` to silence) |
| `unnecessary-pass` | A `pass` statement in a body that has other statements |
| `expression-not-assigned` | An expression used as a statement whose result is discarded (calls and `await` are exempt) |
| `no-else-return` | An `else` clause following an `if` body that always returns |
| `no-elif-return` | An `elif` clause following an `if` body that always returns |
| `comparison-with-itself` | A comparison operator with identical left and right operands (e.g. `x == x`) |
| `duplicated-load` | A `load()` or `preload()` call for a path that already appeared earlier in the file |

### Structure

| Rule | Default limit | What it checks |
|---|---|---|
| `class-definitions-order` | — | Class members must follow Godot's canonical order: tool/class annotations → `class_name` → `extends` → signals → enums → constants → exported vars → public vars → private vars → onready vars → methods |
| `private-method-call` | — | Calling a private method (leading `_`) on another object |
| `max-line-length` | 100 | Lines longer than `line-length` characters |
| `function-arguments-number` | 10 | Functions with more parameters than the limit |
| `max-public-methods` | 20 | Classes with more public methods than the limit |

## CI setup

The minimal CI configuration runs formatting, clippy, and tests on Linux, macOS, and Windows. Add a strict job when you want type checking against a pinned Godot release:

```yaml
# .github/workflows/ci.yml (excerpt)

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --workspace --all-targets
      - run: cargo test --workspace

  strict:
    runs-on: ubuntu-latest
    env:
      GODOT_VERSION: 4.6.2
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install pinned Godot (headless)
        run: |
          base="https://github.com/godotengine/godot/releases/download/${GODOT_VERSION}-stable"
          zip="Godot_v${GODOT_VERSION}-stable_linux.x86_64.zip"
          curl -fsSL "${base}/${zip}" -o /tmp/godot.zip
          unzip -q /tmp/godot.zip -d /tmp/godot
          sudo install -m755 "/tmp/godot/Godot_v${GODOT_VERSION}-stable_linux.x86_64" /usr/local/bin/godot
      - run: cargo test --workspace
        env:
          GODOT: /usr/local/bin/godot
```

**Pin the Godot version.** Godot's warning output is not a stable API; gdstrict version-gates its diagnostic parser against tested releases. Float the version only when you also update and verify the parser.

## How gdstrict compares

| | Formats | Auto-wraps long lines | Style lint | Real type checking | Engine |
|---|---|---|---|---|---|
| **gdstrict** | yes | yes | yes | yes (wraps Godot) | Rust + tree-sitter |
| gdformat / gdlint | yes | no | yes (syntactic) | no | Python + lark |
| GDQuest formatter | yes | no | yes (syntactic) | no | Rust + Topiary |
| Godot `--check-only` | no | no | no | yes | the engine |

## Architecture

A Cargo workspace that produces one distributable CLI binary. The pipeline is `source -> parse -> { format | lint | strict }`. Formatting and linting never need Godot (the fast path); strict mode shells out to the engine.

```
gdstrict/
  crates/
    gdstrict-cli/      # clap args, config discovery, exit codes, output rendering
    gdstrict-syntax/   # tree-sitter-gdscript wrapper: source to CST, error recovery, trivia
    gdstrict-format/   # CST to canonical text: document IR + width-aware renderer
    gdstrict-lint/     # syntactic style rules over the CST
    gdstrict-strict/   # headless Godot driver: run the analyzer, parse diagnostics
  fixtures/            # golden in/out pairs + idempotency corpus
```

The formatter uses a hand-written Wadler/Prettier-style document IR (`Group`, `Indent`, `Line`, `SoftLine`, `Text`) feeding a width-aware renderer, chosen over a query-based engine because auto-wrapping needs real layout control.

The deeper design rationale, milestones, and the research behind the strict-mode driver live in [PLAN.md](PLAN.md).

## Development

```sh
cargo test --workspace          # full suite, including the idempotency gate
cargo fmt --all --check         # formatting gate (the same one CI runs)
cargo clippy --workspace --all-targets -- -D warnings   # lint gate
```

CI runs the formatting and clippy gates on Linux, and builds and tests on Linux, macOS, and Windows. See [.github/workflows/ci.yml](.github/workflows/ci.yml).

## GitHub Action

gdstrict ships a reusable composite action (`action.yml` at the repo root) that installs gdstrict and runs format/lint/check steps inside any Godot project's CI.

### Minimal workflow

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

This runs `gdstrict format --check` and `gdstrict lint` on the repository root by default.

### With strict-typing check (requires Godot)

The `check` command drives Godot's own analyzer and requires a Godot binary. Pin the version to match your project's `gdstrict.toml` — Godot's diagnostic output is an unstable contract.

```yaml
- uses: mrf/gdstrict@main
  with:
    install-godot: 'true'
    godot-version: '4.6.2'   # pin to the version your project targets
    run-check: 'true'
```

### Inputs

| Input | Default | Description |
|---|---|---|
| `version` | `main` | Git ref (branch, tag, SHA) of gdstrict to install. |
| `install-method` | `cargo-install` | Install method. Only `cargo-install` is supported today (prebuilt binaries are not yet shipped). |
| `install-godot` | `false` | Install a headless Godot binary. Required when `run-check: true`. Linux runners only. |
| `godot-version` | `4.6.2` | Godot release to install. Pin this deliberately — see note above. |
| `run-format-check` | `true` | Run `gdstrict format --check`. Exits 1 if any file would change. |
| `run-lint` | `true` | Run `gdstrict lint` (syntactic naming rules). |
| `run-check` | `false` | Run `gdstrict check` (strict-typing via headless Godot). Requires `install-godot: true`. |
| `working-directory` | `.` | Directory to run gdstrict commands in. Must be a Godot project root. |

### Outputs

| Output | Description |
|---|---|
| `gdstrict-version` | The installed gdstrict version string. |

> **Note:** `gdstrict lint` and `gdstrict check` are currently in active development. The formatter (`gdstrict format --check`) is the stable command today. The action is designed to run all three so no workflow changes are needed when `lint` and `check` reach stable CLI status.

## Roadmap

- Prebuilt binaries for Linux, macOS, and Windows (cargo-dist).
- A `.pre-commit-hooks.yaml` so projects can install gdstrict as a pre-commit hook.
- A reusable GitHub Action wrapping the Godot install + `gdstrict check` flow.
- VS Code task / extension integration.

## License

MIT. See [LICENSE](LICENSE).
