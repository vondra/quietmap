import assert from 'node:assert/strict'
import test from 'node:test'

import { displayedTileZoom, energySumLdenDb, hm3CellAt } from '../src/lib/hm3-sample.ts'
import { NO_DATA, TILE_PX } from '../src/lib/hm3-decoder.ts'

test('a point maps to the painted tile cell, wraps across the antimeridian and is null off the world', () => {
  const cell = hm3CellAt(14.1639, 49.8486, 12)
  assert.deepEqual({ z: cell.z, tx: cell.tx, ty: cell.ty }, { z: 12, tx: 2209, ty: 1391 })
  assert.ok(cell.px >= 0 && cell.px < TILE_PX && cell.py >= 0 && cell.py < TILE_PX)
  assert.equal(hm3CellAt(-180.5, 0, 3).tx, 7)
  assert.equal(hm3CellAt(0, 89.9, 3), null)
  assert.equal(hm3CellAt(0, NaN, 3), null)
})

test('the readout energy-sums the cell across tiles and is null where every tile is silent', () => {
  const cell = { z: 12, tx: 0, ty: 0, px: 7, py: 3 }
  const at60 = new Uint8Array(TILE_PX * TILE_PX).fill(NO_DATA)
  at60[cell.py * TILE_PX + cell.px] = 120 // HM3 encodes dB × 2
  assert.equal(energySumLdenDb([at60], cell), 60)
  assert.equal(energySumLdenDb([at60, at60, null], cell).toFixed(2), '63.01')
  assert.equal(energySumLdenDb([null, new Uint8Array(TILE_PX * TILE_PX).fill(NO_DATA)], cell), null)
})

test('the sampled tile zoom is the renderer\'s: rounded, one finer on HiDPI, clamped into the published band', () => {
  const build = { latest: 'b1', byLayer: {}, base: '', zoom: 13 }
  assert.equal(displayedTileZoom(build, 9.4, 1), 9)
  assert.equal(displayedTileZoom(build, 9.4, 2), 10)
  assert.equal(displayedTileZoom(build, 0.7, 1), 2)
  assert.equal(displayedTileZoom(build, 15.2, 2), 13)
})

function hm3Response(status = 200) {
  const bytes = new Uint8Array(6 + TILE_PX * TILE_PX).fill(NO_DATA)
  bytes.set([72, 77, 51, 32, 3, 1]) // "HM3 ", version 3, source 1
  return new Response(status === 200 ? bytes : null, { status })
}

test('the tile cache shares in-flight fetches, bounds itself to 64 settled tiles, and retries a failed tile after 30 s', async (t) => {
  const { tileCells } = await import('../src/lib/hm3-sample.ts')
  const fetched = []
  const failing = new Set(['/t/fail'])
  t.mock.method(globalThis, 'fetch', async (url) => { fetched.push(url); return hm3Response(failing.has(url) ? 503 : 200) })
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

  assert.equal(await tileCells('/t/fail'), null)
  assert.equal(await tileCells('/t/fail'), null) // silence, no refetch yet
  assert.equal(fetched.filter((u) => u === '/t/fail').length, 1)
  t.mock.timers.tick(30_000)
  failing.clear()
  assert.ok((await tileCells('/t/fail')) instanceof Uint8Array)
  assert.equal(fetched.filter((u) => u === '/t/fail').length, 2)
})
