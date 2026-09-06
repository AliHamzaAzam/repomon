/// Builds printable shortcut HTML from the shared registry and selected platform.

import { BINDINGS, formatChord, isMac, type Binding, type KeymapSection } from "./keymap";

const SECTION_ORDER: KeymapSection[] = ["Panels", "Layout", "Fleet", "Lane", "Agents", "Terminal", "Editor", "Help"];

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function scopeLabel(binding: Binding): string {
  const scope = binding.scope ?? "global";
  return scope === "global" ? "" : ` (${scope} only)`;
}

export function buildPrintableShortcutsHtml(bindings: Binding[] = BINDINGS, platform?: string): string {
  const mac = isMac(platform);
  const sections = SECTION_ORDER.map((section) => ({
    section,
    rows: bindings.filter((binding) => binding.section === section),
  })).filter(({ rows }) => rows.length > 0);

  const body = sections
    .map(
      ({ section, rows }) => `
        <section>
          <h2>${escapeHtml(section)}</h2>
          <table>
            <thead><tr><th>Shortcut</th><th>Action</th></tr></thead>
            <tbody>
              ${rows
                .map(
                  (binding) => `
              <tr>
                <td class="chord">${escapeHtml(formatChord(binding.chord, platform))}</td>
                <td>${escapeHtml(binding.label)}${escapeHtml(scopeLabel(binding))}</td>
              </tr>`,
                )
                .join("")}
            </tbody>
          </table>
        </section>`,
    )
    .join("");

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<title>Repomon keyboard shortcuts</title>
<style>
  /* Standalone printouts use fixed neutral colors because app theme tokens are unavailable. */
  body { font-family: -apple-system, "Segoe UI", sans-serif; color: rgb(26 26 26); background: rgb(255 255 255); margin: 2rem; }
  h1 { font-size: 1.25rem; margin-bottom: 0.25rem; }
  p.sub { color: rgb(85 85 85); margin-top: 0; margin-bottom: 1.5rem; font-size: 0.85rem; }
  section { break-inside: avoid; margin-bottom: 1.25rem; }
  h2 { font-size: 0.95rem; text-transform: uppercase; letter-spacing: 0.04em; color: rgb(85 85 85); border-bottom: 1px solid rgb(204 204 204); padding-bottom: 0.25rem; }
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  td, th { text-align: left; padding: 0.25rem 0.5rem 0.25rem 0; }
  td.chord { font-family: "SF Mono", "Cascadia Code", monospace; white-space: nowrap; width: 1%; }
  thead { display: none; }
  @media print { body { margin: 0.5in; } }
</style>
</head>
<body>
  <h1>Repomon keyboard shortcuts</h1>
  <p class="sub">mod is ${mac ? "Cmd" : "Ctrl"} on this reference. Rows marked "(editor only)" or "(terminal only)" only fire while that surface has focus.</p>
  ${body}
</body>
</html>`;
}

export function shortcutsHtmlDataUrl(bindings: Binding[] = BINDINGS, platform?: string): string {
  const html = buildPrintableShortcutsHtml(bindings, platform);
  const encoded =
    typeof btoa === "function"
      ? btoa(unescape(encodeURIComponent(html)))
      : Buffer.from(html, "utf8").toString("base64");
  return `data:text/html;base64,${encoded}`;
}
