/** Plan CZ speed transitions along junction-free road chains, independently of traffic. */

import { DATASETS } from './enrichment-datasets.js'
import type { PlanningRoad } from './road-planning-input.js'
import { LEGACY_SPEED, DERESTRICTED_SPEED_KMH } from './road-planning-defaults.generated.js'

export const TAPER_CLASSES: ReadonlySet<number> = new Set(
  DATASETS.find((d) => d.key === 'osm-transition-taper')!.roadCoverage!,
)

// Dev1 R7 trigger uses the CNOSSOS category-1 rolling coefficient 30 log10(v).
export const TRIGGER_DB = 2.0

// Dev1 R7: 250 m anchored braking transition; two default sides use 125 m each.
const W_ANCHORED_M = 250

const W_DEFAULT_M = 125

const MIN_SPEED_DIFF_KMH = 3

export type CountrySpeeds = readonly [number, number, number, number]

export type Seg = Omit<PlanningRoad, 'ref' | 'name' | 'src' | 'aadt'>

export function resolveSpeed(s: Seg, country: CountrySpeeds): number {
  if (s.speedTag === 255) return DERESTRICTED_SPEED_KMH
  if (s.speedTag > 0) return s.speedTag
  const [urban, rural, motorway, motorroad] = country
  let v = 0
  if (s.cls === 0) v = motorway
  else if (s.cls === 1) v = motorroad > 0 ? motorroad : rural
  else if (s.cls === 2 || s.cls === 3 || s.cls === 4 || s.cls === 9) {
    v = s.builtUp === 2 ? urban : s.builtUp === 1 ? rural : 0
  }
  return v > 0 ? v : (LEGACY_SPEED[s.cls] ?? 50)
}

const eligible = (s: Seg): boolean =>
  TAPER_CLASSES.has(s.cls) &&
  s.speedTag === 0 &&
  (s.access === 0 || s.access === 5) &&
  !s.roundabout

export interface PlanEntry {
  speed: number
  dist: number
}

export interface TaperStats {
  boundaries: number
  kindCounts: Record<string, number>
}

export function buildTaperPlan(
  segs: Seg[],
  country: CountrySpeeds,
): { plan: Map<number, PlanEntry>; stats: TaperStats } {
  const stats: TaperStats = {
    boundaries: 0, kindCounts: {},
  }
  const nodeWays = new Map<string, Set<number>>()
  const nodeEdges = new Map<string, number>()
  for (const s of segs) {
    if (s.cls === 7 || s.cls === 8) continue
    for (const k of [s.a, s.b]) {
      let set = nodeWays.get(k)
      if (!set) nodeWays.set(k, (set = new Set()))
      set.add(s.osmId)
      nodeEdges.set(k, (nodeEdges.get(k) ?? 0) + 1)
    }
  }
  const byWay = new Map<number, Seg[]>()
  for (const s of segs) {
    const arr = byWay.get(s.osmId)
    if (arr) arr.push(s)
    else byWay.set(s.osmId, [s])
  }
  const neighbours = new Map<number, Array<{ seg: Seg; via: string }>>()
  const link = (p: Seg, q: Seg, via: string) => {
    let arr = neighbours.get(p.i)
    if (!arr) neighbours.set(p.i, (arr = []))
    arr.push({ seg: q, via })
  }
  const sharedNode = (p: Seg, q: Seg): string | null =>
    p.a === q.a || p.a === q.b ? p.a : p.b === q.a || p.b === q.b ? p.b : null
  for (const rows of byWay.values()) {
    rows.sort((x, y) => x.segIdx - y.segIdx)
    for (let k = 0; k + 1 < rows.length; k++) {
      const via = sharedNode(rows[k], rows[k + 1])
      if (!via) continue // gap in the way inside this owner (boundary clip)
      link(rows[k], rows[k + 1], via)
      link(rows[k + 1], rows[k], via)
    }
  }
  for (const [node, ways] of nodeWays) {
    if (ways.size !== 2 || nodeEdges.get(node) !== 2) continue
    const terminals: Seg[] = []
    for (const wayId of ways) {
      const rows = byWay.get(wayId)!
      const first = rows[0]
      const last = rows[rows.length - 1]
      if (first.a === node || first.b === node) terminals.push(first)
      else if (last.a === node || last.b === node) terminals.push(last)
    }
    if (terminals.length === 2 && terminals[0].osmId !== terminals[1].osmId) {
      link(terminals[0], terminals[1], node)
      link(terminals[1], terminals[0], node)
    }
  }
  const isJunction = (node: string): boolean => (nodeEdges.get(node) ?? 0) > 2
  const plan = new Map<number, PlanEntry>()
  const walk = (
    start: Seg,
    cameFrom: Seg,
    window: number,
    anchorSpeed: number,
  ): void => {
    let prev = cameFrom
    let cur: Seg | null = start
    let dist = 0
    while (cur && dist < window) {
      if (!eligible(cur)) return
      const dMid = dist + cur.len / 2
      const t = Math.min(1, dMid / window)
      const own = resolveSpeed(cur, country)
      const speed = Math.round(anchorSpeed + (own - anchorSpeed) * t)
      if (Math.abs(speed - own) >= MIN_SPEED_DIFF_KMH) {
        const existing = plan.get(cur.i)
        if (!existing || existing.dist > dMid) {
          plan.set(cur.i, { dist: dMid, speed: Math.max(1, Math.min(254, speed)) })
        }
      }
      dist += cur.len
      const next: Array<{ seg: Seg; via: string }> = (neighbours.get(cur.i) ?? []).filter(
        (n) => n.seg.i !== prev.i,
      )
      if (next.length !== 1) return // fork/dead-end inside the walk → stop
      if (isJunction(next[0].via)) return
      prev = cur
      cur = next[0].seg
    }
  }
  const segByIdx = new Map(segs.map((s) => [s.i, s]))
  for (const [i, links] of neighbours) {
    const p = segByIdx.get(i)!
    for (const { seg: q, via } of links) {
      if (p.i >= q.i) continue
      if (!TAPER_CLASSES.has(p.cls) && !TAPER_CLASSES.has(q.cls)) continue
      if (p.roundabout || q.roundabout) continue
      if (isJunction(via)) continue
      const pE = eligible(p)
      const qE = eligible(q)
      if (!pE && !qE) continue
      const vP = resolveSpeed(p, country)
      const vQ = resolveSpeed(q, country)
      if (Math.abs(vQ - vP) < MIN_SPEED_DIFF_KMH || Math.abs(30 * Math.log10(vQ / vP)) < TRIGGER_DB) continue
      stats.boundaries++

      const kind =
        (p.speedTag > 0) !== (q.speedTag > 0) ? 'speed-tag-edge'
        : p.cls !== q.cls ? 'class-flip' : 'built-up-flip'
      stats.kindCounts[kind] = (stats.kindCounts[kind] ?? 0) + 1

      const midSpeed = (vP + vQ) / 2
      if (pE) walk(p, q, qE ? W_DEFAULT_M : W_ANCHORED_M, qE ? midSpeed : vQ)
      if (qE) walk(q, p, pE ? W_DEFAULT_M : W_ANCHORED_M, pE ? midSpeed : vP)
    }
  }

  return { plan, stats }
}
