#!/usr/bin/env bash
# Build veil-engine and place it where Tauri expects the sidecar:
#   apps/desktop/src-tauri/binaries/veil-engine-<target-triple>[.exe]
#
# Usage: scripts/prepare-sidecar.sh [--release] [--target <triple>|universal-apple-darwin]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile=debug
cargo_flags=()
target=""

while [ $# -gt 0 ]; do
  case "$1" in
    --release) profile=release; cargo_flags+=(--release) ;;
    --target) target="$2"; shift ;;
    -h|--help) sed -n 2,6p "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

host="$(rustc -vV | sed -n 's/^host: //p')"
target="${target:-$host}"
dest="$root/apps/desktop/src-tauri/binaries"
mkdir -p "$dest"

ext=""
case "$target" in *windows*) ext=".exe" ;; esac

build_one() { # triple
  local t="$1"
  if [ "$t" = "$host" ]; then
    cargo build -p veil-engine "${cargo_flags[@]}" --manifest-path "$root/Cargo.toml" >&2
    echo "$root/target/$profile/veil-engine$ext"
  else
    rustup target add "$t" >&2 || true
    cargo build -p veil-engine "${cargo_flags[@]}" --target "$t" --manifest-path "$root/Cargo.toml" >&2
    echo "$root/target/$t/$profile/veil-engine$ext"
  fi
}

if [ "$target" = "universal-apple-darwin" ]; then
  arm="$(build_one aarch64-apple-darwin)"
  x86="$(build_one x86_64-apple-darwin)"
  lipo -create -output "$dest/veil-engine-universal-apple-darwin" "$arm" "$x86"
  # Tauri also looks up the per-arch names when building a single arch.
  cp "$arm" "$dest/veil-engine-aarch64-apple-darwin"
  cp "$x86" "$dest/veil-engine-x86_64-apple-darwin"
  echo "sidecar: $dest/veil-engine-universal-apple-darwin"
else
  bin="$(build_one "$target")"
  cp "$bin" "$dest/veil-engine-$target$ext"
  echo "sidecar: $dest/veil-engine-$target$ext"
fi
