/**
 * Pure-core tests for the R7 transition taper (`buildTaperPlan`): boundary
 * detection (junction-free steps only), eligibility-driven anchoring, graded
 * ramps, and the never-touch invariants (tagged speed, census AADT, junctions).
 *
 * Run: `cd pipeline && npx tsx --test enrich-roads-taper.test.ts`
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { buildTaperPlan as buildPlanRaw, resolveSpeed, type CountrySpeeds, type Seg } from './lib/roads-taper-plan.js'
import { CZ_SPEEDS } from './lib/road-planning-defaults.generated.js'

// The CZ row from the SAME generated table the CLI uses — a legal-speed
// refresh flows into these tests automatically (drift-proof by design).
const CZ: CountrySpeeds = CZ_SPEEDS
const buildTaperPlan = (segs: Seg[]) => buildPlanRaw(segs, CZ)

/** Pure planner fixtures use opaque node labels; native readers supply z30 keys. */
function seg(o: Partial<Seg> & { i: number; osmId: number; a: string; b: string }): Seg {
  return {
    segIdx: 0, cls: 4, src: 0, speedTag: 0, builtUp: 1, access: 0,
    roundabout: false, len: 60, aadt: [0, 0, 0, 0],
    ...o,
  }
}

/** A way as a straight chain of `n` segments from node `startN`, sharing nodes. */
function way(osmId: number, i0: number, startN: number, n: number, o: Partial<Seg> = {}): Seg[] {
  return Array.from({ length: n }, (_, k) =>
    seg({
      i: i0 + k, osmId, segIdx: k,
      a: `50.${startN + k}_14.0`, b: `50.${startN + k + 1}_14.0`,
      ...o,
    }))
}

test('speed-tag edge (DE shape): untagged side ramps from the tag toward its own default', () => {
  // Way 1: one tagged-100 segment. Way 2: 5 untagged urban segments (resolved 50).
  // Continuation node (exactly two ways) → boundary; step = 30·log10(100/50) ≈ 9 dB.
  const segs = [...way(1, 0, 0, 1, { speedTag: 100 }), ...way(2, 1, 1, 5, { builtUp: 2 })]
  const { plan, stats } = buildTaperPlan(segs)
  assert.equal(stats.boundaries, 1)
  assert.equal(stats.kindCounts['speed-tag-edge'], 1)
  assert.equal(plan.has(0), false, 'tagged row never planned')

  // Untagged rows at dMid 30/90/150/210 m of the 250 m window ramp 100 → 50.
  const speeds = [1, 2, 3, 4].map((i) => plan.get(i)?.speed)
  assert.deepEqual(speeds, [94, 82, 70, 58])
  for (const i of [1, 2, 3, 4]) assert.equal(plan.get(i)?.aadt, undefined, 'same-class defaults → no AADT ramp')
  assert.equal(plan.get(5)?.speed, undefined, 'past the window the own value wins (suppressed)')
})

test('census edge: default side ramps log-space from the census value; census row untouched', () => {
  const census = way(1, 0, 0, 1, { src: 20, aadt: [5000, 100, 200, 50] })
  const defaults = way(2, 1, 1, 6)
  const { plan, stats } = buildTaperPlan([...census, ...defaults])
  assert.equal(stats.boundaries, 1)
  assert.equal(stats.kindCounts['census-edge'], 1)
  assert.equal(plan.has(0), false, 'census row never planned')

  const totals = [1, 2, 3, 4].map((i) => {
    const a = plan.get(i)?.aadt
    return a ? a[0] + a[1] + a[2] + a[3] : null
  })
  // Log-space descent from 5350 toward the tertiary default 800.
  assert.ok(totals.every((t): t is number => t !== null))
  for (let k = 0; k + 1 < totals.length; k++) assert.ok(totals[k]! > totals[k + 1]!, `monotone: ${totals}`)
  assert.ok(totals[0]! < 5350 && totals[0]! > 3000, `first ramp row below the anchor: ${totals[0]}`)
  assert.ok(totals[3]! > 800, `last ramp row above the default: ${totals[3]}`)
  for (const i of [1, 2, 3, 4]) assert.equal(plan.get(i)?.speed, undefined, 'equal resolved speeds → no speed ramp')
})

