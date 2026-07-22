import { resolve } from "node:path";

const desktopRoot = resolve(import.meta.dir, "..");
const target = process.env.TAURI_ENV_TARGET_TRIPLE || process.env.REPOMON_DESKTOP_TARGET || "";
const windows = process.platform === "win32" || target.includes("windows");

function run(command: string[]) {
  const result = Bun.spawnSync(command, { cwd: desktopRoot, stdout: "inherit", stderr: "inherit" });
  if (result.exitCode !== 0) process.exit(result.exitCode);
}

run(["bun", "run", "sidecar:prepare"]);
const args = ["bun", "tauri", "build", "--config", "src-tauri/tauri.release.conf.json"];
if (windows) args.push("--config", "src-tauri/tauri.release.win.conf.json");
run(args);
