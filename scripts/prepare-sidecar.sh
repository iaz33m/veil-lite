#!/usr/bin/env bash
# Build veil-engine and copy it to apps/desktop/src-tauri/binaries/veil-engine-<target-triple>[.exe],
# the name Tauri expects for an `externalBin` sidecar.
#
#   scripts/prepare-sidecar.sh                                   # debug, host target
#   scripts/prepare-sidecar.sh --release                         # release, host target
#   scripts/prepare-sidecar.sh --release --target x86_64-pc-windows-msvc
#   scripts/prepare-sidecar.sh --release --target universal-apple-darwin   # arm64 + x86_64, joined with lipo
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="debug"
target=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) profile="release"; shift ;;
    --target) target="${2:?--target needs a value}"; shift 2 ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

host="$(rustc -vV | sed -n 's/^host: //p')"
[[ -n "$target" ]] || target="$host"
out_dir="$root/apps/desktop/src-tauri/binaries"
mkdir -p "$out_dir"

cargo_flags=(-p veil-engine)
[[ "$profile" == "release" ]] && cargo_flags+=(--release)

build_one() {
  local triple="$1"
  if [[ "$triple" == "$host" ]]; then
    (cd "$root" && cargo build "${cargo_flags[@]}") >&2
    echo "$root/target/$profile/veil-engine"
  else
    (cd "$root" && cargo build "${cargo_flags[@]}" --target "$triple") >&2
    echo "$root/target/$triple/$profile/veil-engine"
  fi
}

if [[ "$target" == "universal-apple-darwin" ]]; then
  arm="$(build_one aarch64-apple-darwin)"
  x86="$(build_one x86_64-apple-darwin)"
  lipo -create -output "$out_dir/veil-engine-universal-apple-darwin" "$arm" "$x86"
  # Tauri resolves the sidecar per architecture during universal builds too.
  cp "$arm" "$out_dir/veil-engine-aarch64-apple-darwin"
  cp "$x86" "$out_dir/veil-engine-x86_64-apple-darwin"
  echo "sidecar: $out_dir/veil-engine-universal-apple-darwin"
  exit 0
fi

ext=""
[[ "$target" == *windows* ]] && ext=".exe"
bin="$(build_one "$target")$ext"
dest="$out_dir/veil-engine-$target$ext"
cp "$bin" "$dest"
chmod +x "$dest" 2>/dev/null || true
echo "sidecar: $dest"
