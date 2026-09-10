# Repomon desktop

<!-- impeccable:product-schema 1 -->

## Platform

Web interface in a Tauri desktop shell, built with SolidJS and TypeScript. Headless Chrome exercises the same components through an isolated IPC fixture.

## Users and purpose

The operator supervises coding agents across repositories and worktree lanes. The desktop makes lanes, attention requests, live agent output, terminal sessions, and repository tools available in one workspace.

## Operating context

The operator's actual desktop window can be 2000px wide. Required visual checks also include 1440x900 and 1040x680, in both light and dark themes. Many real lanes have no usable transcript headline and nothing needs attention. Repeated branch names such as main are normal.

## Capabilities and constraints

- Home identifies each lane with its repository and accepted transcript headline, falling back to its branch. Duplicate repo/title pairs retain a lane identifier.
- The daemon owns headline rejection. The desktop does not infer headline quality or invent extra wire fields.
- Conversation and Terminal are two views of the same mounted pane and session. Changing the view does not restart the agent.
- Chat receives a watch's initial page and live upserts keyed by transcript item ID. A partial and its final row share one ID. Previously loaded history survives live updates.
- Missing optional metadata and malformed transcript lines must retain readable content. Unsupported transcript sources can show raw pane output.
- Pending prompts are answerable through the daemon's existing dialog protocol. Compose sends input to the agent's existing window.
- A lane override wins over its agent-kind default, with Terminal as the final fallback. Unsupported kinds have disabled default-view controls and a Terminal only explanation.

## Brand commitments

The existing index.css tokens, controls, icons, and fonts are authoritative. No new palette, typography system, ornamental assets, or visual world is introduced by conversation work.

The approved visual contracts are `design/mocks/codex/home-refined.html` and `design/mocks/codex/conversation-refined.html` in the repository's design material. During this task they are available at `/Users/azaleas/Developer/Claude/repomon/design/mocks/codex/`.

The operator approved these home adaptations to the idealized comp: repo-leading rows, a bounded reading column, compact strips, explicit state words, and a quiet zero-urgency anchor. Preserve them when adding headline data.

## Evidence on hand

`src/ipc/tauriCore.fixture.ts` contains the operator's supplied headline, branch, and PR strings verbatim, alongside raw and rejected-headline scenarios. The ordinary fleet, rich conversation, partial stream, failed tool, malformed line, missing source, and pending prompt are all required evidence. Fixtures must not flatter the design by assuming rich headlines or urgency everywhere.

## Product principles

- A lane must remain identifiable without a headline.
- Live output appears while work runs.
- Broken or missing source data remains readable.
- Controls preserve the session and expose their current state.
- Validate the operator's actual window width as well as compact layouts.
