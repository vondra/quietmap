/** Bottom-sheet handle: what a finished touch on it means (both mobile sheets). */

// A finger that moved less than this is a tap; the synthesized click must
// reach onClick (audit 2026-09-06: preventDefault on every touchend ate it).
const TAP_SLOP_PX = 10
// A downward drag this long, or a third of a shorter sheet's height, dismisses the sheet: a finger
// on a short sheet's handle reaches the screen's edge before 80 px (review 2026-11-17).
const DISMISS_DRAG_PX = 80

export function resolveSheetTouchEnd(deltaY: number, sheetHeight: number): { cancelClick: boolean; dismiss: boolean } {
  return { cancelClick: Math.abs(deltaY) >= TAP_SLOP_PX, dismiss: deltaY > Math.min(DISMISS_DRAG_PX, sheetHeight / 3) }
}
