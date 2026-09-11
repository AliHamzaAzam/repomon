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
  caption:
    fontFamily: "var(--font-sans)"
    fontSize: "11px"
    lineHeight: 1.5
components:
  button-primary:
    backgroundColor: "{colors.signal}"
    textColor: "{colors.background}"
  button-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.foreground}"
  reply-field:
    backgroundColor: "{colors.surface}"
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

Repomon keeps agent work in a dense, quiet desktop workspace. Home and Settings prioritize operating the fleet; Conversation gives assistant prose a continuous reading track, contains human turns on the right, and collects work behind one disclosure per turn. Wide panes earn repository context while reading widths stay bounded. The incumbent chrome, repo-leading strips, and transcript ledger remain authoritative.

Authority: [PRODUCT.md](PRODUCT.md), [index.css](src/index.css), the built HomeScreen, ConversationPane, ConversationContext, TerminalPane, SettingsModal, [conversation.css](src/components/conversation.css), and controls. The approved reference paths are recorded in PRODUCT.md. This is a record of the incumbent system, not a replacement visual direction.

**Key Characteristics:**

- Semantic theme colors and existing SVG controls.
- Compact operational rows with explicit state words.
- Assistant-led prose, grouped work, and a unified attachment composer.
- Useful wide context with compact pane fallbacks.

## Colors

The frontmatter references the live CSS properties so every existing theme and operator customization remains authoritative. Resolve their actual HSL values from index.css; do not freeze light-theme values into new components.

Primary: `signal` marks active work, enabled actions, and keyboard focus. `attention` marks a pending decision; `fault` identifies failed actions and errors. These are semantic roles, not interchangeable decoration.

Neutral: `background` grounds the workspace, `surface` holds controls and pane chrome, `raised` distinguishes inset or hovered areas, `line` separates rows, `foreground` carries content, and `muted` carries secondary metadata. Existing pane accents and terminal ANSI colors remain owned by their source tokens.

**The Zero Urgency Rule.** When a synced fleet has lanes and none needs attention, show the quiet summary and retain the recessed separator. Do not manufacture an urgent row.

## Typography

Use the existing `--font-sans` system stack for interface text and transcript prose, and `--font-mono` for branches, timestamps, model metadata, tool output, and terminal content. No display scale is introduced.

Pane titles and controls are compact; repository names use semibold text, accepted home headlines use the slightly larger body treatment, and branch fallbacks use muted mono. Transcript prose has more line spacing than operational chrome. The ledger gutter stacks time above a short speaker label; message model metadata is available in the speaker tooltip. The latest observed model appears in the composer. Its button opens the agent's native model controls in the same mounted terminal, with Back to chat returning to the preserved draft and transcript.

A muted sans caption step (11px) sits between metadata and body for compact secondary chrome that is not mono: the Latest output pill, the pagination boundary labels, the composer's observed agent/model name, and attachment chip filenames. These predate the conversation layout rework and stay as-is rather than moving to the body or metadata step, since either would change their established, still-correct visual weight.

**The Identity Rule.** Home leads with the repository, then the accepted headline or branch fallback. Repeated repository/title pairs retain the lane identifier.

## Layout

Home fills its available pane. At a pane width of (1200px), its recent-lane area becomes two compact columns beside a context rail (320px) for repositories, changed lanes, and pull requests. The attention area stays above the lane grid. Each wide lane strip stacks repository above headline or branch, retaining explicit state and age; its padding is (12px 20px). Below that pane breakpoint, lanes use a single column and pull requests return inline. Ordinary single-column strips use vertical padding (10px), with extra room for an inline blocking question. The existing viewport adjustment at (1100px) narrows repository and state columns.

Conversation keeps the existing app tabs and pane header. Terminal/Chat is the primary segmented control; transcript detail remains subordinate. Assistant prose and the composer share a centered body track capped at (760px). Both sides reserve a metadata gutter (68px) and gap (16px), inside outer padding (24px), for an overall maximum of (976px). Agent/time sits in the left gutter; user/time sits in the right gutter. Human bubbles align to the body track's right edge and remain capped at (90%) of its width. Tool disclosures use that same body track.

At a Conversation pane width of (1200px), the existing context rail (320px) shows environment, changes, files, agents, and pull requests. Below that threshold, compact repository context sits above the composer. At pane widths up to (800px), outer padding becomes (16px), gutters (44px), and gaps (12px). At widths up to (480px), those become (8px), (32px), and (8px), while metadata retains its (10px) size. These are container queries for split panes. Pending inputs sit above the composer in compact, right-aligned bubbles, with a scrollable queue capped at (160px); their labels remain outside the body track.

The footer remains outside the scrollable ledger. Its context or pending-decision strip and unified composer align to the shared body width cap (760px). An Open live terminal action leads to the mounted emulator; Chat chrome contains no raw terminal tail. Settings retains its centered modal, independently scrolling body, sticky section tabs, and persistent footer. Short windows constrain modal height rather than hiding actions. The existing multi-pane layout and app tabs are preserved.

**The Context Width Rule.** Spend wide pane space on compact lane columns and repository context while retaining a bounded transcript and composer.

