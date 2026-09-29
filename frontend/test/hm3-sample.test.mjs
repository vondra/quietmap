import assert from 'node:assert/strict'
import test from 'node:test'

import { displayedTileZoom, hm3CellAt, readHeatmapCell } from '../src/lib/hm3-sample.ts'
import { decodeHM3, HM3_COMPUTED_SILENCE, HM3_NOT_ASSESSED, TILE_PX } from '../src/lib/hm3-decoder.ts'

test('a point maps to the painted tile cell, wraps across the antimeridian and is null off the world', () => {
  const cell = hm3CellAt(14.1639, 49.8486, 12)
  assert.deepEqual({ z: cell.z, tx: cell.tx, ty: cell.ty }, { z: 12, tx: 2209, ty: 1391 })
  assert.ok(cell.px >= 0 && cell.px < TILE_PX && cell.py >= 0 && cell.py < TILE_PX)
  assert.equal(hm3CellAt(-180.5, 0, 3).tx, 7)
  assert.equal(hm3CellAt(0, 89.9, 3), null)
  assert.equal(hm3CellAt(0, NaN, 3), null)
})

const CELL = { z: 12, tx: 0, ty: 0, px: 7, py: 3 }
function tileWith(byteAtCell, fill = HM3_NOT_ASSESSED) {
  const cells = new Uint8Array(TILE_PX * TILE_PX).fill(fill)
  cells[CELL.py * TILE_PX + CELL.px] = byteAtCell
  return cells
}

test('the readout energy-sums the cell across layers; computed silence adds nothing', () => {
  const at60 = tileWith(120) // HM3 encodes dB × 2
  assert.deepEqual(readHeatmapCell([{ source: 'road', tile: at60 }], CELL), { kind: 'level', ldenDb: 60 })
  const both = readHeatmapCell([{ source: 'road', tile: at60 }, { source: 'rail', tile: at60 }], CELL)
  assert.equal(both.ldenDb.toFixed(2), '63.01')
  const withSilence = readHeatmapCell([{ source: 'road', tile: at60 }, { source: 'rail', tile: tileWith(HM3_COMPUTED_SILENCE) }], CELL)
  assert.deepEqual(withSilence, { kind: 'level', ldenDb: 60 })
})

test('a failed selected layer yields no number, never the sum of the rest', () => {
  const road40 = { source: 'road', tile: tileWith(80) }
  assert.deepEqual(readHeatmapCell([road40, { source: 'rail', tile: 'failed' }], CELL), { kind: 'unavailable', failedSources: ['rail'] })
})

test('silence in every layer is no modelled source; an unassessed cell or an absent tile is not assessed', () => {
  const silent = { source: 'road', tile: tileWith(HM3_COMPUTED_SILENCE) }
  assert.deepEqual(readHeatmapCell([silent, { ...silent, source: 'rail' }], CELL), { kind: 'no-modelled-source' })
  assert.deepEqual(readHeatmapCell([silent, { source: 'rail', tile: tileWith(HM3_NOT_ASSESSED, 120) }], CELL), { kind: 'not-assessed' })
  assert.deepEqual(readHeatmapCell([silent, { source: 'rail', tile: 'not-assessed' }], CELL), { kind: 'not-assessed' })
})

test('only HM3 version 4 decodes', () => {
  const bytes = new Uint8Array(6 + TILE_PX * TILE_PX).fill(HM3_NOT_ASSESSED)
  bytes.set([72, 77, 51, 32, 4, 1])
  assert.equal(decodeHM3(bytes.buffer).sourceId, 1)
  bytes[4] = 3
  assert.throws(() => decodeHM3(bytes.buffer), /HM3 version 3 != 4/)
})

test('the sampled tile zoom is the renderer\'s: rounded, one finer on HiDPI, clamped into the published band', () => {
  const build = { latest: 'b1', byLayer: {}, base: '', zoom: 13 }
  assert.equal(displayedTileZoom(build, 9.4, 1), 9)
  assert.equal(displayedTileZoom(build, 9.4, 2), 10)
  assert.equal(displayedTileZoom(build, 0.7, 1), 2)
  assert.equal(displayedTileZoom(build, 15.2, 2), 13)
})

function hm3Response(status = 200, empty = false) {
  const bytes = new Uint8Array(6 + TILE_PX * TILE_PX).fill(HM3_NOT_ASSESSED)
  bytes.set([72, 77, 51, 32, 4, 1]) // "HM3 ", version 4, source 1
  return new Response(status === 200 && !empty ? bytes : null, { status })
}

test('the tile cache shares in-flight fetches, bounds itself to 64 settled tiles, reads an absent tile as not assessed, and retries a failed tile after 30 s', async (t) => {
  const { tileCells } = await import('../src/lib/hm3-sample.ts')
  const fetched = []
  const failing = new Set(['/t/fail'])
  t.mock.method(globalThis, 'fetch', async (url) => { fetched.push(url); return hm3Response(failing.has(url) ? 503 : 200, url === '/t/absent') })
  t.mock.timers.enable({ apis: ['setTimeout'] })

  const urls = Array.from({ length: 70 }, (_, i) => `/t/${i}`)
  const burst = urls.map((u) => tileCells(u))
  urls.forEach((u) => tileCells(u)) // concurrent second readers share the promise
  assert.equal(fetched.length, 70)
  const cells = await Promise.all(burst)
  assert.ok(cells.every((c) => c instanceof Uint8Array && c.length === TILE_PX * TILE_PX))
  await tileCells('/t/69') // newest: still cached
  assert.equal(fetched.length, 70)
  await tileCells('/t/0') // oldest: evicted once the burst settled past the limit
  assert.equal(fetched.length, 71)

  assert.equal(await tileCells('/t/absent'), 'not-assessed')
  assert.equal(await tileCells('/t/fail'), 'failed')
  assert.equal(await tileCells('/t/fail'), 'failed') // no refetch yet
  assert.equal(fetched.filter((u) => u === '/t/fail').length, 1)
  t.mock.timers.tick(30_000)
  failing.clear()
  assert.ok((await tileCells('/t/fail')) instanceof Uint8Array)
  assert.equal(fetched.filter((u) => u === '/t/fail').length, 2)
})
