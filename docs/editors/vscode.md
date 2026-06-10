# VS Code Integration

gdstrict integrates with VS Code in two ways:

1. **Format on save** — rewrite the active `.gd` file in place every time you save.
2. **Check task** — run `gdstrict format --check` across the project and surface results in the Problems panel.

The example configs below live in the repo as [`docs/editors/vscode/`](vscode/).

---

## Prerequisites

Build gdstrict from source and note the path to the binary (see the [Install section in the README](../../README.md#install)):

```sh
cargo build --release
# binary: target/release/gdstrict
```

For format-on-save, install the [Run on Save](https://marketplace.visualstudio.com/items?itemName=emeraldwalk.RunOnSave) extension (`emeraldwalk.runonsave`).

The [Godot Tools](https://marketplace.visualstudio.com/items?itemName=geequlim.godot-tools) extension (`geequlim.godot-tools`) provides GDScript language support. It is not required for gdstrict but pairs naturally with it.

---

## Format on save

Add this block to your project's `.vscode/settings.json`, adjusting the `cmd` path to wherever your `gdstrict` binary lives:

```json
{
  "emeraldwalk.runonsave": {
    "commands": [
      {
        "match": "\\.gd$",
        "cmd": "${workspaceFolder}/target/release/gdstrict format ${file}",
        "runIn": "backend"
      }
    ]
  }
}
```

`runIn: "backend"` runs the command silently. VS Code will reload the saved file after the command exits, so you see the formatted result immediately.

If `gdstrict` is on your `PATH` (e.g. installed to `~/.cargo/bin`), you can shorten the command:

```json
"cmd": "gdstrict format ${file}"
```

### What this does

On every save of a `.gd` file, VS Code runs `gdstrict format <path>`. gdstrict rewrites the file in place to canonical style. VS Code then picks up the updated content from disk — the round-trip is invisible.

---

## Check task (Problems panel / CI-style)

Add a task to `.vscode/tasks.json` that runs `gdstrict format --check` across the workspace and reports which files are out of format:

```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "gdstrict: check format",
      "type": "shell",
      "command": "${workspaceFolder}/target/release/gdstrict",
      "args": ["format", "--check", "."],
      "group": "build",
      "presentation": {
        "reveal": "always",
        "panel": "shared"
      },
      "problemMatcher": []
    },
    {
      "label": "gdstrict: format all",
      "type": "shell",
      "command": "${workspaceFolder}/target/release/gdstrict",
      "args": ["format", "."],
      "group": "build",
      "presentation": {
        "reveal": "always",
        "panel": "shared"
      },
      "problemMatcher": []
    }
  ]
}
```

Run these with **Terminal → Run Task → gdstrict: check format** (or **format all**). Exit code 1 means at least one file would change; exit code 0 means everything is already formatted.

---

## Recommended extensions

A `.vscode/extensions.json` that prompts team members to install the required extensions:

```json
{
  "recommendations": [
    "emeraldwalk.runonsave",
    "geequlim.godot-tools"
  ]
}
```

---

## Complete copy-paste example

The [`docs/editors/vscode/`](vscode/) directory contains working versions of all three files. Copy its contents into your project's `.vscode/` directory and adjust the binary path.

| File | Purpose |
|---|---|
| [`docs/editors/vscode/settings.json`](vscode/settings.json) | Format-on-save via Run on Save |
| [`docs/editors/vscode/tasks.json`](vscode/tasks.json) | Check and format-all build tasks |
| [`docs/editors/vscode/extensions.json`](vscode/extensions.json) | Recommended extension list |
