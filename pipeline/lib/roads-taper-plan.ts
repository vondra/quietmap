/**
 * R7 transition-taper planning core (pure — no I/O; `enrich-roads-taper.ts`
 * is the thin Arrow/CLI wrapper, tests feed synthetic segments).
 *
 * Finds junction-free boundaries where the RESOLVED emission inputs (speed,
 * AADT — enriched value or engine class default) step between adjacent
 * segments of one physical road, and plans graded ramps over a short window:
 * a car decelerates over distance, it does not step.
 *
 * Anchoring is eligibility-driven: a side the taper cannot write (census
 * AADT, OSM-tagged speed, out-of-coverage class, restricted access) keeps its
 * value fixed and the writable side ramps from that full value; two writable
 * default sides each ramp from the midpoint. Ramp targets are per-row own
 * resolved values, so multi-class chains grade correctly.
 *
 * Resolution mirrors the engine (defaults.rs / normalize/road.rs) for
 * countries whose class-default AADT is the unscaled WORLD table — classes
 * 3-9 everywhere (see defaults.rs::country_local_roads_are_not_scaled);
 * GDP-scaled majors (0/1/2 + links) resolve to `null` when unenriched and
 * their boundaries are skipped. Legal speeds come from the generated
 * per-country table (country-speed-defaults.generated.json — same generator
 * as the engine's Rust table, so the two cannot drift).
 */

import { DATASETS } from './enrichment-datasets.js'
import { classDefault } from './road-class-defaults.js'

/** Classes the taper may stamp — read from the registry entry so the writer
 *  coverage set, the invariant scanner's R1 rule and this plan gate cannot
 *  drift apart. */
export const TAPER_CLASSES: ReadonlySet<number> = new Set(
  DATASETS.find((d) => d.key === 'osm-transition-taper')!.roadCoverage!,
)

/** Emission-step trigger. ΔdB estimate = 10·log10(AADT ratio) +
 *  30·log10(speed ratio) — 30 is the CNOSSOS-EU category-1 rolling-noise
 *  speed coefficient (2015/996 Annex II, B_R ≈ 30·log10(v/70)); AADT enters
 *  emission linearly. Below 2 dB a step is within the palette's 0.5 dB
 *  quantisation blur and not worth stamping rows for. */
export const TRIGGER_DB = 2.0

/** Ramp length when the far side is anchored (census AADT / tagged speed):
 *  ~250 m covers comfortable deceleration 100→50 km/h (~1 m/s² ≈ 290 m)
 *  without repainting whole villages. */
const W_ANCHORED_M = 250
/** Ramp length PER SIDE when both sides are writable defaults (built_up /
 *  class flip) — the full transition is again ~250 m. */
const W_DEFAULT_M = 125

/** Write-suppression floors — a graded value this close to what the engine
 *  would resolve anyway is a wasted stamp. */
const MIN_SPEED_DIFF_KMH = 3
const MIN_AADT_RATIO = 0.05

/** Unenriched GDP-scaled classes (0/1/2 + links) resolve through the engine's
 *  country-scale arm this mirror can't reproduce (defaults.rs country_default
 *  arm b) — their defaults are not plannable. */
const GDP_SCALED = new Set([0, 1, 2, 10, 11, 12])

/** Engine legacy world speed table (normalize/road.rs::default_road_speed) —
 *  the fallback when the country table has no value for the class/built_up. */
const LEGACY_SPEED: Record<number, number> = {
  0: 100, 1: 70, 2: 50, 3: 50, 4: 50, 5: 30, 6: 20, 7: 20, 8: 20, 9: 50, 10: 60, 11: 50, 12: 50,
}

/** [urban, rural, motorway, motorroad] km/h, 0 = absent — one row of
 *  country-speed-defaults.generated.json. */
export type CountrySpeeds = readonly [number, number, number, number]

export interface Seg {
  i: number
  osmId: number
  segIdx: number
  cls: number
  src: number
  speedTag: number
  builtUp: number
  access: number
  roundabout: boolean
  len: number
  a: string
  b: string
  aadt: readonly [number, number, number, number]
}

