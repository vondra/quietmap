/**
 * Focused tests for lib/building-footprints.ts (task #15).
 * Run: `npx tsx --test pipeline/lib/building-footprints.test.ts`
 *
 * Builds a synthetic prepared tree (one H3 R4 cell dir + a structures.arrow
 * per cell) so the tests pin the window geometry, the WKB area arithmetic, the
 * Overture-stock row mask and — the one that decides whether a country
 * silently loses its legal speeds — the three-state coverage contract, without
 * touching the real tree.
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Binary, Float64, Int64, Table, Uint8, tableToIPC, vectorFromArray } from 'apache-arrow'
import { cellToBoundary, latLngToCell } from 'h3-js'
import {
  BuildingFootprintSampler,
  BUILT_UP_UNKNOWN,
  BUILT_UP_RURAL,
  BUILT_UP_URBAN,
  BUILT_UP_MIN_BUILT_PIXELS,
  BUILT_UP_WINDOW_HALF_DEG,
} from './building-footprints.js'

/** Test point well inside one R4 cell so a plain window touches exactly one. */
const LAT = 49.5
const LON = 14.5

/** Little-endian 2D WKB. `rings[0]` is the outer ring; the rest are holes. */
function polygonWkb(rings: [number, number][][]): Uint8Array {
  const size = 9 + rings.reduce((n, r) => n + 4 + r.length * 16, 0)
  const buf = new Uint8Array(size)
  const view = new DataView(buf.buffer)
  let p = 0
  view.setUint8(p, 1)
  p += 1
  view.setUint32(p, 3, true)
  p += 4
  view.setUint32(p, rings.length, true)
  p += 4
  for (const ring of rings) {
    view.setUint32(p, ring.length, true)
    p += 4
    for (const [lat, lon] of ring) {
      view.setFloat64(p, lon, true)
      view.setFloat64(p + 8, lat, true)
      p += 16
    }
  }
  return buf
}

/** Closed axis-aligned ring of `side` metres centred on (lat, lon). */
function squareRing(lat: number, lon: number, side: number): [number, number][] {
  const dLat = side / 2 / 111_132
  const dLon = side / 2 / (111_320 * Math.cos((lat * Math.PI) / 180))
  return [
    [lat - dLat, lon - dLon],
    [lat - dLat, lon + dLon],
    [lat + dLat, lon + dLon],
    [lat + dLat, lon - dLon],
    [lat - dLat, lon - dLon],
  ]
}

interface Footprint {
  lat: number
  lon: number
  wkb: Uint8Array
  /** 'only' = OSM-only row (osm_id set, no emission override — out of the
   *  sampled stock); 'matched' = OSM↔Overture pair (Overture geometry, the
   *  emission centroid override proves it — sampled). Absent = Overture-only. */
  osm?: 'only' | 'matched'
}

/** One cell's structures.arrow rows; an empty list writes the 0-row table. */
function cellArrow(rows: Footprint[]): Uint8Array {
  const table = new Table({
    kind: vectorFromArray(Uint8Array.from(rows.map(() => 0)), new Uint8()),
    geometry_wkb: vectorFromArray(rows.map((f) => f.wkb), new Binary()),
    centroid_lat: vectorFromArray(Float64Array.from(rows.map((f) => f.lat)), new Float64()),
    centroid_lon: vectorFromArray(Float64Array.from(rows.map((f) => f.lon)), new Float64()),
    osm_id: vectorFromArray(rows.map((f) => (f.osm ? 1n : null)), new Int64()),
    emission_centroid_lat: vectorFromArray(
      rows.map((f) => (f.osm === 'matched' ? f.lat : null)),
      new Float64(),
    ),
  })
  return tableToIPC(table, 'file')
}

/** A prepared-tree stub holding one structures.arrow per entry. */
function makeWorld(entries: { cell: string; rows: Footprint[] }[]) {
  const root = mkdtempSync(join(tmpdir(), 'structures-store-test-'))
  for (const { cell, rows } of entries) {
    mkdirSync(join(root, cell), { recursive: true })
    writeFileSync(join(root, cell, 'structures.arrow'), cellArrow(rows))
  }
  return { root, sampler: new BuildingFootprintSampler(root, 4) }
}

/** The cells the sampler's 3×3 window grid touches around (lat, lon). */
function touchedCells(lat: number, lon: number): string[] {
  const h = BUILT_UP_WINDOW_HALF_DEG
  const cells = new Set<string>()
  for (const la of [lat - h, lat, lat + h]) {
    for (const lo of [lon - h, lon, lon + h]) cells.add(latLngToCell(la, lo, 4))
  }
  return [...cells]
}

