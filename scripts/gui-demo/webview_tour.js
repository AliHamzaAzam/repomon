// Recorder-only DOM interactions in the copied WKWebView. Uses the app's ordinary controls.
(() => {
  // Ordinary persisted panel preference, scoped to this disposable WebKit store.
  localStorage.setItem("repomon:right-panel-width", "520");
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const shown = element => !!element && element.getClientRects().length > 0 && getComputedStyle(element).visibility !== "hidden";
  const text = element => (element.innerText || element.textContent || "").replace(/\s+/g, " ").trim();
  const buttons = (scope = document) => [...scope.querySelectorAll('button,[role="tab"],[role="radio"]')].filter(shown);
  const label = element => element.getAttribute("aria-label") || text(element);
  let phase = "", started = 0;
  const send = (event, details = {}) => window.webkit.messageHandlers.demoProbe.postMessage({event, phase, seconds: (performance.now() - started) / 1000, ...details});
  async function until(check, description, timeout = 15000) {
    const end = performance.now() + timeout;
    while (performance.now() < end) {
      const result = check();
      if (result) return result;
      await pause(150);
    }
    throw new Error(`Missing ${description}; visible controls: ${buttons().map(label).join(" | ")}; content: ${document.body.innerText}`);
  }
  async function click(name, {partial = false, scope = document} = {}) {
    const button = await until(() => buttons(scope).find(b => partial ? label(b).includes(name) : label(b) === name), name);
    button.focus(); button.click();
    await pause(400);
    return button;
  }
  async function requireText(value) { return until(() => text(document.body).toLowerCase().includes(value.toLowerCase()), value); }
  async function key(key, {meta = false, shift = false, code = ""} = {}) {
    (document.activeElement || document.body).dispatchEvent(new KeyboardEvent("keydown", {key, code, metaKey: meta, shiftKey: shift, bubbles: true, cancelable: true}));
    await pause(400);
  }
  const mod = digit => key(digit, {meta: true, code: `Digit${digit}`});
  async function hero() { await click("fix-nav-focus-trap", {partial: true}); }
  async function finder(file, open = true) {
    await key("p", {meta: true, code: "KeyP"});
    const input = await until(() => [...document.querySelectorAll('input')].find(i => shown(i) && /find|search.*file|file.*name/i.test(i.placeholder + " " + i.getAttribute("aria-label"))), "file finder input");
    input.focus(); input.value = file; input.dispatchEvent(new Event("input", {bubbles: true}));
    await requireText(file); await pause(500);
    if (open) await key("Enter", {code: "Enter"});
  }
  async function beat(name, seconds = 5) {
    send("tour-beat", {name});
    await pause(seconds * 1000);
  }
  async function opening() {
    await requireText("orbit-api");
    await requireText("5 repos"); await requireText("8 lanes");
    await hero(); await click("Focused layout");
    await requireText("Needs you"); await requireText("Running");
    await pause(1500);
    send("tour-beat", {name: "Opening hero"});
  }
  async function tour() {
    await beat("Fleet and fake Claude hero", 7);
    await mod("5");
    await click("Configure multitasking panes");
    const picker = await until(() => document.querySelector('[aria-label="Choose multitasking panes"]'), "pane picker");
    while (buttons(picker).filter(b => label(b).startsWith("Hide ")).length > 4) {
      const selected = buttons(picker).filter(b => label(b).startsWith("Hide "));
      selected[selected.length - 1].click(); await pause(250);
    }
    if (buttons(picker).filter(b => label(b).startsWith("Hide ")).length !== 4) throw new Error("Expected exactly four selected panes");
    const widths = buttons(picker).filter(b => label(b).endsWith(": two columns wide"));
    if (widths.length !== 4) throw new Error("Expected four pane width controls");
    widths[0].click(); await pause(200);
    widths[3].click(); await pause(200);
    await click("Configure multitasking panes");
    await until(() => !shown(document.querySelector('[aria-label="Choose multitasking panes"]')), "closed pane picker");
    await until(() => buttons().some(b => label(b) === "Configure multitasking panes" && text(b).includes("4")), "four-pane count");
    await beat("Multitasking: four panes", 7);
    await mod("5"); await hero(); await mod("1");
    await click("MobileMenu.tsx", {partial: true}); await requireText("Restore focus");
    await beat("Git: uncommitted navigation diff", 7); await mod("1");
    await click("Editor"); await finder("MobileMenu.tsx"); await requireText("MobileMenu.tsx");
    await beat("Editor workspace: TypeScript syntax", 6);
    await finder("useFocusTrap", false); await beat("File finder", 4); await key("Escape");
    await finder("navigation.svg"); await key("v", {meta: true, shift: true, code: "KeyV"});
    await until(() => { const img = document.querySelector('img[alt="SVG preview"]'); return shown(img) && img.complete && img.naturalWidth > 0; }, "loaded SVG preview");
    await beat("Image preview: navigation design", 6); await click("Editor");
    await mod("3"); await click("7 days"); await requireText("Sessions");
    await beat("Usage chart and cards", 6);
    const sessions = await until(() => [...document.querySelectorAll('h1,h2,h3,h4')].find(e => text(e).toLowerCase() === "sessions"), "sessions heading");
    sessions.scrollIntoView({block: "start", behavior: "smooth"});
    await beat("Usage: sessions", 4);
    await key(",", {meta: true});
    const settings = await until(() => document.querySelector('[role="dialog"]'), "settings dialog");
    await click("Usage", {scope: settings}); await requireText("Built-in");
    if (/fetch failed|operation not permitted/i.test(text(settings))) throw new Error("Unexpected price refresh error in offline fixture");
    await beat("Settings: model rates", 6); await key("Escape"); await mod("3");
    await mod("8"); await requireText("Rate-limit headers are ready");
    await beat("Repomail: coordinated fake lanes", 6);
    await mod("8"); await click("feat-rate-limit-headers", {partial: true}); await mod("7");
    await requireText("Activity log"); await requireText("hold");
    const activity = await until(() => [...document.querySelectorAll('h1,h2,h3,h4,p')].find(e => text(e).toLowerCase() === "activity log"), "activity heading");
    activity.scrollIntoView({block: "start", behavior: "smooth"});
    await beat("Supervision: held permission audit", 6); await mod("7");
    await mod("9"); await requireText("release-review");
    await beat("Repomind: two plans and a draft playbook", 6); await mod("9");
    await key("?", {meta: true, shift: true, code: "Slash"}); await requireText("Keyboard shortcuts");
    await beat("Shortcuts overlay", 4); await key("Escape"); await hero();
    await beat("Return to the opening hero", 6);
  }
  window.repomonDemoTour = {async run(next) {
    phase = next; started = performance.now();
    try { await (phase === "opening" ? opening() : tour()); send("tour-complete"); }
    catch (error) { send("tour-error", {message: error.message, stack: error.stack}); }
  }};
})();
