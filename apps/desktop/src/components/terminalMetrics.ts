/// Sets the guaranteed terminal row floor to match the daemon’s minimum pane size.
export const MULTITASK_MIN_ROWS = 24;

export interface MultitaskRowFloorInput {
  /// Header + composer inset height around the terminal screen, in pixels.
  chromeHeight: number;
  /// The renderer's actual per-row height, in pixels.
  cellHeight: number;
}

/// Computes chrome plus a fixed row count at measured cell height, independent of the current grid
/// to prevent row-height feedback.
export function multitaskRowFloor({ chromeHeight, cellHeight }: MultitaskRowFloorInput): number {
  return Math.ceil(chromeHeight + MULTITASK_MIN_ROWS * cellHeight);
}
