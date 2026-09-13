/// Captures the Settings > Alert Notifications panel, where eleven on/off Switches sit in one
/// grid, against the screenshot fixture rather than a live daemon. The grid is the real case for
/// this control: a single switch in isolation proves nothing about eleven of them.
///
/// It also measures the built result. The knob's clearance is read off getBoundingClientRect in
/// the same render that produces the images, so the arithmetic in index.css is checked against
/// what the browser actually laid out rather than against itself.
///
/// Start the fixture server first:
///   bunx --bun vite --config vite.screenshot.config.ts --port 4181
/// then: bun scripts/screenshot-switch.ts
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const ORIGIN = process.env.REPOMON_SHOT_ORIGIN ?? "http://127.0.0.1:4181";
const OUT = resolve(import.meta.dir, "../../../qa", process.env.REPOMON_SCREENSHOT_OUTPUT ?? "switch-geometry");
const SIZES: Array<[number, number]> = [[1440, 900], [1040, 680]];
const THEMES = ["light", "dark"] as const;
/// One switch stays off, the way the operator's own screenshot had it.
const LEAVE_OFF = "Coalesce bursts";

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

  async evaluate<T>(expression: string): Promise<T> {
    const { result, exceptionDetails } = await this.send("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? "evaluate failed");
    return result.value as T;
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
  for (let attempt = 0; attempt < 200; attempt++) {
    if (await session.evaluate<boolean>(`document.querySelector(${JSON.stringify(selector)}) !== null`)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${selector}`);
}

/// Open Settings, land on Notifications, turn the master on so the grid is live, then switch all
/// but one of the rest on. Everything goes through the control's own click handler and the
/// fixture's config.set, so the panel on screen is the one the operator would reach.
const ARRANGE = `(async () => {
  const settle = () => new Promise((done) => setTimeout(done, 120));
  document.querySelector('button[aria-label="Settings"]').click();
  await settle();
  [...document.querySelectorAll('[role="tab"]')].find((tab) => tab.textContent.trim() === "Notifications").click();
  await settle();
  const master = document.querySelector('[role="switch"][aria-label="Enable notifications"]');
  if (master.getAttribute("aria-checked") === "false") master.click();
  await settle();
  for (const control of document.querySelectorAll('[role="switch"]')) {
    const label = control.getAttribute("aria-label");
    const on = control.getAttribute("aria-checked") === "true";
    if (label === ${JSON.stringify(LEAVE_OFF)}) { if (on) control.click(); continue; }
    if (!on) control.click();
    await settle();
  }
  document.querySelector('.section-label').scrollIntoView({ block: "start" });
  await settle();
  return document.querySelectorAll('[role="switch"]').length;
})()`;

/// The clearance between the knob and each inner edge of its track, off the laid-out boxes.
const MEASURE = `(() => {
  const read = (state) => {
    const track = document.querySelector('.switch-track[aria-checked="' + state + '"]');
    const knob = track.querySelector(".switch-knob");
    const t = track.getBoundingClientRect();
    const k = knob.getBoundingClientRect();
    const round = (value) => Math.round(value * 100) / 100;
    return {
      label: track.getAttribute("aria-label"),
      track: { width: round(t.width), height: round(t.height) },
      knob: { width: round(k.width), height: round(k.height) },
      clearance: {
        top: round(k.top - t.top),
        right: round(t.right - k.right),
        bottom: round(t.bottom - k.bottom),
        left: round(k.left - t.left),
      },
    };
  };
  return { on: read("true"), off: read("false") };
})()`;

/// A magnified crop of one switch, with a little of its row around it so the knob can be judged
/// against the track rather than against the page.
const CROP = `(() => {
  const pad = 10;
  const box = (state) => {
    const rect = document.querySelector('.switch-track[aria-checked="' + state + '"]').getBoundingClientRect();
    return { x: rect.x - pad, y: rect.y - pad, width: rect.width + pad * 2, height: rect.height + pad * 2 };
  };
  return { on: box("true"), off: box("false") };
})()`;

mkdirSync(OUT, { recursive: true });
let port = 9400;
for (const theme of THEMES) {
  for (const [width, height] of SIZES) {
    const profile = mkdtempSync(join(tmpdir(), "repomon-switch-shot-"));
    const chrome = spawn(CHROME, [
      "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
      `--user-data-dir=${profile}`, "--hide-scrollbars", `--window-size=${width},${height}`,
      "--force-device-scale-factor=1", `--remote-debugging-port=${port}`, "about:blank",
    ], { stdio: "ignore" });
    try {
      const session = await connect(port);
      await session.send("Page.enable");
      await session.send("Runtime.enable");
      await session.send("Page.navigate", { url: `${ORIGIN}/?fleet=ordinary&theme=${theme}` });
      await waitFor(session, 'button[aria-label="Settings"]');
      await sleep(600);
      const switches = await session.evaluate<number>(ARRANGE);
      await sleep(400);

      const measured = await session.evaluate<Record<string, unknown>>(MEASURE);
      console.log(`${theme} ${width}x${height}: ${switches} switches`, JSON.stringify(measured));

      const panel = await session.send("Page.captureScreenshot", { format: "png" });
      const panelPath = join(OUT, `alert-notifications-${theme}-${width}x${height}.png`);
      writeFileSync(panelPath, Buffer.from(panel.data, "base64"));
      console.log(panelPath);

      const crops = await session.evaluate<Record<"on" | "off", { x: number; y: number; width: number; height: number }>>(CROP);
      for (const state of ["on", "off"] as const) {
        const shot = await session.send("Page.captureScreenshot", {
          format: "png",
          clip: { ...crops[state], scale: 8 },
        });
        const cropPath = join(OUT, `knob-${state}-${theme}-${width}x${height}.png`);
        writeFileSync(cropPath, Buffer.from(shot.data, "base64"));
        console.log(cropPath);
      }
      session.close();
    } finally {
      chrome.kill();
      rmSync(profile, { recursive: true, force: true });
      port += 1;
    }
  }
}
