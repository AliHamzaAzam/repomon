---
name: Repomon desktop
description: The incumbent visual system for operating coding agents and reading their output.
colors:
  background: "var(--background)"
  foreground: "var(--foreground)"
  surface: "var(--surface)"
  raised: "var(--raised)"
  line: "var(--line)"
  muted: "var(--muted)"
  signal: "var(--signal)"
  attention: "var(--attention)"
  fault: "var(--fault)"
typography:
  body:
    fontFamily: "var(--font-sans)"
    fontSize: "13px"
    lineHeight: 1.45
  title:
    fontFamily: "var(--font-sans)"
    fontSize: "12px"
    fontWeight: 600
  transcript:
    fontFamily: "var(--font-sans)"
    fontSize: "13px"
    lineHeight: 1.7
  metadata:
    fontFamily: "var(--font-mono)"
    fontSize: "10px"
    lineHeight: 1.5
components:
  button-primary:
    backgroundColor: "{colors.signal}"
    textColor: "{colors.background}"
  button-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.foreground}"
  reply-field:
    backgroundColor: "{colors.raised}"
    textColor: "{colors.foreground}"
  view-toggle:
    backgroundColor: "{colors.raised}"
  status-chip:
    textColor: "{colors.signal}"
  settings-card:
    textColor: "{colors.foreground}"
  settings-field:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.foreground}"
---

# Design System: Repomon desktop

## Overview

**Creative North Star: "Order carries attention"**

Repomon keeps agent work in a dense, quiet desktop workspace. Home and Settings prioritize operating the fleet; Conversation combines a readable transcript ledger with the existing terminal and reply controls. The approved home and conversation references retain the incumbent chrome, with repo-leading home rows and bounded reading widths.

Authority: [PRODUCT.md](PRODUCT.md), [index.css](src/index.css), the built HomeScreen, ConversationPane, TerminalPane, SettingsModal, and controls. The approved reference paths are recorded in PRODUCT.md. This is a record of the incumbent system, not a replacement visual direction.

**Key Characteristics:**

- Semantic theme colors and existing SVG controls.
- Compact operational rows with explicit state words.
- A transcript ledger with a persistent action footer.

## Colors

The frontmatter references the live CSS properties so every existing theme and operator customization remains authoritative. Resolve their actual HSL values from index.css; do not freeze light-theme values into new components.

Primary: `signal` marks active work, enabled actions, and keyboard focus. `attention` marks a pending decision; `fault` identifies failed actions and errors. These are semantic roles, not interchangeable decoration.

Neutral: `background` grounds the workspace, `surface` holds controls and pane chrome, `raised` distinguishes inset or hovered areas, `line` separates rows, `foreground` carries content, and `muted` carries secondary metadata. Existing pane accents and terminal ANSI colors remain owned by their source tokens.

**The Zero Urgency Rule.** When a synced fleet has lanes and none needs attention, show the quiet summary and retain the recessed separator. Do not manufacture an urgent row.

## Typography

Use the existing `--font-sans` system stack for interface text and transcript prose, and `--font-mono` for branches, timestamps, model metadata, tool output, and terminal content. No display scale is introduced.

Pane titles and controls are compact; repository names use semibold text, accepted home headlines use the slightly larger body treatment, and branch fallbacks use muted mono. Transcript prose has more line spacing than operational chrome. Model labels sit below message content, secondary to the short speaker label.

**The Identity Rule.** Home leads with the repository, then the accepted headline or branch fallback. Repeated repository/title pairs retain the lane identifier.

## Layout

Home uses a left-aligned reading column capped at (64rem). Compact strips align the status icon, identity, explicit state, and age; ordinary rows use vertical padding (10px), with extra room for an inline blocking question. At widths up to (1100px), repository and state columns narrow while retaining the same order.

Conversation keeps the existing app tabs above a pane header (40px) containing task identity, repository, and adjacent Terminal/Chat and Summary/Normal/Verbose controls. Its scrollable ledger is bounded at (960px), with a metadata gutter (108px), gap (14px), and prose capped at (76ch). Time and short speaker occupy parallel gutter columns. At widths up to (1100px), ledger padding and body size tighten.

The footer stays outside the scrollable ledger. Its terminal tail or pending decision sits above the reply row; the field and send action share a bounded area (960px). Settings uses the existing centered modal, independently scrolling body, sticky section tabs, and persistent footer. Short windows constrain modal height rather than hiding actions. The existing multi-pane layout and app tabs are preserved.

## Elevation & Depth

Home strips and transcript rows use tonal surfaces and fine rules. Existing settings cards, menus, and modal shells retain their established depth; this is not an app-wide ban on shadows. Menu and modal shadows use `--shadow`; their exact recipes and focus treatments live in the sidecar. Reduced-motion preferences suppress animation and transition duration through the existing global rule.

## Shapes

Keep the flat strip and ledger geometry. Reuse small rounded controls, larger rounded settings cards and modal shells, and the existing pill switch. Do not give transcript messages the card silhouette used by separate settings and history surfaces. Borders use `--line`; focus and semantic state can change the boundary color.

## Components

- **Buttons and fields:** retain the existing primary signal action, bordered surface action, and quiet icon action. Hover clarifies the surface or foreground; keyboard focus uses the signal boundary. Disabled actions remain visibly disabled. Settings fields keep their labels and compact dimensions.
- **View and detail controls:** reuse the existing segmented components. A selected segment has a surface ground and foreground text inside a raised bordered group, with `aria-pressed` exposing selection. Unsupported agent defaults remain disabled with the visible Terminal only explanation.
- **Home strips:** the whole row is a keyboard-focusable action. Use existing state icons with explicit words, truncate long secondary identity without losing the repository, and retain the inline attention question and recessed separator.
- **Transcript ledger:** use the same content column for prose, raw monospace fallback, and tool rows. Tool rows have a quiet bottom rule and no repeated time/tool gutter. Summary hides tool rows; Normal collapses them by default; Verbose opens them by default. Manual expansion reveals raw results or the existing diff renderer. Partial output has the visible Writing state.
- **Conversation footer:** the ordinary terminal preview opens Terminal. A pending decision replaces it with an attention-colored top rule, the title incorporated into the mono question, and adjacent wrapping choices. The raised reply field stays visible and explains its disabled state until the decision is answered; its focus boundary uses signal.
- **Settings and icons:** keep the existing modal, horizontal section tabs, cards, Select, Switch, and ColorField. SVG paths come from the incumbent icon library and inherit current color; controls supply accessible names. Reuse these components rather than restyling each instance.

## Do's and Don'ts

### Do:

- Do reuse the existing CSS custom properties, control components, and SVG icon library.
- Do keep state words visible alongside color and preserve focus, selected, and disabled states.
- Do retain the existing app tabs and mounted Terminal/Chat session when changing views.
- Do check light and dark themes at 1040x680, 1440x900, and 2000x1000.

### Don't:

- Don't introduce a new palette, font system, decorative assets, emoji, or glyph icons.
- Don't depend on a rich headline, model label, timestamp, or valid Markdown for readable identity and content.
- Don't stretch home rows or the reply action across the full width of a large window.
