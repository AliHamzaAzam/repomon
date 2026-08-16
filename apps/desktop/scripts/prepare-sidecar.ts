import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { cpus } from "node:os";
import { dirname, resolve } from "node:path";
import { gunzipSync } from "node:zlib";

const desktopRoot = resolve(import.meta.dir, "..");
const repoRoot = resolve(desktopRoot, "../..");
const cacheDir = resolve(repoRoot, "target", "sidecar-cache");

const isStrict = Boolean(
  process.env.CI ||
  process.env.STRICT_SIDECAR ||
  process.env.RELEASE ||
  process.env.GITHUB_ACTIONS
);

// Pinned source artifacts and checksums
const PINNED = {
  linuxX64StaticTmux: {
    url: "https://github.com/mjakob-gh/build-static-tmux/releases/download/v3.4/tmux.linux-amd64.gz",
    sha256: "0019dfc4b32d63c1392aa264aed2253c1e0c2fb09216f8e2cc269bbfb8bb49b5",
    version: "3.4",
  },
  libevent: {
    url: "https://github.com/libevent/libevent/releases/download/release-2.1.12-stable/libevent-2.1.12-stable.tar.gz",
    sha256: "92e6de1be9ec176428fd2367677e61ceffc2ee1cb119035037a27d346b0403bb",
    version: "2.1.12-stable",
    dirName: "libevent-2.1.12-stable",
  },
  tmuxSource: {
    url: "https://github.com/tmux/tmux/releases/download/3.4/tmux-3.4.tar.gz",
    sha256: "551ab8dea0bf505c0ad6b7bb35ef567cdde0ccb84357df142c254f35a23e19aa",
    version: "3.4",
    dirName: "tmux-3.4",
  },
};

function hostTarget(): string {
  const result = Bun.spawnSync(["rustc", "-vV"], { cwd: repoRoot });
  const output = result.stdout.toString();
  const host = output.match(/^host: (.+)$/m)?.[1];
  if (!host) throw new Error("rustc did not report a host target");
  return host;
}

const target =
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  process.env.REPOMON_DESKTOP_TARGET ||
  hostTarget();
const windows = target.includes("windows");
const targetArgs =
  process.env.TAURI_ENV_TARGET_TRIPLE || process.env.REPOMON_DESKTOP_TARGET
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
  const source = resolve(
    repoRoot,
    "target",
    targetArgs.length ? target : "",
    "release",
    executable
  );
  const destination = resolve(
    desktopRoot,
    "src-tauri",
    "binaries",
    `${name}-${target}${windows ? ".exe" : ""}`
  );
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(source, destination);
  chmodSync(destination, 0o755);
  console.info(`Prepared ${destination}`);
}

copySidecar("repomond");
if (windows) copySidecar("repomon-agent-host");

function computeSha256(data: Buffer | Uint8Array): string {
  return createHash("sha256").update(data).digest("hex");
}

