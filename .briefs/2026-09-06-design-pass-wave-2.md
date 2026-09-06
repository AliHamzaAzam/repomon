# Brief D2: design pass, wave 2 (three parallel agents)

Context: D1 (`design/responsive-brand-pass`, six commits, gate green, under review) made the
brand palette the default theme, moved every color to tokens, added the four breakpoints
(narrow <=1100, medium <=1280, wide >=1600, short <=720), the shared `.panel-header`, and the
accent focus ring. It verified the shell (sidebar, toolbar, terminal bay empty state, connection
rail) with screenshots but could NOT verify the view bodies, which need a daemon. Wave 2 covers
those bodies. Operator request (2026-09-06): "use fable agents for the design, 2-3".

Every agent: branch from the tip of `design/responsive-brand-pass` (`git -C
/Users/azaleas/Developer/Claude/repomon worktree add <path> -b <branch> design/responsive-brand-pass`).
Run `/frontend-design` and `/impeccable` (`audit`, `adapt`, `layout`, `polish`) before editing and
apply them within your slice. Operate mode. Read D1's diff first (`git log --oneline main..HEAD`
in your worktree, `git show` the six commits) so you build on its tokens, breakpoints,
`.panel-header`, and focus ring instead of inventing parallel ones. Verify at the four sizes; for
live screenshots run an ISOLATED daemon with the recipe in `apps/desktop/e2e/isolated.sh` (unique
tmux session, throwaway config, data dir and socket) and the Vite dev build pointed at it, never
the production socket; save screenshots under `qa/screens/` (untracked) and stop everything you
started by PID. Shared-file rule to keep the three branches mergeable: do not edit
`apps/desktop/src/index.css` token blocks or breakpoint rules; add slice-specific CSS at the END
of index.css under a comment header naming your slice, or in the component. Do not edit
`theme.ts`, `keymap.ts`, `App.tsx`, `WindowChrome.tsx`, or `FleetSidebar.tsx` unless your slice
names them. Tests where the codebase has patterns (jsdom, class contracts). Keep behavior
unchanged. Zero hex literals in components, no emoji, no em-dashes, 1-line Conventional Commits,
no co-author trailer, no merge/push/bundle, no cargo builds beyond `bun run bindings:check`.
Gate in `apps/desktop`: `bun run check`, `bun run test --maxWorkers=2`, `bun run bindings:check`.
Report: commits, per-view findings and fixes, tests, gate tails, screenshots produced, and what
still could not be verified.

## D2a: shell interiors and settings (branch `design/wave2-shell`, worktree `/private/tmp/repomon-design-2a`)
Fleet sidebar with real repos and lanes (row grammar, chips, pinned Repomind row, rate-limits
card, collapsed and narrow states), the terminal bay with panes (pane headers, multitasking grid
at 2, 4, 6 panes, the tall/row ratchet), the Multitasking view, Onboarding (all seven steps at
the four sizes, including the System step's cards and the Done step), and the Settings modal
(every tab: General, Appearance with theme cards, accent swatches and tint preview reflecting
the brand default, Policies, System with the CLI card and health rows, Usage with the rates
table, Remote, and any others), plus the shortcuts overlay and command palette. Files you own:
FleetSidebar.tsx and its rows, TerminalWorkspace/TerminalPane and multitasking components,
Onboarding.tsx, SettingsModal.tsx and its tab views (UsageSettingsView, PolicySettings,
SystemHealthView, CommandLineToolsCard, DaemonBootRow), ShortcutsOverlay, ControlCenter.

## D2b: work views (branch `design/wave2-work`, worktree `/private/tmp/repomon-design-2b`)
Usage view (header, range picker and calendar popover, chart with crosshair and legend, the two
cards, sessions table, expand rows, footnote), Git view (status, diff, commit surfaces), and the
Editor workspace (rail, tabs, CodeMirror chrome, file finder, project search panel, PDF viewer
toolbar, image viewer and SVG preview, diff view). Files you own: UsageView.tsx and friends,
UsageRangePicker, the Git view components, EditorWorkspace, FileEditorPanel, CodeEditor chrome,
FileFinder, ProjectSearchPanel, PdfViewer, ImageViewer, SvgPreview.

## D2c: agent surfaces (branch `design/wave2-agents`, worktree `/private/tmp/repomon-design-2c`)
Repomind panel (all sections: plans, playbooks, standing duties, memory health, controllers) and
its fullscreen mode, the Repomail panel and composer, the Supervision panel and audit list, the
Extensions view, the right panel host tabs and empty states, the spawn modal and action modals,
notifications and toasts. Files you own: RepomindPanel and its section components, RepomindRow,
Repomail components, SupervisionPanel, Extensions view, RightPanelHost, SpawnModal, ActionModals,
toast and notification components.

## Addition for D2a, 2026-09-06 04:10 (operator screenshots): the title bars

macOS: the header lockup repeated the app name shown in the menu bar; fixed on main in aaa9a73
(`BrandLockup` gains `markOnly`, App passes `isMac()`). Rebase onto main before touching these.

Windows: the VM shows a native title bar ("Repomon", minimize, maximize, close) stacked above the
app's own 35px header, two bars for one window. Make Windows match macOS with a single custom
bar: set `decorations: false` for Windows only (in `apps/desktop/src-tauri/tauri.preview.win.conf.json`
and `tauri.release.win.conf.json`, not the shared conf), mark the header as the drag region
(`data-tauri-drag-region` on the header's empty areas, never on buttons), draw minimize,
maximize/restore, and close controls at the right end of the header in the Windows caption
style (SVG icons, 46px wide hit targets, close turns fault-red on hover) wired to the window API
(`getCurrentWindow().minimize()`, `toggleMaximize()`, `close()`), support double-click on the
drag region to maximize, keep the traffic-light inset logic macOS-only, and on Linux leave native
decorations as they are. `WindowChrome.tsx` is in scope for this item. Tests: the controls
render only when the platform is Windows (inject the platform; do not sniff the user agent in
the component), the drag attribute is present on the header and absent on every button. Report
what still needs the operator's VM (snap layouts, the maximize double-click, the DPI scaling).
