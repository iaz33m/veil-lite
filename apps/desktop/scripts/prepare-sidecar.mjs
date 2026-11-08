#!/usr/bin/env node
// Puts the `veil-engine` binary where Tauri expects its sidecar:
//   src-tauri/binaries/veil-engine-<target-triple>[.exe]
//
// Usage: node scripts/prepare-sidecar.mjs [--build] [--release] [--target <triple>]
//   --build    build veil-engine with cargo first (otherwise copy an existing build)
//   --release  use the release profile (ignored when Tauri runs a debug build)
//   --target   target triple; defaults to TAURI_ENV_TARGET_TRIPLE, then the rustc host.
//              `universal-apple-darwin` builds both Mac architectures and joins them with lipo.
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(appDir, "../..");
const args = process.argv.slice(2);
const has = (name) => args.includes(name);
const value = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};

const host = /host: (\S+)/.exec(execFileSync("rustc", ["-vV"], { encoding: "utf8" }))?.[1];
if (!host) throw new Error("couldn't read the rustc host triple");
const triple = value("--target") ?? process.env.TAURI_ENV_TARGET_TRIPLE ?? host;
const release = has("--release") && process.env.TAURI_ENV_DEBUG !== "true";
const profile = release ? "release" : "debug";
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repoDir, "target");
const exe = (t) => (t.includes("windows") ? ".exe" : "");

/** Path of the built engine for one concrete triple, building it first when asked. */
function engineFor(t) {
  const crossTarget = t !== host;
  if (has("--build")) {
    const cargoArgs = ["build", "-p", "veil-engine"];
    if (process.env.CI) cargoArgs.push("--locked");
    if (release) cargoArgs.push("--release");
    if (crossTarget) cargoArgs.push("--target", t);
    console.log(`prepare-sidecar: cargo ${cargoArgs.join(" ")}`);
    execFileSync("cargo", cargoArgs, { cwd: repoDir, stdio: "inherit" });
  }
  const path = crossTarget
    ? join(targetDir, t, profile, `veil-engine${exe(t)}`)
    : join(targetDir, profile, `veil-engine${exe(t)}`);
  if (!existsSync(path)) {
    throw new Error(`no engine build at ${path}; run with --build or build veil-engine first`);
  }
  return path;
}

const outDir = join(appDir, "src-tauri", "binaries");
mkdirSync(outDir, { recursive: true });
const out = join(outDir, `veil-engine-${triple}${exe(triple)}`);

if (triple === "universal-apple-darwin") {
  const parts = ["aarch64-apple-darwin", "x86_64-apple-darwin"].map(engineFor);
  execFileSync("lipo", ["-create", "-output", out, ...parts], { stdio: "inherit" });
} else {
  copyFileSync(engineFor(triple), out);
}
if (!triple.includes("windows")) chmodSync(out, 0o755);
console.log(`prepare-sidecar: ${out}`);
