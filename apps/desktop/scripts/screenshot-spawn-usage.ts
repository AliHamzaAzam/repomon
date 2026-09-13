/// Captures the keyboard-first Spawn dialog and the rate-limits meters against the screenshot
/// fixture, never a live daemon. Chrome is driven over CDP rather than `--screenshot` because the
/// focus ring these shots exist to prove only renders for genuine keyboard input; a programmatic
/// .focus() never sets :focus-visible, so a headless still would be evidence of nothing.
///
/// Start the fixture server first:
///   bunx --bun vite --config vite.screenshot.config.ts --port 4178
/// then: bun scripts/screenshot-spawn-usage.ts
import { mkdirSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const ORIGIN = process.env.REPOMON_SHOT_ORIGIN ?? "http://127.0.0.1:4178";
const OUT = resolve(import.meta.dir, "../../../qa", process.env.REPOMON_SCREENSHOT_OUTPUT ?? "spawn-keyboard-usage-bars");
const SIZES: Array<[number, number]> = [[1440, 900], [1040, 680]];
const THEMES = ["light", "dark"] as const;

type Shot = { name: string; query: string; ready: string; keys: string[] };

const SHOTS: Shot[] = [
  {
    // Arrow into the grid so the tile that carries focus carries a real focus ring too.
    name: "spawn-dialog",
    query: "surface=conversation&case=spawn-keys",
    ready: '[role="radiogroup"]',
    keys: ["ArrowRight", "ArrowDown"],
  },
  {
    name: "rate-limits",
    query: "fleet=ordinary&usage=1",
    ready: '[role="progressbar"]',
    keys: [],
  },
];

const KEY_CODES: Record<string, { code: string; keyCode: number }> = {
  ArrowRight: { code: "ArrowRight", keyCode: 39 },
  ArrowDown: { code: "ArrowDown", keyCode: 40 },
  ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
  ArrowUp: { code: "ArrowUp", keyCode: 38 },
};

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

class Session {
  #socket: WebSocket;
  #next = 1;
  #pending = new Map<number, { resolve: (value: any) => void; reject: (cause: Error) => void }>();

  constructor(socket: WebSocket) {
    this.#socket = socket;
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data));
      const waiting = this.#pending.get(message.id);
      if (!waiting) return;
      this.#pending.delete(message.id);
      if (message.error) waiting.reject(new Error(JSON.stringify(message.error)));
      else waiting.resolve(message.result);
    });
  }

  send(method: string, params: Record<string, unknown> = {}): Promise<any> {
    const id = this.#next++;
    this.#socket.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.#pending.set(id, { resolve, reject }));
  }

  close() {
    this.#socket.close();
  }
}

async function connect(port: number): Promise<Session> {
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = targets.find((target: { type: string }) => target.type === "page");
      if (page) {
        const socket = new WebSocket(page.webSocketDebuggerUrl);
        await new Promise((ready, failed) => {
          socket.addEventListener("open", ready, { once: true });
          socket.addEventListener("error", () => failed(new Error("devtools socket failed")), { once: true });
        });
        return new Session(socket);
      }
    } catch {
      // Chrome is still coming up.
    }
    await sleep(100);
  }
  throw new Error("Chrome never exposed a page target");
}

async function waitFor(session: Session, selector: string) {
  for (let attempt = 0; attempt < 150; attempt++) {
    const { result } = await session.send("Runtime.evaluate", {
      expression: `document.querySelector(${JSON.stringify(selector)}) !== null`,
      returnByValue: true,
    });
    if (result.value === true) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${selector}`);
}

async function press(session: Session, key: string) {
  const descriptor = KEY_CODES[key];
  if (!descriptor) throw new Error(`unmapped key ${key}`);
  const base = { key, code: descriptor.code, windowsVirtualKeyCode: descriptor.keyCode, nativeVirtualKeyCode: descriptor.keyCode };
  await session.send("Input.dispatchKeyEvent", { type: "rawKeyDown", ...base });
  await session.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
  await sleep(120);
}

mkdirSync(OUT, { recursive: true });
let port = 9333;
for (const shot of SHOTS) {
  for (const theme of THEMES) {
    for (const [width, height] of SIZES) {
      const profile = mkdtempSync(join(tmpdir(), "repomon-shot-"));
      const chrome = spawn(CHROME, [
        "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
        `--user-data-dir=${profile}`, "--hide-scrollbars", `--window-size=${width},${height}`,
        "--force-device-scale-factor=1", `--remote-debugging-port=${port}`, "about:blank",
      ], { stdio: "ignore" });
      try {
        const session = await connect(port);
        await session.send("Page.enable");
        await session.send("Runtime.enable");
        await session.send("Page.navigate", { url: `${ORIGIN}/?${shot.query}&theme=${theme}` });
        await waitFor(session, shot.ready);
        await sleep(600);
        for (const key of shot.keys) await press(session, key);
        await sleep(200);
        const { data } = await session.send("Page.captureScreenshot", { format: "png" });
        const path = join(OUT, `${shot.name}-${theme}-${width}x${height}.png`);
        writeFileSync(path, Buffer.from(data, "base64"));
        console.log(path);
        session.close();
      } finally {
        chrome.kill();
        rmSync(profile, { recursive: true, force: true });
        port += 1;
      }
    }
  }
}
