/**
 * Regression tests for enrich-roads-service-tree.ts root-cause fixes.
 * Run: `npx tsx --test pipeline/enrich-roads-service-tree.test.ts`
 *
 * Three scenarios from the /gg quad review (commit 2bda9264 + 6a2fa2af):
 *   1. track-stub-not-root           — service road dead-ending at cls=8 track
 *                                       must NOT form an exit; flow stays local.
 *   2. measured-boundary-still-roots — non-overwriteable cls 5–9 (already filled
 *                                       by higher-precedence source) must still
 *                                       create an exit, otherwise pseudo-root
 *                                       inverts flow toward an internal hub.
 *   3. apartment-cap-clamps           — cls=7 cap=400 clamps high accumulated flow.
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  buildGraph,
  findComponents,
  flowAccumulate,
  splitAADT,
  SERVICE_TREE_CAP_PER_CLASS,
} from './enrich-roads-service-tree.ts'
import { WORLD_FLEET } from './lib/country-fleet.generated.ts'
import type { BuildingLoad } from './lib/trip-rates.ts'

// ─── Mock arrow-table helper ───────────────────────────────────────────────
//
// `buildGraph` only needs `.numRows` and `.getChild(name).get(i)` from the
// table — no need to construct a real apache-arrow Table for these tests.
// Each test row is `{ start_lat, start_lon, end_lat, end_lon, length_m,
// road_class, source_id, osm_id }`.

interface RoadRow {
  start_lat: number; start_lon: number
  end_lat: number;   end_lon: number
  length_m: number
  road_class: number
  source_id?: number
  osm_id?: number
  /** Arrow Bool column — the mock returns real booleans like apache-arrow does. */
  tunnel?: boolean
  access?: number
  /** The mock table reads rows by column name, like `getChild(name).get(i)`. */
  [column: string]: number | boolean | undefined
}

const OPTIONAL_COLUMNS = new Set(['source_id', 'osm_id', 'tunnel', 'access'])

function mockRoadTable(rows: RoadRow[]): any {
  return {
    numRows: rows.length,
    getChild(name: string) {
      if (!rows.length) return undefined
      const sample = rows[0]
      if (!(name in sample) && !OPTIONAL_COLUMNS.has(name)) return undefined
      return {
        get: (i: number) => {
          const v = rows[i][name]
          if (name === 'tunnel') return v ?? false // Bool column: boolean, never 0/1
          return v ?? 0
        },
      }
    },
  }
}

// ─── Test 1: track-stub does not create an exit ────────────────────────────

test('track-stub-not-root: service road dead-ending at track is not an exit', () => {
  // N0 ──cls=5──N1──cls=7──N2──cls=8(track)──N3
  //                         │
  //                         (track has no further connections)
  const rows: RoadRow[] = [
    { start_lat: 0, start_lon: 0, end_lat: 0, end_lon: 0.001, length_m: 100, road_class: 5 }, // N0-N1 residential
    { start_lat: 0, start_lon: 0.001, end_lat: 0, end_lon: 0.002, length_m: 100, road_class: 7 }, // N1-N2 service
    { start_lat: 0, start_lon: 0.002, end_lat: 0, end_lon: 0.003, length_m: 100, road_class: 8 }, // N2-N3 track
  ]
  const graph = buildGraph(mockRoadTable(rows))
  const components = findComponents(graph)

  // Eligible: cls=5 + cls=7 (track excluded)
  assert.strictEqual(graph.eligible[0], 1, 'cls=5 eligible')
  assert.strictEqual(graph.eligible[1], 1, 'cls=7 eligible')
  assert.strictEqual(graph.eligible[2], 0, 'cls=8 track NOT eligible')

  // Exactly one component (the cls=5 + cls=7 chain)
  assert.strictEqual(components.length, 1)
  assert.strictEqual(components[0].segments.length, 2)

  // No exit edges anywhere — track is not an exit; pure dead-end.
  assert.strictEqual(components[0].rootNodes.size, 0,
    'track endpoint must NOT be a root — that was the Pasito bug')

  // Pseudo-root fallback fires; flow stays bounded by local trips
  // (no buildings in this fixture, so segFlow seeded with 0).
  const segNodeIds = graph.segNodeIds
  const lengthCol = mockRoadTable(rows).getChild('length_m')
  const emptyLoad = new Map<number, BuildingLoad>()
  const segFlow = flowAccumulate(components[0], segNodeIds, lengthCol, emptyLoad, () => WORLD_FLEET)
  for (const flow of segFlow.values()) {
    assert.strictEqual(flow, 0, 'no buildings → no flow inflation through fake-root')
  }
})

// ─── Test 1b: tunnel/access eligibility mirrors the engine drop rule ────────

test('tunnel (Bool column) and access=no are excluded; plain rows stay eligible', () => {
  // Four cls=5 segments in a row: open, tunnel=true, access=2 (no), access=3
  // (destination — engine keeps it, only discounted).
  const rows: RoadRow[] = [
    { start_lat: 0, start_lon: 0, end_lat: 0, end_lon: 0.001, length_m: 100, road_class: 5, tunnel: false, access: 0 },
    { start_lat: 0, start_lon: 0.001, end_lat: 0, end_lon: 0.002, length_m: 100, road_class: 5, tunnel: true, access: 0 },
    { start_lat: 0, start_lon: 0.002, end_lat: 0, end_lon: 0.003, length_m: 100, road_class: 5, tunnel: false, access: 2 },
    { start_lat: 0, start_lon: 0.003, end_lat: 0, end_lon: 0.004, length_m: 100, road_class: 5, tunnel: false, access: 3 },
  ]
  const graph = buildGraph(mockRoadTable(rows))
  // The Bool column returns `false`, not 0 — a numeric !== 0 compare here once
  // disqualified EVERY segment (/gg diff review CRITICAL); this pins it.
  assert.strictEqual(graph.eligible[0], 1, 'open segment eligible')
  assert.strictEqual(graph.eligible[1], 0, 'tunnel excluded')
  assert.strictEqual(graph.eligible[2], 0, 'access=no excluded')
  assert.strictEqual(graph.eligible[3], 1, 'access=destination stays eligible')
})