## Elevation & Depth

Home strips and transcript rows use tonal surfaces and fine rules. Existing settings cards, menus, and modal shells retain their established depth; this is not an app-wide ban on shadows. Menu and modal shadows use `--shadow`; their exact recipes and focus treatments live in the sidecar. Reduced-motion preferences suppress animation and transition duration through the existing global rule.

## Shapes

Keep the flat strip and ledger geometry. Reuse small rounded controls, larger rounded settings cards and modal shells, and the existing pill switch. Assistant prose stays on the plain ledger ground. Human turns use a small rounded raised containment; malformed output and disclosed work use inset surface panels. These retain the compact geometry rather than inheriting settings-card silhouettes. Borders use `--line`; focus and semantic state can change the boundary color.

## Components

- **Buttons and fields:** retain the existing primary signal action, bordered surface action, and quiet icon action. Hover clarifies the surface or foreground; keyboard focus uses the signal boundary. Disabled actions remain visibly disabled. Settings fields keep their labels and compact dimensions.
- **View and detail controls:** Terminal/Chat reuses the existing segmented component, with surface-ground selection and `aria-pressed`. Transcript detail uses the existing frameless Select, with a muted trigger, keyboard selection, and a portaled listbox that escapes terminal stacking contexts. Unsupported agent defaults remain disabled with the visible Terminal only explanation. Changing representation preserves the mounted session.
- **Home strips:** the whole row is a keyboard-focusable action. Use existing state icons with explicit words, truncate long secondary identity without losing the repository, and retain the inline attention question and recessed separator. Keyboard navigation follows the visible lane-grid arrangement.
- **Transcript ledger and turn work:** prose and raw fallback share the body track. A user message or turn-start boundary begins a work group; tool calls and selected status notices share one expandable work row. Summary and Normal keep work collapsed by default; Verbose opens it. A closed work row still names failed tools and selected limit notices. Disclosed tools retain raw results or the existing diff renderer. Partial transcript output has the visible Writing state; fallback entries do not. Live pane output starts collapsed under Terminal excerpt with a line count, and malformed entries start collapsed under Unformatted entry. Expanding either reveals the preserved monospace content in a scrollable panel capped at (160px), independent of the transcript detail setting.
- **Turn notice selection:** absent an explicit per-agent override, Summary shows no notices, Normal selects rate and usage limits, and Verbose selects all five notice kinds: turn started, turn finished, turn cost, rate limit, and usage limit. Settings displays Follow detail level until a custom list is chosen. That list, stored under `agent_status_rows` through Config/config.set, overrides notice selection at every detail level; it does not change disclosure defaults. Notices appear inside work details, and daemon cost text appears there once without a second amount synthesized from metadata. The daemon's periodic token-count bookkeeping status (`turn_usage`, "Usage recorded") is not one of the five notice kinds and is never selectable or shown at any detail level; it is bookkeeping, not conversation, and cost already surfaces through turn cost. A turn whose only work items are bookkeeping rows discloses nothing at all, not an empty work row.
- **Shared attachments and previews:** composer and transcript reuse one typed filename chip with an existing file/image icon and visible extension or File label. Full attachment paths appear only in tooltips, including on numbered image references. Transcript image previews load actual local files through the native allow-preview command and asset protocol; they sit above message text, align right for human turns, and preserve aspect ratio within (240px × 160px). Inline [Image #N] markers connect prose to previews. Missing, unreadable, or still-loading images retain a filename/type chip; non-images use that chip directly. Only valid standalone attachment-delivery lines receive this treatment; fenced examples and malformed lines remain text.
- **Unified attachment composer:** the text field rests at (40px) and grows or shrinks with its content up to (160px), returning to its compact height after a successful send. Below the field, plus and removable typed file chips form the leading group; the observed model control and send form the trailing group. Removable chips occupy (28px) in height, wrap beside plus, and truncate long filenames. Focus changes the whole composer boundary to signal. The keyboard hint sits below the surface and appears only while focus is within the composer; attachment-saving feedback also makes that line visible. Enter sends and Shift+Enter adds a line. The native picker supplies existing file paths; pasted attachment bytes are saved in application data before their paths are added to the prompt. Failed sends preserve draft and attachments. Attachment errors remain visible. Model selection opens the agent's native controls. Slash commands open that same mounted terminal without creating a pending chat message. Its own keyboard, search, and multi-step menus remain available; Back to chat preserves the draft and history. OpenCode uses its native /models command. No model catalog is hard-coded in the chat.
- **Pending decision:** an attention-colored top rule introduces the question and any command/body alongside wrapping choices. This replaces the ordinary context/terminal action strip. The composer remains visible with Answer the prompt first until the decision is answered.
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
- Don't replace useful wide-pane context with longer prose or a full-width reply action.
- Don't give routine turn notices separate transcript rows or duplicate the daemon's cost text.
- Don't invent model choices; open the current agent's native model controls.
- Don't present a terminal excerpt as assistant prose or label fallback output Writing.
- Don't expose full attachment paths as message content or replace a failed preview with an empty image frame.