async function downloadWithChecksum(
  url: string,
  expectedSha256: string,
  destPath: string
): Promise<void> {
  if (existsSync(destPath)) {
    const existing = readFileSync(destPath);
    if (computeSha256(existing) === expectedSha256) {
      return;
    }
  }
  mkdirSync(dirname(destPath), { recursive: true });
  console.info(`Downloading ${url}...`);
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to download ${url}: HTTP ${response.status} ${response.statusText}`);
  }
  const buffer = Buffer.from(await response.arrayBuffer());
  const actualSha256 = computeSha256(buffer);
  if (actualSha256 !== expectedSha256) {
    throw new Error(
      `Checksum mismatch for ${url}.\nExpected: ${expectedSha256}\nActual:   ${actualSha256}`
    );
  }
  writeFileSync(destPath, buffer);
}

function verifyMacOsDylibs(binaryPath: string) {
  const otool = Bun.spawnSync(["otool", "-L", binaryPath]);
  if (otool.exitCode !== 0) {
    throw new Error(`otool -L failed on ${binaryPath}: ${otool.stderr.toString()}`);
  }
  const lines = otool.stdout.toString().split("\n").slice(1);
  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line) continue;
    const match = line.match(/^(\S+)/);
    if (!match) continue;
    const libPath = match[1];
    if (!libPath.startsWith("/usr/lib/") && !libPath.startsWith("/System/")) {
      throw new Error(
        `Non-portable dylib linked by ${binaryPath}: ${libPath} (full line: ${line})`
      );
    }
  }
}

async function acquireTmuxForLinux(destination: string): Promise<void> {
  const cachedBinary = resolve(cacheDir, `tmux-${target}`);
  if (!existsSync(cachedBinary)) {
    const gzPath = resolve(cacheDir, "downloads", "tmux-linux-amd64.gz");
    await downloadWithChecksum(
      PINNED.linuxX64StaticTmux.url,
      PINNED.linuxX64StaticTmux.sha256,
      gzPath
    );
    const compressed = readFileSync(gzPath);
    const decompressed = gunzipSync(compressed);
    mkdirSync(dirname(cachedBinary), { recursive: true });
    writeFileSync(cachedBinary, decompressed);
    chmodSync(cachedBinary, 0o755);
  }

  copyFileSync(cachedBinary, destination);
  chmodSync(destination, 0o755);
  console.info(`Prepared static Linux tmux: ${destination}`);
}

async function acquireTmuxForMacOs(destination: string): Promise<void> {
  const cachedBinary = resolve(cacheDir, `tmux-${target}`);
  let isCacheValid = false;

  if (existsSync(cachedBinary)) {
    try {
      verifyMacOsDylibs(cachedBinary);
      isCacheValid = true;
    } catch {
      isCacheValid = false;
    }
  }

  if (!isCacheValid) {
    const downloadsDir = resolve(cacheDir, "downloads");
    const libeventTar = resolve(downloadsDir, "libevent.tar.gz");
    const tmuxTar = resolve(downloadsDir, "tmux.tar.gz");

    await downloadWithChecksum(PINNED.libevent.url, PINNED.libevent.sha256, libeventTar);
    await downloadWithChecksum(PINNED.tmuxSource.url, PINNED.tmuxSource.sha256, tmuxTar);

    const buildRoot = resolve(cacheDir, `build-${target}`);
    rmSync(buildRoot, { recursive: true, force: true });
    mkdirSync(buildRoot, { recursive: true });

    // Extract libevent
    const untarLibevent = Bun.spawnSync(["tar", "-xzf", libeventTar, "-C", buildRoot]);
    if (untarLibevent.exitCode !== 0) {
      throw new Error(`Failed to extract libevent: ${untarLibevent.stderr.toString()}`);
    }

    // Extract tmux
    const untarTmux = Bun.spawnSync(["tar", "-xzf", tmuxTar, "-C", buildRoot]);
    if (untarTmux.exitCode !== 0) {
      throw new Error(`Failed to extract tmux: ${untarTmux.stderr.toString()}`);
    }

    const libeventDir = resolve(buildRoot, PINNED.libevent.dirName);
    const tmuxDir = resolve(buildRoot, PINNED.tmuxSource.dirName);
    const depsInstallDir = resolve(buildRoot, "deps");
    const numCpus = String(Math.max(1, cpus().length));

    // Arch flags if targeting cross-arch
    const archFlags: string[] = [];
    if (target.startsWith("aarch64")) {
      archFlags.push("CFLAGS=-arch arm64", "LDFLAGS=-arch arm64");
    } else if (target.startsWith("x86_64")) {
      archFlags.push("CFLAGS=-arch x86_64", "LDFLAGS=-arch x86_64");
    }

    console.info(`Building static libevent for ${target}...`);
    const libeventConfigure = Bun.spawnSync(
      [
        "./configure",
        "--disable-shared",
        "--enable-static",
        "--disable-samples",
        "--disable-openssl",
        `--prefix=${depsInstallDir}`,
        ...archFlags,
      ],
      { cwd: libeventDir, stdout: "pipe", stderr: "pipe" }
    );
    if (libeventConfigure.exitCode !== 0) {
      throw new Error(
        `libevent configure failed:\n${libeventConfigure.stderr.toString()}`
      );
    }

    const libeventMake = Bun.spawnSync(["make", `-j${numCpus}`, "install"], {
      cwd: libeventDir,
      stdout: "pipe",
      stderr: "pipe",
    });
    if (libeventMake.exitCode !== 0) {
      throw new Error(`libevent build failed:\n${libeventMake.stderr.toString()}`);
    }

    console.info(`Building tmux for ${target} with static libevent...`);
    const tmuxConfigure = Bun.spawnSync(
      [
        "./configure",
        "--disable-utf8proc",
        `--prefix=${resolve(buildRoot, "dist")}`,
        ...archFlags,
      ],
      {
        cwd: tmuxDir,
        env: {
          ...process.env,
          PKG_CONFIG_PATH: resolve(depsInstallDir, "lib", "pkgconfig"),
        },
        stdout: "pipe",
        stderr: "pipe",
      }
    );
    if (tmuxConfigure.exitCode !== 0) {
      throw new Error(`tmux configure failed:\n${tmuxConfigure.stderr.toString()}`);
    }

    const tmuxMake = Bun.spawnSync(["make", `-j${numCpus}`], {
      cwd: tmuxDir,
      stdout: "pipe",
      stderr: "pipe",
    });
    if (tmuxMake.exitCode !== 0) {
      throw new Error(`tmux make failed:\n${tmuxMake.stderr.toString()}`);
    }

    const builtTmux = resolve(tmuxDir, "tmux");
    verifyMacOsDylibs(builtTmux);
    Bun.spawnSync(["strip", builtTmux]);

    mkdirSync(dirname(cachedBinary), { recursive: true });
    copyFileSync(builtTmux, cachedBinary);
    chmodSync(cachedBinary, 0o755);
  }

  copyFileSync(cachedBinary, destination);
  chmodSync(destination, 0o755);
  console.info(`Prepared portable macOS tmux: ${destination}`);
}

async function prepareTmuxSidecar() {
  if (windows) {
    return;
  }

  const destination = resolve(
    desktopRoot,
    "src-tauri",
    "binaries",
    `tmux-${target}`
  );
  mkdirSync(dirname(destination), { recursive: true });

  try {
    if (target.includes("linux")) {
      await acquireTmuxForLinux(destination);
    } else if (target.includes("apple-darwin") || target.includes("darwin")) {
      await acquireTmuxForMacOs(destination);
    } else {
      throw new Error(`Unsupported target for bundled tmux: ${target}`);
    }

    // Verify the prepared binary runs -V
    const testRun = Bun.spawnSync([destination, "-V"]);
    if (testRun.exitCode !== 0) {
      throw new Error(
        `Prepared tmux binary failed verification (${destination} -V): ${testRun.stderr.toString()}`
      );
    }
    const versionOutput = testRun.stdout.toString().trim();
    console.info(`Verified ${destination} -> ${versionOutput}`);
  } catch (error) {
    if (isStrict) {
      throw error;
    } else {
      console.warn(
        `⚠️  [prepare-sidecar] Warning: failed to acquire bundled tmux sidecar (${(error as Error).message}). Dev builds will continue without bundled tmux.`
      );
    }
  }
}

await prepareTmuxSidecar();