// ─── Test 2: measured-boundary still roots ─────────────────────────────────

test('measured-boundary-still-roots: non-overwriteable cls=5 marks exit', () => {
  // N0 ──cls=5(measured src=10)──N1──cls=5──N2──cls=7──N3
  //
  // The N0-N1 segment is non-overwriteable (eu-city-traffic, source_id=10).
  // It is filtered OUT of routing eligibility, but N1 must still be an exit
  // root because the measured edge IS a real motor-vehicle exit.
  const rows: RoadRow[] = [
    { start_lat: 0, start_lon: 0, end_lat: 0, end_lon: 0.001, length_m: 100, road_class: 5, source_id: 10 }, // measured
    { start_lat: 0, start_lon: 0.001, end_lat: 0, end_lon: 0.002, length_m: 100, road_class: 5, source_id: 0 }, // unfilled
    { start_lat: 0, start_lon: 0.002, end_lat: 0, end_lon: 0.003, length_m: 100, road_class: 7, source_id: 0 }, // unfilled service
  ]
  const graph = buildGraph(mockRoadTable(rows))

  // Eligibility: only un-overwriteable rows are in routing graph
  assert.strictEqual(graph.eligible[0], 0, 'measured cls=5 NOT in routing graph')
  assert.strictEqual(graph.eligible[1], 1)
  assert.strictEqual(graph.eligible[2], 1)

  // hasExitEdge: N1 is shared between measured edge (sets hasExitEdge) and
  // the eligible cls=5 (doesn't set). The OR → N1.hasExitEdge=true.
  // N0 only touches the measured edge → also hasExitEdge=true (but N0 is
  // outside any eligible component, so won't appear as root).
  // N2, N3 only touch eligible local edges → hasExitEdge=false.
  const components = findComponents(graph)
  assert.strictEqual(components.length, 1, 'one eligible component')
  assert.strictEqual(components[0].segments.length, 2)

  // The component must have exactly one root: N1 (where measured boundary meets eligible).
  assert.strictEqual(components[0].rootNodes.size, 1,
    'measured-cls=5 boundary node must still be a root — that was the Codex finding')
})

// ─── Test 3: cls=7 cap clamps apartment-block accumulated flow ─────────────

test('apartment-cap-clamps: cls=7 flow > 400 clamps to cap, splitAADT correct', () => {
  // Cap is 400 (1.6× engine default of 250); apartment-block driveways with
  // many dwellings can legitimately accumulate more, and the cap is the
  // intended defense. Verify the constant + the splitAADT result.
  assert.strictEqual(SERVICE_TREE_CAP_PER_CLASS[7], 400,
    'cls=7 cap raised from 200 to 400 (above engine default 250)')

  // Apartment block: 200 dwellings × 3.68 (WORLD tpd) = 736 trips (above cap).
  const rawTrips = 200 * WORLD_FLEET.tripsPerDwelling
  assert.ok(rawTrips > 400, 'apartment-block trip count exceeds cap')

  const capped = Math.min(rawTrips, SERVICE_TREE_CAP_PER_CLASS[7])
  assert.strictEqual(capped, 400)

  // splitAADT proportions with the WORLD fleet: 1 % medium, 2 % heavy,
  // 1 % moto (bit-preserved pre-2026-07 behaviour), rest light.
  const split = splitAADT(capped, WORLD_FLEET)
  assert.strictEqual(split.medium + split.heavy + split.moto + split.light, 400)
  assert.ok(split.light >= 380 && split.light <= 396, `light should dominate, got ${split.light}`)
})

// ─── Test 4: fleet split conservation + country mix ────────────────────────
// (Per-building trip generation moved to lib/trip-rates.test.ts.)

test('splitAADT: conservation, clamp floor, and country moto share', () => {
  // Conservation at any total, any fleet: the four classes always re-sum to
  // the rounded total (light is the exact remainder).
  for (const fleet of [WORLD_FLEET, { motoTrafficShare: 0.45, tripsPerDwelling: 2.5 }]) {
    for (const total of [0, 7, 20, 333.4, 2000]) {
      const s = splitAADT(total, fleet)
      const expected = Math.round(Math.max(total, 20)) // MIN_AADT floor
      assert.strictEqual(s.light + s.medium + s.heavy + s.moto, expected)
      assert.ok(s.light >= 0, `light must stay non-negative, got ${s.light}`)
    }
  }

  // A Thailand-like fleet writes the moto column from the country share —
  // the pre-2026-07 hardcoded 1 % wrote 3 moto/day on ~100 moto/hour roads.
  const th = splitAADT(1000, { motoTrafficShare: 0.2, tripsPerDwelling: 2.5 })
  assert.strictEqual(th.moto, 200)
  assert.strictEqual(th.light, 1000 - 10 - 20 - 200)

  // Determinism: identical inputs give identical integer outputs (idempotent
  // re-runs byte-compare stored ints against this candidate).
  assert.deepEqual(
    splitAADT(777.7, WORLD_FLEET),
    splitAADT(777.7, WORLD_FLEET),
  )
})
