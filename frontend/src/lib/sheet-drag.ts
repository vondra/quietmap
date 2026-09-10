/** Bottom-sheet handle: what a finished touch on it means (both mobile sheets). */

// A finger that moved less than this is a tap; the synthesized click must
// reach onClick (audit 2026-09-06: preventDefault on every touchend ate it).
const TAP_SLOP_PX = 10
// A downward drag at least this long dismisses the sheet.
const DISMISS_DRAG_PX = 80

export function resolveSheetTouchEnd(deltaY: number): { cancelClick: boolean; dismiss: boolean } {
  return { cancelClick: Math.abs(deltaY) >= TAP_SLOP_PX, dismiss: deltaY > DISMISS_DRAG_PX }
}
