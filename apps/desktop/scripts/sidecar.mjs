// Build vda-run (the command that refreshes a project without the app) and
// put it where Tauri bundles it: src-tauri/binaries/vda-run-<target triple>.
// Run by `npm run bundle` before the app is built (see tauri.bundle.conf.json).
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, chmodSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const out = join(root, "apps", "desktop", "src-tauri", "binaries");
const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)[1];
// The Tauri CLI says which target it is building for.
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const debug = process.env.TAURI_ENV_DEBUG === "true";
const profile = debug ? "debug" : "release";

function build(target) {
  const args = ["build", "-p", "vda-app", "--bin", "vda-run", "--locked"];
  if (!debug) args.push("--release");
  if (target !== host) args.push("--target", target);
  console.log(`sidecar: cargo ${args.join(" ")}`);
  execFileSync("cargo", args, { cwd: root, stdio: "inherit" });
  const exe = target.includes("windows") ? "vda-run.exe" : "vda-run";
  return target === host ? join(root, "target", profile, exe) : join(root, "target", target, profile, exe);
}

mkdirSync(out, { recursive: true });
if (triple === "universal-apple-darwin") {
  // Tauri builds each Mac chip type on its own and looks for vda-run per type.
  for (const arch of ["aarch64-apple-darwin", "x86_64-apple-darwin"]) {
    const dest = join(out, `vda-run-${arch}`);
    copyFileSync(build(arch), dest);
    chmodSync(dest, 0o755);
    console.log(`sidecar: ${dest}`);
  }
} else {
  const src = build(triple);
  const dest = join(out, `vda-run-${triple}${triple.includes("windows") ? ".exe" : ""}`);
  copyFileSync(src, dest);
  chmodSync(dest, 0o755);
  console.log(`sidecar: ${dest}`);
}
