// A pin marker lives in a separate non-interleaved deck.gl overlay whose
// click handler cannot stop MapLibre's own click handler (DetailPopup) from
// also firing — `return true` only suppresses propagation within deck, not
// MapLibre's separate listener. So a pin click stamps this guard, and
// DetailPopup defers its open by a tick and skips when a pin was just
// clicked. Layers stamp via [`attachPinTapGuard`], NOT from deck's `onClick`:
// deck's click pick can lag frames behind the native click (66 ms measured
// under GPU readPixels stalls), which would lose the race against the
// deferred check — and a late stamp would dangle into the consume window and
// swallow an unrelated later click.
let lastClickTs = 0

export function markPropertyClick(): void {
  lastClickTs = Date.now()
}

// Generous window because deck's synchronous pick (readPixels) can stall the
// main thread well past the old 50 ms budget (66 ms measured under Playwright)
// — but consumed on first check, so one marker click suppresses exactly one
// deferred popup-open and a later genuine map click is never swallowed.
export function propertyJustClicked(): boolean {
  const hit = Date.now() - lastClickTs < 500
  lastClickTs = 0
  return hit
}

/**
 * Stamp the guard from raw canvas taps, deterministically ahead of the race:
 * the stamp runs inside the pointerup task, which always precedes the native
 * click's deferred check — no dependence on deck's pick timing. `hitTest`
 * gets canvas-relative CSS px and decides cheaply on the CPU (no readPixels;
 * a GPU pick here would stall every pan). Only a tap stamps (≤ 3 px travel,
 * matching MapLibre's clickTolerance: past that the map suppresses the DOM
 * click entirely, the deferred check never runs, and the stamp would dangle
 * into the consume window and swallow the next genuine tap.
 * Returns the detach function.
 */
export function attachPinTapGuard(
  canvas: HTMLElement,
  hitTest: (x: number, y: number) => boolean,
): () => void {
  let down: { x: number; y: number } | null = null
  const onDown = (e: PointerEvent) => {
    // Primary button only — a right-click fires no click event, so its stamp
    // would dangle and swallow the next genuine tap.
    down = e.button === 0 ? { x: e.clientX, y: e.clientY } : null
  }
  const onUp = (e: PointerEvent) => {
    const tap = down != null && e.button === 0 && Math.hypot(e.clientX - down.x, e.clientY - down.y) <= 3
    down = null
    if (!tap) return
    const rect = canvas.getBoundingClientRect()
    if (hitTest(e.clientX - rect.left, e.clientY - rect.top)) markPropertyClick()
  }
  canvas.addEventListener('pointerdown', onDown)
  canvas.addEventListener('pointerup', onUp)
  return () => {
    canvas.removeEventListener('pointerdown', onDown)
    canvas.removeEventListener('pointerup', onUp)
  }
}