test('built-up flip inside one way: both defaults ramp from the midpoint, W/2 each side', () => {
  // One way: 3 rural (90) + 3 urban (50) segments, class 4 → speed-only step.
  const segs = [...way(1, 0, 0, 3, { builtUp: 1 }), ...way(1, 3, 3, 3, { builtUp: 2 })]
  segs.forEach((s, k) => (s.segIdx = k))
  const { plan, stats } = buildTaperPlan(segs)
  assert.equal(stats.boundaries, 1)
  assert.equal(stats.kindCounts['built-up-flip'], 1)

  // 125 m window per side, midpoint anchor 70: rural side ramps 70→90,
  // urban side 70→50; row dMids 30/90 → t=0.24/0.72.
  assert.deepEqual([plan.get(2)?.speed, plan.get(1)?.speed], [75, 84], 'rural side ascends away from the flip')
  assert.deepEqual([plan.get(3)?.speed, plan.get(4)?.speed], [65, 56], 'urban side descends away from the flip')
  assert.equal(plan.get(0)?.speed, undefined, 'beyond W/2 suppressed')
  assert.equal(plan.get(5)?.speed, undefined, 'beyond W/2 suppressed')
})

test('a junction kills the boundary; a service driveway does not', () => {
  const censusEdge = (extra: Seg[]) => {
    const segs = [
      ...way(1, 0, 0, 1, { src: 20, aadt: [5000, 100, 200, 50] }),
      ...way(2, 1, 1, 4),
      ...extra,
    ]
    return buildTaperPlan(segs).stats.boundaries
  }
  // A third THROUGH way at the shared node "50.1_14.0" → junction → no boundary.
  const side = seg({ i: 90, osmId: 9, cls: 5, a: '50.1_14.0', b: '50.1_14.9' })
  assert.equal(censusEdge([side]), 0, 'through-class side road = junction')
  // A service driveway (class 7) at the same node is not a flow split.
  const driveway = seg({ i: 91, osmId: 9, cls: 7, a: '50.1_14.0', b: '50.1_14.9' })
  assert.equal(censusEdge([driveway]), 1, 'service driveway ignored')
})

test('a loop way meeting another way is physical degree 3 — junction, not continuation', () => {
  // Way 9 loops n1→n2→n3→n1; way 1 (census) ends at n1. Only 2 distinct way
  // ids at n1, but 3 segment endpoints — the edge-degree rule must refuse the
  // continuation link (a way-id set alone would walk straight through).
  const census = way(1, 0, 0, 1, { src: 20, aadt: [5000, 100, 200, 50] })
  const loop = [
    seg({ i: 10, osmId: 9, segIdx: 0, a: '50.1_14.0', b: '50.1_14.5' }),
    seg({ i: 11, osmId: 9, segIdx: 1, a: '50.1_14.5', b: '50.2_14.5' }),
    seg({ i: 12, osmId: 9, segIdx: 2, a: '50.2_14.5', b: '50.1_14.0' }),
  ]
  const { plan, stats } = buildTaperPlan([...census, ...loop])
  assert.equal(stats.boundaries, 0, 'no boundary across a degree-3 node')
  assert.equal(plan.size, 0)
})

test('never-touch invariants: restricted access and roundabouts are not planned', () => {
  const censusRow = way(1, 0, 0, 1, { src: 20, aadt: [5000, 100, 200, 50] })
  const restricted = way(2, 1, 1, 4, { access: 3 })
  assert.equal(buildTaperPlan([...censusRow, ...restricted]).plan.size, 0, 'destination-access rows ineligible')

  const roundabout = way(2, 1, 1, 4, { roundabout: true })
  assert.equal(buildTaperPlan([...censusRow, ...roundabout]).plan.size, 0, 'roundabout rows ineligible')
})

test('resolveSpeed mirrors the engine cascade for the CZ row', () => {
  const s = (o: Partial<Seg>) => seg({ i: 0, osmId: 1, a: 'x', b: 'y', ...o })
  assert.equal(resolveSpeed(s({ speedTag: 70 }), CZ), 70, 'tag wins')
  assert.equal(resolveSpeed(s({ speedTag: 255 }), CZ), 130, 'derestricted sentinel')
  assert.equal(resolveSpeed(s({ cls: 4, builtUp: 2 }), CZ), 50, 'urban')
  assert.equal(resolveSpeed(s({ cls: 4, builtUp: 1 }), CZ), 90, 'rural')
  assert.equal(resolveSpeed(s({ cls: 4, builtUp: 0 }), CZ), 50, 'unknown → legacy table')
  assert.equal(resolveSpeed(s({ cls: 5, builtUp: 1 }), CZ), 30, 'residential stays legacy (engine scope)')
  assert.equal(resolveSpeed(s({ cls: 0 }), CZ), 130, 'motorway')
  assert.equal(resolveSpeed(s({ cls: 1 }), CZ), 110, 'CZ motorroad trunk')
})
