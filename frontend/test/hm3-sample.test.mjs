// Heatmap cell readouts: the painted cell's loudness, silence, no level and failed tiles, the tile cache.
import assert from 'node:assert/strict'
import test from 'node:test'

import { displayedTileZoom, hm3CellAt, readHeatmapCell } from '../src/lib/hm3-sample.ts'
import { decodeHM3, HM3_NO_LEVEL, HM3_SILENCE, HM3_VERSION, TILE_PX, paethPrediction } from '../src/lib/hm3-decoder.ts'

test('a point maps to the painted tile cell, wraps across the antimeridian and is null off the world', () => {
  const cell = hm3CellAt(14.1639, 49.8486, 12)
  assert.deepEqual({ z: cell.z, tx: cell.tx, ty: cell.ty }, { z: 12, tx: 2209, ty: 1391 })
  assert.ok(cell.px >= 0 && cell.px < TILE_PX && cell.py >= 0 && cell.py < TILE_PX)
  assert.equal(hm3CellAt(-180.5, 0, 3).tx, 7)
  assert.equal(hm3CellAt(0, 89.9, 3), null)
  assert.equal(hm3CellAt(0, NaN, 3), null)
})

const CELL = { z: 12, tx: 0, ty: 0, px: 7, py: 3 }
function tileWith(byteAtCell, metric = 'nden') {
  const cells = new Uint8Array(TILE_PX * TILE_PX).fill(HM3_NO_LEVEL)
  cells[CELL.py * TILE_PX + CELL.px] = byteAtCell
  return { cells, metric }
}

test('the readout is the cell loudness: twice its loudness level in phon, 80 one sone', () => {
  assert.deepEqual(readHeatmapCell(tileWith(80), CELL), { kind: 'level', sone: 1 })
  assert.deepEqual(readHeatmapCell(tileWith(100), CELL), { kind: 'level', sone: 2 })
  assert.ok(Math.abs(readHeatmapCell(tileWith(60), CELL).sone - 0.5) < 1e-6)
})

test('an Lden cell reads its decibels: twice its level in half-dB steps', () => {
  assert.deepEqual(readHeatmapCell(tileWith(121, 'lden'), CELL), { kind: 'lden', db: 60.5 })
  assert.deepEqual(readHeatmapCell(tileWith(HM3_SILENCE, 'lden'), CELL), { kind: 'silence' })
})

test('a cell assessed under 1/16 sone reads silence, one not assessed no level, a failed tile unavailable', () => {
  assert.deepEqual(readHeatmapCell(tileWith(HM3_SILENCE), CELL), { kind: 'silence' })
  assert.deepEqual(readHeatmapCell(tileWith(HM3_NO_LEVEL), CELL), { kind: 'no-level' })
  assert.deepEqual(readHeatmapCell('absent', CELL), { kind: 'no-level' })
  assert.deepEqual(readHeatmapCell('failed', CELL), { kind: 'unavailable' })
})

test('HM3 prediction restores every byte across row boundaries and refuses other formats', () => {
  const cells = Uint8Array.from({ length: TILE_PX * TILE_PX }, (_, at) =>
    (Math.floor(at / TILE_PX) * 19 + at % TILE_PX * 31) % 256)
  const bytes = predictedTile(cells)
  const decoded = decodeHM3(bytes.slice().buffer)
  assert.equal(decoded.sourceId, 1)
  assert.equal(decoded.metric, 'nden')
  assert.deepEqual(decoded.cells, cells)
  const lden = bytes.slice()
  lden.set([72, 77, 68, 32, 1])
  assert.equal(decodeHM3(lden.slice().buffer).metric, 'lden')
  lden[4] = HM3_VERSION
  assert.throws(() => decodeHM3(lden.buffer), /HMD version 7 is not 1/)
  bytes[4] = 5
  assert.throws(() => decodeHM3(bytes.buffer), /HM3 version 5 is not 7/)
  bytes[4] = HM3_VERSION
  bytes[5] = 7
  assert.throws(() => decodeHM3(bytes.buffer), /unknown HM3 layer/)
  assert.throws(() => decodeHM3(bytes.slice(1).buffer), /HM3 size/)
})

test('the sampled tile zoom is the renderer\'s: rounded, one finer on HiDPI, clamped into the published band', () => {
  const build = { latest: 'b1', byLayer: {}, zoom: 13 }
  assert.equal(displayedTileZoom(build, 9.4, 1), 9)
  assert.equal(displayedTileZoom(build, 9.4, 2), 10)
  assert.equal(displayedTileZoom(build, 0.7, 1), 2)
  assert.equal(displayedTileZoom(build, 15.2, 2), 13)
})

function predictedTile(cells) {
  const bytes = new Uint8Array(6 + cells.length)
  bytes.set([72, 77, 51, 32, HM3_VERSION, 1])
  for (let at = 0; at < cells.length; at++) {
    const left = at % TILE_PX ? cells[at - 1] : 0
    const above = at >= TILE_PX ? cells[at - TILE_PX] : 0
    const upperLeft = at >= TILE_PX && at % TILE_PX ? cells[at - TILE_PX - 1] : 0
    bytes[6 + at] = cells[at] - paethPrediction(left, above, upperLeft)
  }
  return bytes
}

function hm3Response(status = 200, empty = false) {
  const bytes = predictedTile(new Uint8Array(TILE_PX * TILE_PX).fill(HM3_NO_LEVEL))
  return new Response(status === 200 && !empty ? bytes : null, { status })
}

test('the tile cache shares in-flight fetches, bounds itself to 64 settled tiles, reads an absent tile as absent, and retries a failed tile after 30 s', async (t) => {
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
  assert.ok(cells.every((c) => c.cells instanceof Uint8Array && c.cells.length === TILE_PX * TILE_PX && c.metric === 'nden'))
  await tileCells('/t/69') // newest: still cached
  assert.equal(fetched.length, 70)
  await tileCells('/t/0') // oldest: evicted once the burst settled past the limit
  assert.equal(fetched.length, 71)

  assert.equal(await tileCells('/t/absent'), 'absent')
  assert.equal(await tileCells('/t/fail'), 'failed')
  assert.equal(await tileCells('/t/fail'), 'failed') // no refetch yet
  assert.equal(fetched.filter((u) => u === '/t/fail').length, 1)
  t.mock.timers.tick(30_000)
  failing.clear()
  assert.ok((await tileCells('/t/fail')).cells instanceof Uint8Array)
  assert.equal(fetched.filter((u) => u === '/t/fail').length, 2)
})
