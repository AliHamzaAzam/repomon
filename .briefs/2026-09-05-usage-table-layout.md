# Brief U5: Usage view sessions table, spacing and polish (impeccable pass)

Operator request (2026-09-05, screenshot of the live sessions table): "improve the spacing here,
do an /impeccable pass". Run `/frontend-design`, then `/impeccable layout` on the table, then
`/impeccable polish` on the whole Usage view, and apply them. Operate mode: scanability first.

Branch: `fix/usage-table-layout` in a fresh worktree at `/private/tmp/repomon-fix-usage-table`
from current main. Create it with `git -C /Users/azaleas/Developer/Claude/repomon worktree add
/private/tmp/repomon-fix-usage-table -b fix/usage-table-layout main` if it does not exist. This
brief is the complete scope. Two other agents are editing `UsageView.tsx` in small, separate
regions (the unpriced warning near the group table, and a one-line recount notice under the view
header); stay out of those two spots so the merge is trivial. Do not touch SettingsModal.tsx or
FleetSidebar.tsx.

## What is wrong in the screenshot (`apps/desktop/src/components/UsageView.tsx`, sessions table
near line 454, row component near line 640)

1. No horizontal gutter between cells: "claude-fable-5-1" runs straight into
   "AliHamzaAzam/AliHamzaAzam", which runs straight into "11798". Cells have `py-1` and no `px`.
2. Fixed widths are too tight for real content under `table-fixed`: Agent `w-24` cannot hold
   "claude-fable-5-1"; Turns/Tools `w-12` cannot hold five digits; Lane `w-40` truncates every
   worktree lane ("repomon/repomon-fix-stat...") while the Task column keeps most of the width.
3. Numbers are not aligned as a column set: Cost mixes "$357.5", "$89.34", "$0.0053", "$0.01"
   (the money formatter should give two decimals below $100 and never four; a sub-cent cost reads
   "<$0.01"); tokens "1.4B / 48.3M / 20.4k" are fine but need `tabular-nums` and a consistent
   right edge; Time is empty for one-turn sessions (show "<1m" or an en-dash equivalent drawn as
   text "-", muted).
4. Retries in attention color for any non-zero count draws the eye to "2" as much as "38"; scale
   it (muted for 1-2, attention above a threshold) or show it only when it matters.
5. The "unknown · external" fallback reads as a warning; make "external" a quiet chip or muted
   suffix, and give unknown lanes the cwd basename when there is one.
6. Row rhythm: 50px rows with zebra at 30 percent and full-width hairlines feel heavy for a
   dense table; tighten to a consistent vertical rhythm, lighter separators, and clear hover.
7. The header should read as a header (letter-spaced small caps or the app's existing table
   header treatment, whichever the group table above already uses), sortable columns with a
   visible sort indicator, and the sticky header must not show rows through it.

## Do

- Column plan with real content in mind: Task flexible with a sensible minimum, Agent sized to the
  longest model id in use (or truncated from the middle so the family and version both survive),
  Lane truncated from the start of the path so the lane name survives ("...fix-status-v2"), numeric
  columns sized to their widest realistic value, Cost the widest fixed money column. Use the
  existing responsive column drop order (Sub, Tools, Retries give way first) and keep it working.
- Every cell gets a horizontal gutter; numeric cells `tabular-nums` right aligned; text cells left.
- One money formatter, one token formatter, one duration formatter (`usageMetrics.ts`), applied
  everywhere in the view and the CLI table if the CLI shares them.
- Row expand panel (model breakdown) aligned to the new column grid.
- Apply the same gutter and number discipline to the "Where it went" group table above.
- Tests: formatter cases (sub-cent, two decimals under $100, whole dollars above $1,000), the
  column drop order still passes, a row with a long model id and a long lane path renders both
  without overlap (assert on classes or measured widths in jsdom as the existing tests do).
- Before/after screenshots of the table at 1200px and 900px wide, saved under `qa/` in the
  worktree (untracked), using the existing Vitest browser or Playwright setup if present; if no
  screenshot tooling exists, say so and describe the checks you could do.

## Rules and gate

Worktree only; never edit the main checkout; never bind a daemon to `/tmp/repomon-azaleas.sock`
or copy the production database; never kill processes by name pattern; never `git add -A`; tests
use fixtures and mocked RPCs. Zero hex colors (CSS variables only), no emoji (SVG icons), no
em-dashes in code, comments, or copy. Gate: in `apps/desktop` `bun run check`, `bun run test`,
`bun run bindings:check`; `cargo test -p repomon-tui` if the CLI formatter changes. Commits:
1-line Conventional Commits, no co-author trailer. Do not merge, push, or build the bundle. Report
commit hashes, what changed per numbered item, test names, gate tails, and what could not be
verified without a live screenshot.