/** Engine-resolved speed (defaults.rs::resolve_speed_default + the legacy
 *  fallback in normalize/road.rs, exactly mirrored). */
export function resolveSpeed(s: Seg, country: CountrySpeeds): number {
  if (s.speedTag === 255) return 130 // maxspeed=none sentinel → derestricted model speed
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

const isEnriched = (s: Seg): boolean => s.src !== 0 && s.aadt[0] > 0

/** Engine-resolved AADT 4-tuple, or null where the mirror can't reproduce the
 *  engine's country-scaled default (unenriched 0/1/2 + links). */
export function resolveAadt(s: Seg): readonly [number, number, number, number] | null {
  if (isEnriched(s)) return s.aadt
  if (GDP_SCALED.has(s.cls)) return null
  return classDefault(s.cls)
}

const total = (a: readonly [number, number, number, number]): number => a[0] + a[1] + a[2] + a[3]

/** A row is a valid ramp target: nothing about it is authored data. */
const eligible = (s: Seg): boolean =>
  TAPER_CLASSES.has(s.cls) &&
  s.src === 0 &&
  s.speedTag === 0 &&
  (s.access === 0 || s.access === 5) &&
  !s.roundabout

export interface PlanEntry {
  aadt?: readonly [number, number, number, number]
  speed?: number
  /** Distance from the owning boundary — nearer boundary wins on overlap. */
  dist: number
}

export interface TaperStats {
  boundaries: number
  skippedUnscaled: number
  speedOnly: number
  aadtOnly: number
  both: number
  /** Detected boundaries per kind (census-edge / speed-tag-edge / class-flip /
   *  built-up-flip) — the discontinuity scanner's aggregation unit. */
  kindCounts: Record<string, number>
  /** Top boundaries by estimated step, for screenshot targeting. */
  top: Array<{ lat: number; lon: number; db: number; kind: string }>
}

/** Find junction-free step boundaries in one hex's segments and plan graded
 *  writes. `country` = the hex's legal-speed row (v1 callers gate to CZ). */
export function buildTaperPlan(
  segs: Seg[],
  country: CountrySpeeds,
): { plan: Map<number, PlanEntry>; stats: TaperStats } {
  const stats: TaperStats = {
    boundaries: 0, skippedUnscaled: 0, speedOnly: 0, aadtOnly: 0, both: 0, kindCounts: {}, top: [],
  }

  // Node topology from through classes only (service/track driveways do not
  // split through-traffic, so they make neither junctions nor continuations).
  // `nodeEdges` counts segment ENDPOINTS at the node — the physical degree.
  // A junction is degree > 2; a way-ID set alone misses loops (a way whose
  // both ends meet another way at one node is degree 3 with only 2 way ids —
  // /gg Gemini CRITICAL). `nodeWays` still identifies the two ways of a
  // degree-2 continuation.
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

  // Chain adjacency: within a way by segment_idx; across ways where exactly
  // two ways meet end-to-end (a degree-2 node is a continuation, not a split).
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
      if (!via) continue // gap in the way inside this hex (boundary clip)
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

  // Physical degree decides: >2 endpoint-edges at the node = traffic can
  // enter/leave = the step there is legitimate and stays sharp.
  const isJunction = (node: string): boolean => (nodeEdges.get(node) ?? 0) > 2

  // Walk one side of a boundary, planning graded values toward each row's own
  // resolved target: speed linear (braking is ~linear in speed over distance),
  // AADT log-space (equal dB steps — a 9000→300 ramp must not idle near 9000).
  const plan = new Map<number, PlanEntry>()
  const walk = (
    start: Seg,
    cameFrom: Seg,
    window: number,
    anchorSpeed: number | null,
    anchorAadt: readonly [number, number, number, number] | null,
  ): void => {
    let prev = cameFrom
    let cur: Seg | null = start
    let dist = 0
    while (cur && dist < window) {
      if (!eligible(cur)) return
      const dMid = dist + cur.len / 2
      const t = Math.min(1, dMid / window)
      const entry: PlanEntry = { dist: dMid }
      if (anchorSpeed !== null) {
        const own = resolveSpeed(cur, country)
        const v = Math.round(anchorSpeed + (own - anchorSpeed) * t)
        if (Math.abs(v - own) >= MIN_SPEED_DIFF_KMH) entry.speed = Math.max(1, Math.min(254, v))
      }
      if (anchorAadt !== null) {
        const own = resolveAadt(cur)
        if (own) {
          const graded = [0, 1, 2, 3].map((c) => {
            const a = Math.max(1, anchorAadt[c])
            const o = Math.max(1, own[c])
            return Math.round(Math.exp(Math.log(a) + (Math.log(o) - Math.log(a)) * t))
          }) as [number, number, number, number]
          const ratio = Math.abs(total(graded) - total(own)) / Math.max(1, total(own))
          if (ratio >= MIN_AADT_RATIO) entry.aadt = graded
        }
      }
      if (entry.speed !== undefined || entry.aadt !== undefined) {
        const existing = plan.get(cur.i)
        if (!existing || existing.dist > dMid) plan.set(cur.i, entry)
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

  // One side of a detected boundary: ramp the writable side from the far
  // side's value (full value when the far side is fixed, midpoint when both
  // are writable defaults).
  const planSide = (
    self: Seg,
    other: Seg,
    otherEligible: boolean,
    speedStep: boolean,
    aadtStep: boolean,
    otherSpeed: number,
    midSpeed: number,
    otherAadt: readonly [number, number, number, number],
    midAadt: readonly [number, number, number, number],
  ): void => {
    const w = otherEligible ? W_DEFAULT_M : W_ANCHORED_M
    const speedAnchor = speedStep ? (otherEligible ? midSpeed : otherSpeed) : null
    const aadtAnchor = aadtStep ? (otherEligible ? midAadt : otherAadt) : null
    if (speedAnchor !== null || aadtAnchor !== null) walk(self, other, w, speedAnchor, aadtAnchor)
  }

  // Boundary detection over every chain link (deduped p<q).
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
      const aadtP = resolveAadt(p)
      const aadtQ = resolveAadt(q)
      if (!aadtP || !aadtQ) {
        stats.skippedUnscaled++
        continue
      }
      const vP = resolveSpeed(p, country)
      const vQ = resolveSpeed(q, country)
      const db =
        Math.abs(10 * Math.log10(Math.max(1, total(aadtQ)) / Math.max(1, total(aadtP)))) +
        Math.abs(30 * Math.log10(vQ / vP))
      if (db < TRIGGER_DB) continue
      stats.boundaries++

      const speedStep = Math.abs(vQ - vP) >= MIN_SPEED_DIFF_KMH
      const aadtStep =
        Math.abs(total(aadtQ) - total(aadtP)) / Math.max(1, Math.min(total(aadtP), total(aadtQ))) >= MIN_AADT_RATIO
      const kind =
        isEnriched(p) !== isEnriched(q) ? 'census-edge'
        : (p.speedTag > 0) !== (q.speedTag > 0) ? 'speed-tag-edge'
        : p.cls !== q.cls ? 'class-flip' : 'built-up-flip'
      stats.kindCounts[kind] = (stats.kindCounts[kind] ?? 0) + 1
      const [latS, lonS] = via.split('_').map(Number)
      stats.top.push({ lat: latS, lon: lonS, db, kind })

      const midSpeed = (vP + vQ) / 2
      const midAadt = [0, 1, 2, 3].map((c) =>
        Math.sqrt(Math.max(1, aadtP[c]) * Math.max(1, aadtQ[c])),
      ) as [number, number, number, number]
      if (pE) planSide(p, q, qE, speedStep, aadtStep, vQ, midSpeed, aadtQ, midAadt)
      if (qE) planSide(q, p, pE, speedStep, aadtStep, vP, midSpeed, aadtP, midAadt)
    }
  }

  for (const e of plan.values()) {
    if (e.speed !== undefined && e.aadt !== undefined) stats.both++
    else if (e.speed !== undefined) stats.speedOnly++
    else stats.aadtOnly++
  }
  stats.top.sort((x, y) => y.db - x.db)
  stats.top = stats.top.slice(0, 20)
  return { plan, stats }
}
