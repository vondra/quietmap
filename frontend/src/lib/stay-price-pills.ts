// Price pills over the stay pins: the label, its screen-space box (shared with
// the tap guard) and the CPU declutter that picks which pills are drawn.
import type { Stay } from '../components/StayLayer'

export function formatPerNight(amount: number): string {
  return `€${amount}`
}

/** Pill screen-space box for a pin at (x, y) — mirrors the TextLayer's
 *  pixel offset, font metrics and padding. */
export function pillBox(x: number, y: number, label: string) {
  const halfW = 7 + 3.5 * label.length
  return { x1: x - halfW, y1: y - 26, x2: x + halfW, y2: y - 6 }
}

// Overlapping prices were unreadable (owner 2026-07-29) and deck's
// CollisionFilterExtension silently blanks this TextLayer inside the
// overlaid (non-interleaved) MapboxOverlay — so pill winners are picked on
// the CPU: popularity-ranked greedy placement in screen space. Review count
// moves slowly, so the same pills keep winning and zooming/panning doesn't
// reshuffle which prices show. Runs per moveend over ≤ POOL_MAX pins.
export function declutterPills(stays: Stay[], map: { project: (c: [number, number]) => { x: number; y: number } }, width: number, height: number): Stay[] {
  const priced = stays.filter(s => s.price != null)
  priced.sort((a, b) => (b.rating.count ?? 0) - (a.rating.count ?? 0))
  const placed: { x1: number; y1: number; x2: number; y2: number }[] = []
  const out: Stay[] = []
  for (const s of priced) {
    const p = map.project([s.lng, s.lat])
    if (p.x < -60 || p.x > width + 60 || p.y < -40 || p.y > height + 40) continue
    const r = pillBox(p.x, p.y, formatPerNight(s.price!.perNight))
    if (placed.some(q => q.x1 < r.x2 + 2 && r.x1 < q.x2 + 2 && q.y1 < r.y2 + 2 && r.y1 < q.y2 + 2)) continue
    placed.push(r)
    out.push(s)
  }
  return out
}
