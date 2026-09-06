// Injected only into the disposable demo, before app scripts. No inspector port or UI access.
(() => {
  const send = body => window.webkit.messageHandlers.demoProbe.postMessage(body);
  const describe = value => value instanceof Error ? value.stack : String(value);
  for (const level of ["log", "warn", "error"]) {
    const original = console[level];
    console[level] = (...args) => {
      send({ event: "console", level, args: args.map(describe) });
      original.apply(console, args);
    };
  }
  addEventListener("error", e => send({ event: "error", message: e.message,
    stack: e.error?.stack, filename: e.filename, line: e.lineno }));
  addEventListener("unhandledrejection", e => send({ event: "unhandledrejection", reason: describe(e.reason) }));
  addEventListener("DOMContentLoaded", () => {
    try {
      localStorage.setItem("repomon:demo-storage-probe", "ok");
      send({ event: "localStorage", value: localStorage.getItem("repomon:demo-storage-probe") });
      localStorage.removeItem("repomon:demo-storage-probe");
    } catch (e) { send({ event: "storage-error", storage: "localStorage", error: describe(e) }); }
    try {
      const name = "repomon-demo-storage-probe";
      const request = indexedDB.open(name, 1);
      request.onupgradeneeded = () => request.result.createObjectStore("probe");
      request.onerror = () => send({ event: "storage-error", storage: "indexedDB", error: describe(request.error) });
      request.onsuccess = () => {
        const db = request.result;
        const tx = db.transaction("probe", "readwrite");
        tx.objectStore("probe").put("ok", "check");
        const read = tx.objectStore("probe").get("check");
        read.onsuccess = () => send({ event: "indexedDB", value: read.result });
        tx.oncomplete = () => { db.close(); indexedDB.deleteDatabase(name); };
        tx.onerror = () => send({ event: "storage-error", storage: "indexedDB", error: describe(tx.error) });
      };
    } catch (e) { send({ event: "storage-error", storage: "indexedDB", error: describe(e) }); }
    let tick = 0;
    const timer = setInterval(() => {
      const buttons = [...document.querySelectorAll("button")].map(button => {
        const rect = button.getBoundingClientRect();
        const style = getComputedStyle(button);
        let shown = rect.width > 0 && rect.height > 0 && style.visibility === "visible";
        for (let parent = button; parent; parent = parent.parentElement) {
          const css = getComputedStyle(parent);
          if (css.display === "none" || Number(css.opacity) === 0) shown = false;
        }
        return { text: button.innerText, label: button.getAttribute("aria-label"), shown,
          rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height } };
      });
      send({ event: "dom", tick: ++tick, buttons, viewport: { width: innerWidth, height: innerHeight } });
      if (tick >= 120) clearInterval(timer);
    }, 1000);
  });
})();