test('building-footprints probe', async (t) => {
  await t.test('the window is BUILT_UP_WINDOW_HALF_DEG in BOTH axes', () => {
    const inside = BUILT_UP_WINDOW_HALF_DEG * 0.9
    const outside = BUILT_UP_WINDOW_HALF_DEG * 1.1
    const at = (dLat: number, dLon: number): Footprint => ({
      lat: LAT + dLat,
      lon: LON + dLon,
      wkb: polygonWkb([squareRing(LAT + dLat, LON + dLon, 20)]),
    })
    const rows = [at(0, 0), at(inside, 0), at(0, inside), at(outside, 0), at(0, outside)]
    const cells = [...new Set(rows.map((f) => latLngToCell(f.lat, f.lon, 4)))]
    const { root, sampler } = makeWorld(cells.map((cell) => ({ cell, rows })))
    assert.ok(Math.abs(sampler.windowFootprintAreaM2(LAT, LON)! - 1200) < 30)
    rmSync(root, { recursive: true, force: true })
  })

  await t.test('area is outer ring minus holes, and drives the pixel estimate', () => {
    // 100 m square with a 50 m courtyard = 10 000 − 2 500 = 7 500 m².
    const wkb = polygonWkb([squareRing(LAT, LON, 100), squareRing(LAT, LON, 50)])
    const { root, sampler } = makeWorld([{ cell: latLngToCell(LAT, LON, 4), rows: [{ lat: LAT, lon: LON, wkb }] }])
    const areaM2 = sampler.windowFootprintAreaM2(LAT, LON)!
    assert.ok(Math.abs(areaM2 - 7500) < 50, `expected ~7500 m², got ${areaM2}`)
    // One raster pixel at 49.5° N is (111132/3600) × (111320·cos49.5/3600) ≈ 620 m².
    const px = sampler.estimatedBuiltPixels(LAT, LON)!
    assert.ok(Math.abs(px - areaM2 / 620) < 0.5, `pixel estimate off: ${px}`)
    rmSync(root, { recursive: true, force: true })
  })

  await t.test('threshold decides urban vs rural', () => {
    const cell = latLngToCell(LAT, LON, 4)
    const big = polygonWkb([squareRing(LAT, LON, 200)]) // 40 000 m² ≈ 65 px
    const small = polygonWkb([squareRing(LAT, LON, 30)]) // 900 m² ≈ 1.5 px
    const urban = makeWorld([{ cell, rows: [{ lat: LAT, lon: LON, wkb: big }] }])
    assert.ok(BUILT_UP_MIN_BUILT_PIXELS < 60, 'fixture must clear the threshold')
    assert.equal(urban.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_URBAN)
    rmSync(urban.root, { recursive: true, force: true })
    const rural = makeWorld([{ cell, rows: [{ lat: LAT, lon: LON, wkb: small }] }])
    assert.equal(rural.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_RURAL)
    rmSync(rural.root, { recursive: true, force: true })
  })

  await t.test('coverage is the per-cell file: a present 0-row table ≠ a missing one', () => {
    const cell = latLngToCell(LAT, LON, 4)
    // structures.arrow present and empty — the builder swept this cell and
    // found nothing → RURAL. Guessing UNKNOWN here would send every empty cell
    // in the world back to the legacy speed table.
    const empty = makeWorld([{ cell, rows: [] }])
    assert.equal(empty.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_RURAL)
    rmSync(empty.root, { recursive: true, force: true })
    // No structures.arrow at all → the builder never reached this cell → UNKNOWN.
    const unseen = makeWorld([])
    assert.equal(unseen.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_UNKNOWN)
    rmSync(unseen.root, { recursive: true, force: true })
  })

  await t.test('a window straddling cells needs EVERY touched cell covered', () => {
    // A cell vertex: the ±h window corners land in the cells that meet there.
    const vertex = cellToBoundary(latLngToCell(LAT, LON, 4))[0]
    const cells = touchedCells(vertex[0], vertex[1])
    assert.ok(cells.length > 1, 'fixture point must straddle cells')
    const one = makeWorld([{ cell: cells[0], rows: [] }])
    assert.equal(one.sampler.classifyBuiltUp(vertex[0], vertex[1]), BUILT_UP_UNKNOWN)
    rmSync(one.root, { recursive: true, force: true })
    const all = makeWorld(cells.map((cell) => ({ cell, rows: [] })))
    assert.equal(all.sampler.classifyBuiltUp(vertex[0], vertex[1]), BUILT_UP_RURAL)
    rmSync(all.root, { recursive: true, force: true })
  })

  await t.test('the sampled stock is the pre-merge Overture set: OSM-only rows stay out', () => {
    const cell = latLngToCell(LAT, LON, 4)
    const wkb = polygonWkb([squareRing(LAT, LON, 200)]) // 65 px when counted
    const rows: Footprint[] = [
      { lat: LAT, lon: LON, wkb }, // Overture-only → counted
      { lat: LAT, lon: LON, wkb, osm: 'matched' }, // matched pair → counted (Overture geometry)
      { lat: LAT, lon: LON, wkb, osm: 'only' }, // OSM-only → NOT counted (calibration stock)
    ]
    const { root, sampler } = makeWorld([{ cell, rows }])
    // Counted twice (130 px ⇒ urban); an OSM-only leak would make it 195 px —
    // same class here, so assert the AREA, where the difference is exact.
    assert.ok(Math.abs(sampler.windowFootprintAreaM2(LAT, LON)! - 80_000) < 200)
    rmSync(root, { recursive: true, force: true })
  })
})
