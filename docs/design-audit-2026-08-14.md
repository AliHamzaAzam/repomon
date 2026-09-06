# Repomon Desktop GUI Design Audit & Polish Plan
**Date:** 2026-08-14  
**Target:** `apps/desktop` (SolidJS + Tailwind CSS v4, macOS Tauri Native App)  
**Mode:** Operate (Native Developer Tool / macOS App UI)  
**Quality Bar:** Linear / Raycast / Arc / macOS System Tool Craft Floor  

---

## 1. Executive Summary

Repomon's desktop frontend possesses a solid reactive architecture (SolidJS + xterm.js + WebGL) and high performance, but its visual presentation currently suffers from several "vibe-coded" / AI-slop anti-patterns:
1. **Unstable Tab & Agent Identifiers:** Tab headers display raw, truncated user chat prompt fragments (e.g. `"can you see the antig..."`, `"I t..."`) instead of clean, permanent process/lane identifiers.
2. **Redundant Stride of Brand Headers:** The Repomon product name is repeated 3 times in the top 200px (macOS menu bar, custom title bar, and a large "Repomon MISSION CONTROL" banner), wasting vertical space and creating visual noise.
3. **Typography & Font Stack Drift:** Non-macOS fonts (`"Aptos"`, `"Segoe UI Variable"`) are declared before system fonts; micro-point typography (`text-[0.52rem]` to `text-[0.65rem]` ~ 8–9.5px) is heavily overused.
4. **No Cohesive Icon System:** Raw Unicode characters (`⧉`, `◆`, `●`, `>_`, `×`, `⊘`, `⌕`, `↑`, `↓`) stand in for actual icons, rendering with inconsistent stroke weights and alignment across platforms.
5. **Forbidden AI Tropes:** Radial-masked grid line overlays on the terminal bay, colored 2px left border strips on message cards, and haphazard inline alpha colors (`bg-signal/10`, `border-attention/40`).
6. **Form Controls & Affordances:** Unstyled browser `<select>` dropdowns, jittery custom switch toggles, plain text button bars, and cluttered empty states.

**Audit Health Score:** **14/20** (Target after Phase 2: **19–20/20**)

---

## 2. Detailed Findings by Priority

### P0 — Blocking & Broken Affordances (Fix First)

