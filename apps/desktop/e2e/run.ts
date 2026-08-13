import { remote } from "webdriverio";

const application = process.env.REPOMON_DESKTOP_BIN;
if (!application) throw new Error("REPOMON_DESKTOP_BIN is required");
const screenshot = process.env.REPOMON_E2E_SCREENSHOT;

const terminalText = (selector = ".terminal-host") => browser.execute(
  (terminalSelector) => document.querySelector(terminalSelector)?.textContent ?? "",
  selector,
);

const browser = await remote({
  hostname: "127.0.0.1",
  port: 4444,
  logLevel: "error",
  capabilities: {
    "tauri:options": { application },
  },
});

try {
  const heading = await browser.$("h1");
  await heading.waitForDisplayed({ timeout: 15_000 });
  if ((await heading.getText()) !== "Repomon") throw new Error("mission-control heading missing");

  await browser.$(".status-light.is-connected").waitForExist({ timeout: 15_000 });
  const fleet = await browser.$("[aria-label='Fleet']");
  await fleet.waitForDisplayed();
  await browser.$(".fleet-row").waitForDisplayed({ timeout: 15_000 });

  await browser.execute(() => localStorage.setItem("repomon.terminal.renderer", "dom"));
  await browser.refresh();
  await browser.$("h1").waitForDisplayed({ timeout: 15_000 });
  await browser.$(".status-light.is-connected").waitForExist({ timeout: 15_000 });
  await browser.$(".fleet-row").waitForDisplayed({ timeout: 15_000 });
  const shellButton = await browser.$("button=+ shell");
  await shellButton.waitForEnabled({ timeout: 10_000 });
  await shellButton.click();
  await browser.$(".terminal-host .xterm").waitForDisplayed({ timeout: 15_000 });

  const input = await browser.$(".terminal-host .xterm-helper-textarea");
  await browser.waitUntil(async () => (await terminalText()).trim().length > 0, {
    timeout: 15_000,
    timeoutMsg: "interactive shell prompt did not render",
  });
  await input.click();
  await browser.keys("echo GUI_E2E_OK");
  await browser.keys("Enter");
  await browser.waitUntil(
    async () => (await terminalText()).includes("GUI_E2E_OK"),
    { timeout: 15_000, timeoutMsg: "interactive shell output did not return through xterm" },
  );

  await browser.execute(() => {
    document
      .querySelector(".terminal-layout > div:not(.warm-terminal-hidden)")
      ?.setAttribute("data-e2e-first-pane", "true");
  });
  await shellButton.click();
  await browser.waitUntil(
    async () => browser.execute(() => (
      document.querySelector("[data-e2e-first-pane='true'].warm-terminal-hidden") !== null
    )),
    { timeout: 15_000, timeoutMsg: "previous terminal was not retained in the warm cache" },
  );
  await browser.$("[aria-label='Lane terminals and actions'] button[aria-pressed='false']").click();
  await browser.waitUntil(
    async () => browser.execute(() => (
      document.querySelector("[data-e2e-first-pane='true']:not(.warm-terminal-hidden)") !== null
    )),
    { timeout: 15_000, timeoutMsg: "warm terminal did not become visible again" },
  );
  if (!(await terminalText("[data-e2e-first-pane='true'] .terminal-host")).includes("GUI_E2E_OK")) {
    throw new Error("warm terminal lost its rendered contents");
  }

  await browser.$("button*=Control").click();
  await browser.$("[role='dialog'][aria-label='Control center']").waitForDisplayed();
  await browser.$("button=triage").click();
  await browser.$("button=Close").click();
  await browser.$("[role='dialog'][aria-label='Control center']").waitForDisplayed({ reverse: true });
  if (screenshot) {
    await browser.saveScreenshot(screenshot);
  }
} catch (error) {
  if (screenshot) await browser.saveScreenshot(screenshot);
  throw error;
} finally {
  await browser.deleteSession();
}
