#!/usr/bin/env bash
#
# pre-commit shim: run gdstrict from a prebuilt release binary.
#
# The `language: rust` hooks compile gdstrict from source on first use, which
# costs minutes. This script downloads the static binary for the current
# platform instead, caches it, and execs it. Subsequent commits reuse the
# cached binary and pay only the exec.
#
# Usage: gdstrict-prebuilt.sh <subcommand> [args...] [files...]
#
# Environment:
#   GDSTRICT_BIN      Use this binary directly; skip download entirely.
#   GDSTRICT_VERSION  Release tag to fetch (e.g. v0.1.0). Defaults to the
#                     version in the Cargo.toml of this checkout, so a pinned
#                     `rev:` in .pre-commit-config.yaml pins the binary too.
#   GDSTRICT_CACHE    Cache directory. Defaults under XDG_CACHE_HOME.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

die() {
	printf 'gdstrict-prebuilt: %s\n' "$1" >&2
	exit 1
}

# An explicit binary wins over everything — lets a project use a local build.
if [ -n "${GDSTRICT_BIN:-}" ]; then
	exec "$GDSTRICT_BIN" "$@"
fi

resolve_version() {
	if [ -n "${GDSTRICT_VERSION:-}" ]; then
		printf '%s' "$GDSTRICT_VERSION"
		return
	fi
	# Derive from the checkout so the hook's pinned rev picks the matching
	# release, with no separate VERSION file to drift out of sync.
	local v
	v="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo_root/Cargo.toml" | head -n1)"
	[ -n "$v" ] || die "could not read version from $repo_root/Cargo.toml; set GDSTRICT_VERSION"
	printf 'v%s' "$v"
}

detect_target() {
	local os arch
	case "$(uname -s)" in
		Linux) os='unknown-linux-musl' ;;
		Darwin) os='apple-darwin' ;;
		MINGW* | MSYS* | CYGWIN*) os='pc-windows-msvc' ;;
		*) die "unsupported OS: $(uname -s)" ;;
	esac
	case "$(uname -m)" in
		x86_64 | amd64) arch='x86_64' ;;
		arm64 | aarch64) arch='aarch64' ;;
		*) die "unsupported architecture: $(uname -m)" ;;
	esac
	# The release ships no aarch64 Windows archive.
	if [ "$os" = 'pc-windows-msvc' ] && [ "$arch" = 'aarch64' ]; then
		die 'no prebuilt binary for aarch64 Windows; use the language: rust hooks instead'
	fi
	printf '%s-%s' "$arch" "$os"
}

version="$(resolve_version)"
target="$(detect_target)"

cache_root="${GDSTRICT_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/gdstrict}"
bin_dir="$cache_root/$version/$target"

case "$target" in
	*windows*) bin="$bin_dir/gdstrict.exe" ;;
	*) bin="$bin_dir/gdstrict" ;;
esac

if [ ! -x "$bin" ]; then
	case "$target" in
		*windows*) archive="gdstrict-$target.zip" ;;
		*) archive="gdstrict-$target.tar.xz" ;;
	esac
	url="https://github.com/mrf/gdstrict/releases/download/$version/$archive"

	tmp="$(mktemp -d)"
	# shellcheck disable=SC2064  # expand tmp now, not at trap time
	trap "rm -rf '$tmp'" EXIT

	printf 'gdstrict-prebuilt: fetching %s\n' "$url" >&2
	if ! curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$tmp/$archive"; then
		die "download failed: $url
Pin a released tag in .pre-commit-config.yaml, or set GDSTRICT_VERSION to one."
	fi

	# Modern bsdtar/GNU tar both extract .zip as well as .tar.xz.
	tar -xf "$tmp/$archive" -C "$tmp"

	extracted="$(find "$tmp" -type f -name 'gdstrict' -o -type f -name 'gdstrict.exe' | head -n1)"
	[ -n "$extracted" ] || die "no gdstrict binary inside $archive"

	mkdir -p "$bin_dir"
	# Move into place via a temp name so a concurrent hook run never observes
	# a half-written binary.
	mv "$extracted" "$bin.tmp.$$"
	chmod +x "$bin.tmp.$$"
	mv "$bin.tmp.$$" "$bin"
fi

exec "$bin" "$@"
