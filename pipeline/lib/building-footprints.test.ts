/**
 * Focused tests for lib/building-footprints.ts (task #15).
 * Run: `npx tsx --test pipeline/lib/building-footprints.test.ts`
 *
 * Builds a synthetic obstacle store (one H3 R4 cell dir + an `.ingested-tiles`
 * manifest) so the tests pin the window geometry, the WKB area arithmetic and
 * — the one that decides whether a country silently loses its legal speeds —
 * the three-state coverage contract, without touching the 309 GB real store.
 */

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Binary, Table, tableToIPC, vectorFromArray } from 'apache-arrow'
import { latLngToCell } from 'h3-js'
import {
  BuildingFootprintSampler,
  BUILT_UP_UNKNOWN,
  BUILT_UP_RURAL,
  BUILT_UP_URBAN,
  BUILT_UP_MIN_BUILT_PIXELS,
  BUILT_UP_WINDOW_HALF_DEG,
} from './building-footprints.js'

/** Test point well inside N49E014 so all four window corners share one tile. */
const LAT = 49.5
const LON = 14.5
const TILE = 'N49E014'

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
}

/** A store holding `footprints`, plus a manifest listing `tiles`. */
function makeStore(footprints: Footprint[], tiles: string[]) {
  const root = mkdtempSync(join(tmpdir(), 'obstacle-store-test-'))
  const storeDir = join(root, 'h3r4')
  const manifestPath = join(root, '.ingested-tiles')
  writeFileSync(manifestPath, tiles.join('\n') + '\n')
  if (footprints.length > 0) {
    const cell = latLngToCell(footprints[0].lat, footprints[0].lon, 4)
    mkdirSync(join(storeDir, cell), { recursive: true })
    const table = new Table({
      polygon_wkb: vectorFromArray(
        footprints.map((f) => f.wkb),
        new Binary(),
      ),
      height_m: vectorFromArray(Float32Array.from(footprints.map(() => 8))),
      centroid_lat: vectorFromArray(Float64Array.from(footprints.map((f) => f.lat))),
      centroid_lon: vectorFromArray(Float64Array.from(footprints.map((f) => f.lon))),
    })
    writeFileSync(join(storeDir, cell, `obstacles-${TILE}.arrow`), tableToIPC(table, 'file'))
  }
  return { root, sampler: new BuildingFootprintSampler(storeDir, manifestPath, 4) }
}

test('building-footprints probe', async (t) => {
  await t.test('tile naming uses the SW corner (floor), S/W for negatives', () => {
    assert.equal(BuildingFootprintSampler.tileNameFor(49.78, 14.17), 'N49E014')
    assert.equal(BuildingFootprintSampler.tileNameFor(53.928, -1.387), 'N53W002')
    assert.equal(BuildingFootprintSampler.tileNameFor(-1.5, -0.5), 'S02W001')
  })

  await t.test('the window is BUILT_UP_WINDOW_HALF_DEG in BOTH axes', () => {
    const inside = BUILT_UP_WINDOW_HALF_DEG * 0.9
    const outside = BUILT_UP_WINDOW_HALF_DEG * 1.1
    const at = (dLat: number, dLon: number): Footprint => ({
      lat: LAT + dLat,
      lon: LON + dLon,
      wkb: polygonWkb([squareRing(LAT + dLat, LON + dLon, 20)]),
    })
    const { root, sampler } = makeStore(
      [at(0, 0), at(inside, 0), at(0, inside), at(outside, 0), at(0, outside)],
      [TILE],
    )
    assert.equal(sampler.windowFootprints(LAT, LON)!.count, 3)
    rmSync(root, { recursive: true, force: true })
  })

  await t.test('area is outer ring minus holes, and drives the pixel estimate', () => {
    // 100 m square with a 50 m courtyard = 10 000 − 2 500 = 7 500 m².
    const wkb = polygonWkb([squareRing(LAT, LON, 100), squareRing(LAT, LON, 50)])
    const { root, sampler } = makeStore([{ lat: LAT, lon: LON, wkb }], [TILE])
    const w = sampler.windowFootprints(LAT, LON)!
    assert.ok(Math.abs(w.areaM2 - 7500) < 50, `expected ~7500 m², got ${w.areaM2}`)
    // One raster pixel at 49.5° N is (111132/3600) × (111320·cos49.5/3600) ≈ 620 m².
    const px = sampler.estimatedBuiltPixels(LAT, LON)!
    assert.ok(Math.abs(px - w.areaM2 / 620) < 0.5, `pixel estimate off: ${px}`)
    rmSync(root, { recursive: true, force: true })
  })

  await t.test('threshold decides urban vs rural', () => {
    const big = polygonWkb([squareRing(LAT, LON, 200)]) // 40 000 m² ≈ 65 px
    const small = polygonWkb([squareRing(LAT, LON, 30)]) // 900 m² ≈ 1.5 px
    const urban = makeStore([{ lat: LAT, lon: LON, wkb: big }], [TILE])
    assert.ok(BUILT_UP_MIN_BUILT_PIXELS < 60, 'fixture must clear the threshold')
    assert.equal(urban.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_URBAN)
    rmSync(urban.root, { recursive: true, force: true })
    const rural = makeStore([{ lat: LAT, lon: LON, wkb: small }], [TILE])
    assert.equal(rural.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_RURAL)
    rmSync(rural.root, { recursive: true, force: true })
  })

  await t.test('coverage is the manifest, not the shards: empty ≠ missing', () => {
    // Tile ingested, no shard for the cell → the ingest proved it holds no
    // footprint → RURAL. Guessing UNKNOWN here would send every empty cell in
    // the world back to the legacy speed table.
    const empty = makeStore([], [TILE])
    assert.equal(empty.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_RURAL)
    rmSync(empty.root, { recursive: true, force: true })
    // Tile never ingested → UNKNOWN, whatever else the manifest lists.
    const unseen = makeStore([], ['N50E014'])
    assert.equal(unseen.sampler.classifyBuiltUp(LAT, LON), BUILT_UP_UNKNOWN)
    rmSync(unseen.root, { recursive: true, force: true })
  })

  await t.test('a window straddling two tiles needs BOTH ingested', () => {
    // 1 m north of the 50° line: the window reaches into N50E014 as well.
    const nearLine = 50 - 1 / 111_132
    const one = makeStore([], ['N49E014'])
    assert.equal(one.sampler.classifyBuiltUp(nearLine, LON), BUILT_UP_UNKNOWN)
    rmSync(one.root, { recursive: true, force: true })
    const both = makeStore([], ['N49E014', 'N50E014'])
    assert.equal(both.sampler.classifyBuiltUp(nearLine, LON), BUILT_UP_RURAL)
    rmSync(both.root, { recursive: true, force: true })
  })

  await t.test('no manifest at all → UNKNOWN, never a guessed rural', () => {
    const root = mkdtempSync(join(tmpdir(), 'obstacle-store-test-'))
    const sampler = new BuildingFootprintSampler(join(root, 'h3r4'), join(root, '.ingested-tiles'))
    assert.equal(sampler.classifyBuiltUp(LAT, LON), BUILT_UP_UNKNOWN)
    rmSync(root, { recursive: true, force: true })
  })
})