#### [P0-1] Raw Truncated Chat Fragments in Tab Strip & Agent Labels
- **Location:** [`apps/desktop/src/components/agentLabel.ts:19-21`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/agentLabel.ts#L19-L21), [`apps/desktop/src/components/TerminalWorkspace.tsx:130-153`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/TerminalWorkspace.tsx#L130-L153), [`apps/desktop/src/components/FleetSidebar.tsx:27-29`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/FleetSidebar.tsx#L27-L29).
- **Issue:** `agentLabel(session)` falls back to `session.title`, which is derived by transcript parsers as the first 60 characters of the prompt. This causes tabs and sidebar rows to display random sentence fragments.
- **Impact:** Breaks tool predictability, looks amateurish, and makes finding active windows confusing.
- **Remedy:** Format stable, intentional identifiers:
  - `agentLabel(session)`: return `session.custom_label ?? `${formatAgentKind(session.agent)} ${slotOf(session.tmux_window) ?? 1}``.
  - `LaneRow`: display worktree name or branch as primary title, with custom lane label if set.
  - Tab strip: render clean agent/shell tabs (`claude 1`, `shell 1`, `repomind`) with matching status dots.

---

### P1 — Structural Consistency, Tokens & Design System

#### [P1-1] Font Stack & Typography Scale Modernization
- **Location:** [`apps/desktop/src/index.css:40-42`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/index.css#L40-L42), across all UI components.
- **Issue:** Windows-first font stack (`"Aptos"`, `"Segoe UI Variable"`); proliferation of arbitrary micro font sizes (`0.52rem`, `0.55rem`, `0.58rem`, `0.61rem`, `0.64rem`).
- **Remedy:**
  - Standardize `--font-sans` to native Apple system fonts: `-apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", system-ui, sans-serif`.
  - Standardize `--font-mono` to: `ui-monospace, "SF Mono", "SFMono-Regular", Menlo, Monaco, "Cascadia Code", monospace`.
  - Implement a structured, accessible type scale:
    - `10px` (`text-[10px]`): small badges, status tags, micro mono metadata with `tracking-wider`.
    - `11px` (`text-[11px]`): secondary metadata, timestamps, shortcut badges.
    - `12px` (`text-xs`): tab labels, sidebar items, form labels, buttons.
    - `13px` (`text-[13px]`): primary body text, input fields, menu items.
    - `14px` (`text-sm`): section headers, modal subtitles, card titles.
    - `16px` (`text-base`): modal titles, drawer headers.
    - `18px` (`text-lg`): main view titles.

#### [P1-2] Unified SVG Icon System
- **Location:** Create [`apps/desktop/src/components/icons.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/icons.tsx); replace unicode symbols across all 15+ UI components.
- **Issue:** Unicode glyphs (`⧉`, `◆`, `●`, `>_`, `×`, `⊘`, `⌕`, `↑`, `↓`) look unaligned, platform-dependent, and low-fidelity.
- **Remedy:** Implement authored SVG icon primitives (16x16 / 14x14 / 12x12 with uniform 1.5px stroke):
  `IconSearch`, `IconPlus`, `IconClose`, `IconHide`, `IconTrash`, `IconPin`, `IconGitBranch`, `IconArrowUp`, `IconArrowDown`, `IconTerminal`, `IconBot`, `IconRefresh`, `IconPlay`, `IconStop`, `IconSettings`, `IconExtensions`, `IconSparkles`, `IconCommand`, `IconCheck`, `IconCopy`, `IconChevronRight`, `IconChevronDown`.

#### [P1-3] Removal of AI Cliché Tropes (Grid Lines & Colored Border Strips)
- **Location:** [`apps/desktop/src/index.css:299-310`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/index.css#L299-L310), [`apps/desktop/src/index.css:139-169`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/index.css#L139-L169).
- **Issue:** Decorative grid overlays (`.terminal-bay::before`) and colored 2px border-left strips on messages look like a generic dashboard template.
- **Remedy:** Remove grid background; use calm neutral surfaces with crisp hairline borders (`border-line`) and elevated background tokens (`--surface`, `--raised`).

#### [P1-4] Tab Strip & Terminal Bay Redesign
- **Location:** [`apps/desktop/src/components/TerminalWorkspace.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/TerminalWorkspace.tsx), [`apps/desktop/src/components/TerminalPane.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/TerminalPane.tsx).
- **Issue:** Pill buttons with dashed borders for `+ agent` / `+ shell`; cluttered layout controls; unstyled renderer select.
- **Remedy:**
  - Warp/Linear-style tab bar with active tab elevation, subtle indicator pips, close buttons on hover, and clear keyboard focus rings.
  - Segmented layout controls (Focused / Split / Grid) with SVG glyphs.
  - Polished empty state with actionable cards.

#### [P1-5] Eliminate Redundant Brand Chrome & Merge Top Titlebar
- **Location:** [`apps/desktop/src/App.tsx:260-315`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/App.tsx#L260-L315).
- **Issue:** Repomon name is repeated 3 times in the top vertical space: macOS menu bar, custom titlebar ("Repomon"), and a secondary in-app banner ("Repomon MISSION CONTROL"). This wastes ~45px of vertical screen real estate and creates redundant visual hierarchy.
- **Remedy:**
  - Merge the custom window header and top app bar into a single, high-density, unified macOS toolbar (height: 38px–40px).
  - Left: Sleek BrandMark + single "Repomon" text + compact status dot.
  - Center: Command Bar trigger (`⌘K Search / Actions`).
  - Right: Unified action icons (Settings, Extensions, Repomind toggle, Theme toggle).
  - Remove the repetitive secondary "Repomon MISSION CONTROL" banner entirely.

---

### P2 — Surface Polish, Modals & Component Refinement

#### [P2-1] Fleet Sidebar Polish & Interaction Design
- **Location:** [`apps/desktop/src/components/FleetSidebar.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/FleetSidebar.tsx).
- **Remedy:**
  - Permanent discoverable hover actions with SVG icons for new lane, hide repo, and remove repo.
  - Search bar with embedded icon and `⌘F` / `/` shortcut tag.
  - High-density lane rows with clean visual hierarchy, badge styling, and smooth hover/active states.
  - Integrated footer status widget with clean live metrics.

#### [P2-2] Control Center & Command Palette Transformation
- **Location:** [`apps/desktop/src/components/ControlCenter.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/ControlCenter.tsx).
- **Remedy:**
  - Modern modal layout with left navigation tabs featuring icons and unread count badges.
  - Replace raw HTML buttons with structured action cards (title, subtitle, keyboard shortcut, icon).
  - Add polished search inputs, triage lists, and formatted diff/journal cards.

#### [P2-3] Form Controls Modernization (Switch, Select, ColorField, Modals)
- **Location:** [`apps/desktop/src/components/controls/Switch.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/controls/Switch.tsx), [`Select.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/controls/Select.tsx), [`SettingsModal.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/SettingsModal.tsx), [`Modal.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/components/Modal.tsx).
- **Remedy:**
  - Re-engineer `Switch` to native macOS proportions with smooth transitions and keyboard accessibility.
  - Re-engineer `Select` with a custom styled container, SVG chevron, and proper hover/focus states.
  - Group settings into clean rows with right-aligned controls and helpful explanatory text.

#### [P2-4] Top App Header & Footer Rail Polish
- **Location:** [`apps/desktop/src/App.tsx`](file:///Users/azaleas/Developer/Claude/repomon/apps/desktop/src/App.tsx).
- **Remedy:**
  - Top header: BrandMark, quick command palette button (`⌘K`), clean icon-supported nav buttons, and polished theme toggle.
  - Footer rail: Clean status indicator, version badges, repo/lane counters, and uptime metrics in a balanced layout.

---

## 3. Implementation Order (Phase 2 Plan)

1. **Step 1:** Fix tab titles and agent labels in `agentLabel.ts`, `agentLabel.test.ts`, `FleetSidebar.tsx`, and `TerminalWorkspace.tsx`.
2. **Step 2:** Build the comprehensive SVG icon system in `src/components/icons.tsx`.
3. **Step 3:** Overhaul typography scale, font tokens, and remove AI cliché grid lines in `index.css`.
4. **Step 4:** Modernize form controls (`Switch.tsx`, `Select.tsx`, `ColorField.tsx`, `Modal.tsx`).
5. **Step 5:** Polish the Fleet Sidebar (`FleetSidebar.tsx`, `RepoExtMenu.tsx`).
6. **Step 6:** Redesign the Tab Strip & Terminal Workspace (`TerminalWorkspace.tsx`, `TerminalPane.tsx`).
7. **Step 7:** Overhaul the Control Center & Action Modals (`ControlCenter.tsx`, `ActionModals.tsx`, `SettingsModal.tsx`, `SpawnModal.tsx`, `NewLaneModal.tsx`, `RenameModal.tsx`, `RepoNotesModal.tsx`, `ConfirmDialog.tsx`).
8. **Step 8:** Polish Extensions View, Drawer, Repomind Panel, and App Shell (`ExtensionsView.tsx`, `ExtensionDrawer.tsx`, `RepomindPanel.tsx`, `App.tsx`).
9. **Step 9:** Full verification with TypeScript typecheck (`tsc --noEmit`), Vitest suite (`bun test`), and Rust workspace tests.
