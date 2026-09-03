/// Rows a Multitasking grid row must always fit. Mirrors the daemon's own floor
/// (`MIN_PANE_ROWS`, `crates/repomon-core/src/agent/tmux.rs`) so the frontend's guaranteed
/// minimum can never sit below what every viewer's pty is already guaranteed to have. Shared from
/// one module (rather than the literal `24` living separately in TerminalPane.tsx) so the two
/// floors cannot drift apart.
export const MULTITASK_MIN_ROWS = 24;

export interface MultitaskRowFloorInput {
  /// Header + composer inset height around the terminal screen, in pixels.
  chromeHeight: number;
  /// The renderer's actual per-row height, in pixels.
  cellHeight: number;
}

/// The Multitasking row-height floor: chrome plus room for `MULTITASK_MIN_ROWS` terminal rows at
/// the renderer's real cell height. Pure and independent of the terminal's current row count:
/// unlike measuring the rendered `.xterm-screen` height, this cannot ratchet upward when a pane
/// transiently renders more rows than its grid cell actually has room for (see
/// `.briefs/2026-09-03-multitasking-row-ratchet.md` for the feedback loop this replaces).
export function multitaskRowFloor({ chromeHeight, cellHeight }: MultitaskRowFloorInput): number {
  return Math.ceil(chromeHeight + MULTITASK_MIN_ROWS * cellHeight);
}
