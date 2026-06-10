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

The opportunity is a single standalone tool that does two things well:

1. Formats deterministically, like `black`, with automatic line wrapping.
2. Enforces strict typing by failing the build on untyped or unsafe code.

No single tool ships that combined workflow today. That is the niche gdstrict aims to fill, building on the tools above rather than replacing them.

## Philosophy

gdstrict is built on a few firm opinions.

**Formatting is not a matter of taste.** The formatter is deterministic, idempotent, and close to zero-config, the same contract that made `black` win. There is exactly one knob, `line-length` (default 100, matching Godot's style guide). You do not configure style. You adopt it, and you stop arguing about it in code review.

**Idempotency is a hard invariant, not a nice-to-have.** `format(format(x))` must equal `format(x)` for every input. This is gated in CI by double-formatting every fixture. Idempotency is genuinely hard to get right, so gdstrict treats the invariant as load-bearing from day one.

**Auto-wrapping is the whole point.** Long call chains, argument lists, and array or dictionary literals wrap to one element per line, with a magic trailing comma that forces the expanded form (the `black` and `ruff` behavior). Auto-wrapping is the capability the other fast formatters intentionally leave out, and providing it is a core reason gdstrict exists.

**Strict mode should mean something real.** Reimplementing GDScript's type system would be a multi-year trap. Instead, gdstrict drives the engine's own analyzer headlessly and parses its diagnostics. The type checking is exactly as accurate as Godot itself, because it is Godot. A `gdstrict.toml` profile maps each warning to `error`, `warn`, or `off`, which gives you the project-wide warnings-as-errors enforcement the engine does not offer out of the box.

**Comments and whitespace are first-class.** Comment handling is exactly where formatters break, so the parser treats comments, blank lines, doc comments, annotations, backslash continuations, and multiline strings as significant trivia, with golden tests covering them.

**Lean on prior art.** gdstrict does not hand-write a parser (it uses `tree-sitter-gdscript`), it does not reinvent a style contract (it copies the documented `black` and `ruff` behavior), and it does not reinvent type checking (it wraps Godot). The novel work is the combination and the strict-mode driver, not the parts that already exist.

## Status

gdstrict is early and under active development. Here is the honest state of each piece.

| Capability | Crate | Status |
|---|---|---|
| Formatter (parse, document IR, width-aware wrapping) | `gdstrict-format` | Working. Exposed via the `format` command. |
| CLI (`format`, `--check`, `--diff`, config discovery) | `gdstrict-cli` | Working. |
| Strict-mode driver (headless Godot, two-pass error and warning extraction, `override.cfg` injection, warning-to-severity mapping) | `gdstrict-strict` | Functional as a library. Not yet wired to a CLI subcommand. |
| Syntactic naming rules (snake_case, PascalCase, CONSTANT_CASE, signal names) | `gdstrict-lint` | Functional as a library. Not yet wired to a CLI subcommand. |

What you can run today is `gdstrict format`. The `check` (strict) and `lint` commands are next on the roadmap; the engines behind them already exist and are tested.

## Install

gdstrict is a Rust workspace. Until prebuilt binaries ship, build from source:

```sh
git clone https://github.com/mrf/gdstrict
cd gdstrict
cargo build --release
# the binary lands at target/release/gdstrict
```

Strict mode (once wired to the CLI) additionally requires a Godot binary on your machine. The formatter and linter never need Godot.

## Usage

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

### Exit codes

These are designed for pre-commit hooks and CI:

- `0`: success. Files were written, or nothing would change under `--check`.
- `1`: under `--check`, at least one file would change, or an error occurred.

## Configuration

Configuration is intentionally minimal. A `gdstrict.toml` is a tiny TOML file with a single recognized key:

```toml
line-length = 100
```

Resolution precedence (highest wins):

1. `--line-length <n>` on the command line.
2. `--config <file>`, used verbatim for every file.
3. The nearest `gdstrict.toml`, found by walking up the directory tree from each file (the `black` and `ruff` discovery model).
4. The built-in default of 100.

## How gdstrict compares

| | Formats | Auto-wraps long lines | Style lint | Real type checking | Engine |
|---|---|---|---|---|---|
| **gdstrict** | yes | yes | planned | yes (wraps Godot) | Rust + tree-sitter |
| gdformat / gdlint | yes | no | yes (syntactic) | no | Python + lark |
| GDQuest formatter | yes | no | yes (syntactic) | no | Rust + Topiary |
| Godot `--check-only` | no | no | no | yes | the engine |

## Architecture

A Cargo workspace that produces one distributable CLI binary. The pipeline is `source -> parse -> { format | lint | strict }`. Formatting never needs Godot (the fast path); strict mode shells out to the engine.

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

## Editor Integration

### VS Code

Format on save and a check task are documented in [docs/editors/vscode.md](docs/editors/vscode.md). A ready-to-copy `.vscode/` directory is included at the repo root — copy it into your Godot project and adjust the binary path.

Quick start:

1. Install the [Run on Save](https://marketplace.visualstudio.com/items?itemName=emeraldwalk.RunOnSave) extension.
2. Copy `docs/editors/vscode/settings.json` from this repo into your project's `.vscode/` directory.
3. Point the `cmd` at your `gdstrict` binary.

Every `.gd` save will then run `gdstrict format` in the background. See the [full guide](docs/editors/vscode.md) for the check task and other options.

## Roadmap

- Wire the strict-mode driver to a `gdstrict check` command, including the warning-to-severity profile and a `strict` preset.
- Wire the naming rules to a `gdstrict lint` command.
- Distribution: a pre-commit hook, a GitHub Action, and prebuilt binaries for Linux, macOS, and Windows.

## License

MIT. See [LICENSE](LICENSE).
