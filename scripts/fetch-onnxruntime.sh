#!/usr/bin/env bash
# Download the official ONNX Runtime shared library for a target and place it where the desktop app
# bundles it: apps/desktop/src-tauri/resources/onnxruntime/{libonnxruntime.dylib|libonnxruntime.so|onnxruntime.dll}
#
# veil-detect loads it at runtime (`ort` is built with `load-dynamic`), so the engine binary itself has no
# link-time dependency on ONNX Runtime.
#
#   scripts/fetch-onnxruntime.sh                       # host target
#   scripts/fetch-onnxruntime.sh --target x86_64-pc-windows-msvc
#   ORT_VERSION=1.22.0 ORT_SHA256=<sha256 of the archive> scripts/fetch-onnxruntime.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="${ORT_VERSION:-1.22.0}"
target=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) target="${2:?--target needs a value}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -n "$target" ]] || target="$(rustc -vV | sed -n 's/^host: //p')"

case "$target" in
  universal-apple-darwin) asset="onnxruntime-osx-universal2-$version.tgz"; lib="libonnxruntime.$version.dylib"; out="libonnxruntime.dylib" ;;
  aarch64-apple-darwin)   asset="onnxruntime-osx-arm64-$version.tgz";      lib="libonnxruntime.$version.dylib"; out="libonnxruntime.dylib" ;;
  x86_64-apple-darwin)    asset="onnxruntime-osx-x86_64-$version.tgz";     lib="libonnxruntime.$version.dylib"; out="libonnxruntime.dylib" ;;
  x86_64-unknown-linux-*) asset="onnxruntime-linux-x64-$version.tgz";      lib="libonnxruntime.so.$version";    out="libonnxruntime.so" ;;
  aarch64-unknown-linux-*) asset="onnxruntime-linux-aarch64-$version.tgz"; lib="libonnxruntime.so.$version";    out="libonnxruntime.so" ;;
  x86_64-pc-windows-*)    asset="onnxruntime-win-x64-$version.zip";        lib="onnxruntime.dll";               out="onnxruntime.dll" ;;
  aarch64-pc-windows-*)   asset="onnxruntime-win-arm64-$version.zip";      lib="onnxruntime.dll";               out="onnxruntime.dll" ;;
  *) echo "no ONNX Runtime build known for $target" >&2; exit 1 ;;
esac

url="https://github.com/microsoft/onnxruntime/releases/download/v$version/$asset"
dest_dir="$root/apps/desktop/src-tauri/resources/onnxruntime"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "downloading $url"
curl -fL --retry 3 -o "$work/$asset" "$url"

if [[ -n "${ORT_SHA256:-}" ]]; then
  if command -v sha256sum >/dev/null; then got="$(sha256sum "$work/$asset" | cut -d' ' -f1)"; else got="$(shasum -a 256 "$work/$asset" | cut -d' ' -f1)"; fi
  [[ "$got" == "$ORT_SHA256" ]] || { echo "sha256 mismatch: got $got, expected $ORT_SHA256" >&2; exit 1; }
fi

mkdir -p "$work/x"
case "$asset" in
  *.tgz) tar -xzf "$work/$asset" -C "$work/x" ;;
  *.zip) unzip -q "$work/$asset" -d "$work/x" ;;
esac

found="$(find "$work/x" -type f -name "$lib" | head -n1)"
[[ -n "$found" ]] || { echo "$lib not found in $asset" >&2; exit 1; }

mkdir -p "$dest_dir"
cp "$found" "$dest_dir/$out"
license="$(find "$work/x" -maxdepth 2 -type f -name 'LICENSE*' | head -n1)"
[[ -n "$license" ]] && cp "$license" "$dest_dir/LICENSE-onnxruntime.txt"
echo "onnxruntime $version → $dest_dir/$out"
