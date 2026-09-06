# Brief U2: Usage view second pass, toolbar numbering, Automation consolidation

Run `/frontend-design` and `/impeccable` before touching UI and apply them throughout. Observations
come from the operator's live app on 2026-09-05.

Branch: `feat/usage-view-v2` in a fresh worktree at `/private/tmp/repomon-feat-usage-v2` from
current main.

## A. Usage view (`apps/desktop/src/components/UsageView.tsx` and friends, merged at 21e5b1c)

1. **Range picking.** Today, 7 days, 30 days only. Add a custom range with a date picker (start
   and end day; a small calendar popover in the app's style, keyboard navigable), presets "This
   month" and "Last month", and bar-click narrowing: clicking a day bar shows that day with hour
   buckets, an hour bar shows 15-minute buckets; a breadcrumb restores the previous range. The
   header shows the resolved dates. The RPCs already accept a custom `from`/`to`.
2. **Sparse buckets stretch the bars.** With three hour buckets in view the bars are drawn a
   band wide (the operator's screenshot shows three fat pills across the whole width). Render
   every bucket in the range, including empty ones, so the axis is continuous, and cap the bar
   width (about 28px max) centered in its band with a minimum gap; the hover crosshair and
   tooltip cover empty buckets too ("no usage").
3. **Number formatting.** "12580.0M tokens" must read "12.6B"; one formatter for tokens (k, M,
   B, one decimal, no trailing ".0"), one for money (whole dollars above $1,000, cents below
   $100), one for durations ("3h 0m" becomes "3h"). Apply to tiles, tables, tooltips, and the
   CLI.
4. **Findings name sessions by UUID.** Use the session's task headline (or "untitled session"
   plus lane) and link the finding to the session row; fold repeats of the same shape into one
   line with a count and a total.
5. **Session headlines carry injected text** (`<local-command-caveat>...`, `<USER_REQUEST>`,
   slash commands). The extractor skips system-injected blocks (`<local-command-caveat>`,
   `<system-reminder>`, `<USER_REQUEST>`, `<task-notification>`, `<agent-message>`, lines
   starting with `/`), uses the first real user sentence trimmed to 80 chars, then the first
   assistant sentence, then "untitled session"; raw text in a tooltip. Daemon side
   (`usage_ledger/scan.rs` headline logic) with fixture tests on the exact strings.
6. **Lane labels.** Worktree lanes appear as bare paths; show `repo/lane-name` like the sidebar,
   path in the tooltip, `repo (lane removed)` when the lane is gone.
7. **Unpriced models.** Ids ending in `-free` (OpenCode free tier) price at zero without a
   warning; the warning for genuinely unknown paid models reads "No published rate for X.
   Tokens are counted; cost shows as $0 until you set a price in Settings." with a link.
8. **Design.** Tiles with hierarchy (cost headline, tokens and cache rate secondary, turns
   tertiary); chart y-axis in money or tokens with a toggle, gridlines at sensible steps,
   weekend shading on day buckets, crosshair, legend as a filter (click isolates a series);
   "Where it went" and "What to look at" as two cards with consistent headers; sessions table
   with sticky headers, sortable columns (cost, tokens, time, retries), zebra rows, row expand
   showing the model breakdown and lane/window; the subscription-pricing disclaimer as a muted
   footnote. CSS variables and the dataviz palette only.

## B. Toolbar order and numbered chords

9. Move Usage between Editor and Control. Final order: Git, Editor, Usage, Control,
   Multitasking, Extensions, Supervision, Repomail, Repomind, Settings.
10. Numbered chords follow that order: `mod+1` Git, `mod+2` Editor, `mod+3` Usage, `mod+4`
    Control, `mod+5` Multitasking, `mod+6` Extensions, `mod+7` Supervision, `mod+8` Repomail,
    `mod+9` Repomind, `mod+,` Settings. Theme cycling (currently `mod+6`) moves to
    `mod+shift+t`; `mod+k` stays as an alias for Control (it is the command palette). Update
    keymap.ts (single registry), the docs tables via `apps/desktop/scripts/print-shortcuts-doc.ts`
    (the consistency test must pass), tooltips (they derive from the registry), the wizard's
    Done step copy if it names a chord, and README mentions.
11. The command palette (Control) and the shortcuts overlay list the `mod+<number>` commands in
    numeric order.

## C. Settings > Automation consolidation

12. Settings > Automation becomes **Policies** with two sub-tabs: Approvals and Supervision
    (defaults). Playbooks, Schedules, and the Activity Journal move out: the Repomind panel
    already has Playbooks and Standing duties; give Standing duties an inline "Add duty" form
    (spec, goal, cap, the same validation as today, the same `schedule.add` RPC) instead of
    jumping to Settings, and add a Journal browser link from the panel's Memory health section
    (already there). Keep a one-line pointer in Settings ("Playbooks, standing duties, and the
    journal live in the Repomind panel, mod+9") so nobody hunts for them. Update
    `openSettingsTab` callers, docs/desktop.md, and tests.

## Rules and gate

Worktree only; never edit the main checkout; never bind a daemon to `/tmp/repomon-azaleas.sock`
or copy the production database; never kill processes by name pattern; never `git add -A`;
tests use fixtures and mocked RPCs. Zero hex colors, no emoji, no em-dashes. Gate: `bun run
check`, `bun run test`, `bun run bindings:check` in `apps/desktop`; `cargo test -p repomon-core
-p repomon-daemon -p repomon-tui` for the headline extractor, pricing, and CLI formatter
changes. Commits: 1-line Conventional Commits per numbered item where sensible, no co-author
trailer. Do not merge, push, or build the bundle. Report commit hashes, per-item summary, test
names, gate tails, and what could not be verified without a screenshot.
