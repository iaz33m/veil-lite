#!/usr/bin/env bash
# Download the official ONNX Runtime shared library (CPU) for a platform and place it in
#   apps/desktop/src-tauri/resources/onnxruntime/
# The desktop app bundles it; the engine loads it at runtime (ort `load-dynamic`).
#
# Usage: scripts/fetch-onnxruntime.sh [--target <triple>|universal-apple-darwin] [--version 1.20.1]
# The version must match the ort API level the workspace is built with (api-20 → ONNX Runtime 1.20.x).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="1.20.1"
target="$(rustc -vV | sed -n 's/^host: //p')"

while [ $# -gt 0 ]; do
  case "$1" in
    --target) target="$2"; shift ;;
    --version) version="$2"; shift ;;
    -h|--help) sed -n 2,7p "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

case "$target" in
  universal-apple-darwin|aarch64-apple-darwin|x86_64-apple-darwin)
    asset="onnxruntime-osx-universal2-$version.tgz"; lib="libonnxruntime.$version.dylib"; out="libonnxruntime.dylib" ;;
  x86_64-pc-windows-msvc)
    asset="onnxruntime-win-x64-$version.zip"; lib="onnxruntime.dll"; out="onnxruntime.dll" ;;
  aarch64-pc-windows-msvc)
    asset="onnxruntime-win-arm64-$version.zip"; lib="onnxruntime.dll"; out="onnxruntime.dll" ;;
  x86_64-unknown-linux-gnu)
    asset="onnxruntime-linux-x64-$version.tgz"; lib="libonnxruntime.so.$version"; out="libonnxruntime.so" ;;
  aarch64-unknown-linux-gnu)
    asset="onnxruntime-linux-aarch64-$version.tgz"; lib="libonnxruntime.so.$version"; out="libonnxruntime.so" ;;
  *) echo "no ONNX Runtime build for $target" >&2; exit 1 ;;
esac

url="https://github.com/microsoft/onnxruntime/releases/download/v$version/$asset"
dest="$root/apps/desktop/src-tauri/resources/onnxruntime"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "downloading $url"
curl -fsSL --retry 3 -o "$work/$asset" "$url"
mkdir -p "$work/x" "$dest"
case "$asset" in
  *.tgz) tar -xzf "$work/$asset" -C "$work/x" ;;
  *.zip) (cd "$work/x" && unzip -q "$work/$asset") ;;
esac

found="$(find "$work/x" -name "$lib" -type f | head -n1)"
[ -n "$found" ] || { echo "library $lib not found in $asset" >&2; exit 1; }
cp "$found" "$dest/$out"
licence="$(find "$work/x" -maxdepth 2 -name 'LICENSE*' -type f | head -n1)"
[ -n "$licence" ] && cp "$licence" "$dest/LICENSE-onnxruntime.txt"
echo "onnxruntime $version → $dest/$out"
