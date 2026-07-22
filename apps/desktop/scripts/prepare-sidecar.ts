import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";

const desktopRoot = resolve(import.meta.dir, "..");
const repoRoot = resolve(desktopRoot, "../..");

function hostTarget(): string {
  const result = Bun.spawnSync(["rustc", "-vV"], { cwd: repoRoot });
  const output = result.stdout.toString();
  const host = output.match(/^host: (.+)$/m)?.[1];
  if (!host) throw new Error("rustc did not report a host target");
  return host;
}

const target = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.REPOMON_DESKTOP_TARGET || hostTarget();
const windows = target.includes("windows");
const targetArgs = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.REPOMON_DESKTOP_TARGET
  ? ["--target", target]
  : [];
const packages = ["-p", "repomon-daemon"];
if (windows) packages.push("-p", "repomon-host");

const build = Bun.spawnSync(
  ["cargo", "build", "--release", "--locked", ...packages, ...targetArgs],
  { cwd: repoRoot, stdout: "inherit", stderr: "inherit" },
);
if (build.exitCode !== 0) throw new Error("desktop sidecar release build failed");

function copySidecar(name: string) {
  const executable = `${name}${windows ? ".exe" : ""}`;
  const source = resolve(repoRoot, "target", targetArgs.length ? target : "", "release", executable);
  const destination = resolve(desktopRoot, "src-tauri", "binaries", `${name}-${target}${windows ? ".exe" : ""}`);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(source, destination);
  console.info(`Prepared ${destination}`);
}

copySidecar("repomond");
if (windows) copySidecar("repomon-agent-host");
