// Unit tests for the grid raster tile renderer.
// Run: cd server && node --import tsx --test src/engine/raster-grid-renderer.test.ts

import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import test from 'node:test'
import { demColor, forestColor, imdColor, renderGridTile } from './raster-grid-renderer.js'

// The published native raster year; the real-file tests skip until it exists.
const PRAGUE_PREPARED = '/data/mixeduse2/r260910/rasters/2026'
const PRAGUE_RASTERS = {
  skip: !['dem.i16be', 'forest.u8', 'imd.u8'].every((file) => existsSync(`${PRAGUE_PREPARED}/z9/276/173/${file}`)),
}

test('dem palette: lowlands green, peaks white, water dark', () => {
  assert.deepEqual(demColor(-5), [0x2d, 0x6a, 0x4f, 160])
  assert.deepEqual(demColor(0), [0x2d, 0x6a, 0x4f, 160])
  const mid = demColor(100)
  assert.equal(mid[3], 160)
  // Interpolated between stop 0 and stop 200: red rises from 0x2d.
  assert.ok(mid[0] > 0x2d && mid[0] < 0x74)
  assert.deepEqual(demColor(5000), [0xf0, 0xf0, 0xf0, 160])
})

test('forest palette: transparent when bare, opaque green when dense', () => {
  assert.deepEqual(forestColor(0), [0, 0, 0, 0])
  const dense = forestColor(100)
  assert.deepEqual(dense.slice(0, 3), [0x2d, 0x6a, 0x4f])
  assert.ok(dense[3] >= 140)
})

test('imd palette: transparent when open, red when sealed', () => {
  assert.deepEqual(imdColor(0), [0, 0, 0, 0])
  const sealed = imdColor(100)
  assert.ok(sealed[0] > sealed[1] && sealed[3] > 150)
})

test('square window matches the stored file size (geometry cross-check)', PRAGUE_RASTERS, async () => {
  // engine/grid/src/raster.rs: byte_len = rows * columns * bytesPerNode.
  // A wrong edge formula still passes bounds checks but reads garbage —
  // this pins the ported geometry to the real files.
  const { statSync } = await import('node:fs')
  const { squareWindow } = await import('./raster-grid-renderer.js')
  const cases: [string, number, number, number][] = [
    ['dem.i16be', 276, 173, 2],
    ['forest.u8', 276, 173, 1],
    ['imd.u8', 276, 173, 1],
  ]
  for (const [file, sx, sy, bpn] of cases) {
    const size = statSync(`${PRAGUE_PREPARED}/z9/${sx}/${sy}/${file}`).size
    const w = squareWindow(sx, sy)
    assert.equal(size, w.rows * w.columns * bpn, `${file}: ${size} != ${w.rows}x${w.columns}x${bpn}`)
  }
})

test('renders a real dem tile over Prague', PRAGUE_RASTERS, async () => {
  // z10 tile covering Vaclavak (50.0755N, 14.4378E).
  const z = 10
  const x = Math.floor(((14.4378 + 180) / 360) * 2 ** z)
  const latRad = (50.0755 * Math.PI) / 180
  const y = Math.floor(((1 - Math.log(Math.tan(latRad) + 1 / Math.cos(latRad)) / Math.PI) / 2) * 2 ** z)
  const png = await renderGridTile(PRAGUE_PREPARED, 'dem', z, x, y)
  assert.ok(png.length > 1000, `png too small: ${png.length}`)
  assert.deepEqual(png.subarray(0, 8), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))
  // Not fully transparent: count opaque pixels in the raw expectation via size.
  const png2 = await renderGridTile(PRAGUE_PREPARED, 'forest', z, x, y)
  assert.ok(png2.length > 500)
  const png3 = await renderGridTile(PRAGUE_PREPARED, 'imd', z, x, y)
  assert.ok(png3.length > 500)
})

test('ocean tile over coverage-verified absence (0-byte squares) is transparent', PRAGUE_RASTERS, async () => {
  // z6 8/40: South Pacific around 45°S 130°W; every z9 square there is a 0-byte file.
  const png = await renderGridTile(PRAGUE_PREPARED, 'dem', 6, 8, 40)
  assert.ok(png.length > 100 && png.length < 2000)
})
