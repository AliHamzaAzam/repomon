#!/usr/bin/env node
// Reads the theme tokens in src/index.css and prints WCAG contrast ratios for every text and
// chart pairing the app relies on. Exit code 1 when any required pair misses its floor.
//
//   node qa/contrast.mjs            # every theme block
//   node qa/contrast.mjs .dark      # one block by selector
//
// Floors: 4.5:1 for body text tokens (foreground, muted, signal, attention, fault) against each
// ground (background, surface, raised); 3:1 for chart series against the chart surface and for
// the focus ring (signal) against the background it is drawn on.

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const css = readFileSync(path.resolve(here, "../src/index.css"), "utf-8");

const TEXT_TOKENS = ["--foreground", "--muted", "--signal", "--attention", "--fault"];
const GROUNDS = ["--background", "--surface", "--raised"];
const CHART_TOKENS = ["--chart-1", "--chart-2", "--chart-3", "--chart-4", "--chart-5", "--chart-6"];

function parseBlocks(source) {
  const blocks = new Map();
  const stripped = source.replace(/\/\*[\s\S]*?\*\//g, "");
  const re = /([^{}]*?)\s*\{([^{}]*)\}/g;
  let match;
  while ((match = re.exec(stripped))) {
    const selectors = match[1].split("\n").filter((line) => !line.trim().startsWith("@import")).join(" ").split(",").map((s) => s.trim()).filter(Boolean);
    const body = match[2];
    const vars = {};
    for (const line of body.split("\n")) {
      const m = line.match(/^\s*(--[a-z0-9-]+)\s*:\s*([^;]+);/i);
      if (m) vars[m[1]] = m[2].trim();
    }
    if (Object.keys(vars).length === 0) continue;
    for (const selector of selectors) {
      blocks.set(selector, { ...(blocks.get(selector) ?? {}), ...vars });
    }
  }
  return blocks;
}

function hslToRgb(h, s, l) {
  s /= 100;
  l /= 100;
  const k = (n) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return [f(0) * 255, f(8) * 255, f(4) * 255];
}

function parseColor(value) {
  const hsl = value.match(/^hsl\(\s*([\d.]+)\s+([\d.]+)%\s+([\d.]+)%/i);
  if (hsl) return hslToRgb(Number(hsl[1]), Number(hsl[2]), Number(hsl[3]));
  const hex = value.match(/^#([0-9a-f]{6})$/i);
  if (hex) {
    const n = parseInt(hex[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  return null;
}

function luminance([r, g, b]) {
  const lin = (c) => {
    const v = c / 255;
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function ratio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const blocks = parseBlocks(css);
const root = blocks.get(":root") ?? {};
const only = process.argv[2];
let failures = 0;

for (const [selector, vars] of blocks) {
  if (only && selector !== only) continue;
  if (!vars["--background"] && !vars["--chart-1"]) continue;
  // A theme inherits :root, and every dark theme also inherits the shared dark chart steps.
  const resolved = { ...root, ...vars };
  const color = (name) => parseColor(resolved[name] ?? "");
  if (!color("--background")) continue;

  console.log(`\n${selector}`);
  const rows = [];
  for (const token of TEXT_TOKENS) {
    for (const ground of GROUNDS) {
      const a = color(token);
      const b = color(ground);
      if (!a || !b) continue;
      const r = ratio(a, b);
      const ok = r >= 4.5;
      if (!ok) failures += 1;
      rows.push(`${ok ? "ok  " : "FAIL"} ${r.toFixed(2).padStart(5)}  ${token} on ${ground}`);
    }
  }
  const surface = color("--surface");
  for (const token of CHART_TOKENS) {
    const a = color(token);
    if (!a || !surface) continue;
    const r = ratio(a, surface);
    const ok = r >= 3;
    if (!ok) failures += 1;
    rows.push(`${ok ? "ok  " : "FAIL"} ${r.toFixed(2).padStart(5)}  ${token} on --surface (3:1 floor)`);
  }
  const focus = ratio(color("--signal"), color("--background"));
  const focusOk = focus >= 3;
  if (!focusOk) failures += 1;
  rows.push(`${focusOk ? "ok  " : "FAIL"} ${focus.toFixed(2).padStart(5)}  focus ring (--signal) on --background (3:1 floor)`);
  console.log(rows.map((row) => `  ${row}`).join("\n"));
}

console.log(failures === 0 ? "\nAll required pairs pass." : `\n${failures} pair(s) below the floor.`);
process.exit(failures === 0 ? 0 : 1);
