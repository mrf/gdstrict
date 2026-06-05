#!/usr/bin/env bash
# Best-effort acceptance gate 3 (PLAN.md sec 3): prove the formatter introduces
# no NEW Godot parse/script errors on the real corpus.
#
# Builds two throwaway Godot projects that mirror the corpus at its real
# res://addons/stagehand/core/ path (so the cross-file preloads resolve) — one
# with the unformatted vendored files, one with `gdstrict_format`-formatted
# files — then runs `godot --headless --check-only` on each script and diffs the
# error counts. Requires `godot` (4.6.x) on PATH.
#
# Verified result at landing: all 11 files report 0 errors in BOTH projects ->
# formatting introduces no new errors. Run from the repo root:
#   bash fixtures/acceptance/acceptance_godot_check.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CORPUS="$ROOT/fixtures/acceptance/stagehand_core"
PROJ="$ROOT/fixtures/acceptance/project.godot"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

for variant in unfmt fmt; do
  mkdir -p "$TMP/$variant/addons/stagehand/core"
  cp "$PROJ" "$TMP/$variant/project.godot"
done
for f in "$CORPUS"/*.gd; do
  b="$(basename "$f")"
  cp "$f" "$TMP/unfmt/addons/stagehand/core/$b"
  cargo run -q -p gdstrict-format --example fmt -- "$f" > "$TMP/fmt/addons/stagehand/core/$b"
done

count_errors() { # project dir -> "file errcount" per line
  local proj="$1"
  for f in "$proj"/addons/stagehand/core/*.gd; do
    local rel="res://addons/stagehand/core/$(basename "$f")"
    local out
    out="$(godot --headless --path "$proj" --check-only --script "$rel" 2>&1 || true)"
    echo "$(basename "$f") $(echo "$out" | grep -cE 'SCRIPT ERROR|ERROR:')"
  done
}

echo "FILE UNFMT FMT VERDICT"
diffs=0
join <(count_errors "$TMP/unfmt" | sort) <(count_errors "$TMP/fmt" | sort) \
  | while read -r file u v; do
      verdict=SAME; [ "$u" != "$v" ] && { verdict=NEW-ERRORS; }
      echo "$file $u $v $verdict"
    done
echo "(NEW-ERRORS on any row = formatter regression to investigate)"
