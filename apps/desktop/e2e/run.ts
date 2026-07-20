import { remote } from "webdriverio";

const application = process.env.REPOMON_DESKTOP_BIN;
if (!application) throw new Error("REPOMON_DESKTOP_BIN is required");

const browser = await remote({
  hostname: "127.0.0.1",
  port: 4444,
  logLevel: "error",
  capabilities: {
    browserName: "wry",
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
  const shellButton = await browser.$("button=+ shell");
  await shellButton.waitForEnabled({ timeout: 10_000 });
  await shellButton.click();
  await browser.$(".terminal-host .xterm").waitForDisplayed({ timeout: 15_000 });

  const input = await browser.$(".terminal-host .xterm-helper-textarea");
  await input.click();
  await browser.keys("printf GUI_E2E_OK");
  await browser.keys("Enter");
  await browser.waitUntil(
    async () => (await browser.$(".terminal-host").getText()).includes("GUI_E2E_OK"),
    { timeout: 15_000, timeoutMsg: "interactive shell output did not return through xterm" },
  );

  await browser.$("button*=Control").click();
  await browser.$("[role='dialog'][aria-label='Control center']").waitForDisplayed();
  await browser.$("button=triage").click();
  await browser.$("button=Close").click();
  await browser.$("[role='dialog'][aria-label='Control center']").waitForDisplayed({ reverse: true });
  if (process.env.REPOMON_E2E_SCREENSHOT) {
    await browser.saveScreenshot(process.env.REPOMON_E2E_SCREENSHOT);
  }
} finally {
  await browser.deleteSession();
}
