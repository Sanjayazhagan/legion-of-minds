#!/usr/bin/env bash
# Prints the version the next release gets, without the leading "v".
#   - no v* tag yet            -> the version in game/Cargo.toml (0.1.0 today)
#   - Cargo.toml is ahead      -> the Cargo.toml version (edit it to jump to 0.2.0 or 1.0.0)
#   - otherwise                -> the latest tag with the last number + 1 (v0.1.4 -> 0.1.5)
# Tags must already be fetched (checkout with fetch-depth: 0).
set -euo pipefail
cargo_toml="${1:-game/Cargo.toml}"
cargo=$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/p' "$cargo_toml" | head -n1)
[ -n "$cargo" ] || { echo "no version in $cargo_toml" >&2; exit 1; }
latest=$(git tag --list 'v[0-9]*.[0-9]*.[0-9]*' | sed 's/^v//' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n1 || true)
if [ -z "$latest" ]; then
  echo "$cargo"
elif [ "$(printf '%s\n%s\n' "$latest" "$cargo" | sort -V | tail -n1)" = "$cargo" ] && [ "$cargo" != "$latest" ]; then
  echo "$cargo"
else
  IFS=. read -r major minor patch <<<"$latest"
  echo "$major.$minor.$((patch + 1))"
fi
