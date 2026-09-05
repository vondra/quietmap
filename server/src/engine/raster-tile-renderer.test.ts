/** Exact dev1 pixel oracle for building heights, including as-used caps and emptiness. */

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import test from 'node:test'
import { crc32, inflateSync } from 'node:zlib'
import { renderBuildingVectorTile } from './raster-tile-renderer.js'

function rgbaFromPng(png: Buffer): Buffer {
  assert.deepEqual(png.subarray(0, 8), Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))
  const compressed: Buffer[] = []
  for (let offset = 8; offset < png.length;) {
    const length = png.readUInt32BE(offset)
    const type = png.toString('ascii', offset + 4, offset + 8)
    assert.equal(png.readUInt32BE(offset + 8 + length), crc32(png.subarray(offset + 4, offset + 8 + length)) >>> 0)
    if (type === 'IHDR') {
      assert.equal(png.readUInt32BE(offset + 8), 256)
      assert.equal(png.readUInt32BE(offset + 12), 256)
      assert.equal(png[offset + 16], 8)
      assert.equal(png[offset + 17], 6)
    }
    if (type === 'IDAT') compressed.push(png.subarray(offset + 8, offset + 8 + length))
    offset += 12 + length
  }
  const scanlines = inflateSync(Buffer.concat(compressed))
  assert.equal(scanlines.length, 256 * 1025)
  return Buffer.concat(Array.from({ length: 256 }, (_, row) => {
    assert.equal(scanlines[row * 1025], 0)
    return scanlines.subarray(row * 1025 + 1, (row + 1) * 1025)
  }))
}

test('all building pixels preserve the independently anchored dev1 palette and fill', async () => {
  const z = 14, x = 8840, y = 5580
  const point = (px: number, py: number): [number, number] => [
    Math.atan(Math.sinh(Math.PI * (1 - 2 * (y + py / 256) / 2 ** z))) * 180 / Math.PI,
    (x + px / 256) / 2 ** z * 360 - 180,
  ]
  const rectangle = (left: number, top: number, right: number, bottom: number) =>
    [[left, top], [right, top], [right, bottom], [left, bottom], [left, top]].map(([px, py]) => point(px, py))
  const rows = [3, 10, 20, 45, 80].map((height, i) => ({
    o: rectangle(12 + 45 * i, 22, 42 + 45 * i, 68), h: height, t: i, c: false,
  }))
  rows.push({ o: [[30, 110], [120, 110], [76, 175], [30, 110]].map(([px, py]) => point(px, py)), h: 45, t: 2, c: false })
  rows.push({ o: rectangle(50, 122, 90, 150), h: 3, t: 4, c: true })
  const pixels = rgbaFromPng(await renderBuildingVectorTile(z, x, y, async (...bounds) => {
    assert.deepEqual(bounds, [point(0, 256)[0], point(0, 0)[1], point(0, 0)[0], point(256, 0)[1]])
    return JSON.stringify(rows.map(({ o, ...row }) => ({ ...row, p: [[o]] })))
  }))
  // Independently executed original renderer at dev1 d6065653f06aafdb432206a3b0ca9dee277a73f0.
  // Hash covers all 262,144 RGBA bytes, not a self-generated expected output.
  assert.equal(createHash('sha256').update(pixels).digest('hex'),
    '2f981822b4b2ca8fcb96d7141235e2e5d62145f1de1afc9f7adb68c48c2e5fd8')
  assert.deepEqual([...pixels.subarray((140 * 256 + 70) * 4, (140 * 256 + 70) * 4 + 4)], [254, 224, 139, 210])
  const empty = rgbaFromPng(await renderBuildingVectorTile(z, x, y, async () => '[]'))
  assert.equal(empty.length, 262144)
  assert.ok(empty.every(value => value === 0))
  await assert.rejects(renderBuildingVectorTile(z, x, y, async () => { throw new Error('missing structures') }), /missing structures/)
  for (const invalid of ['not JSON', '{}', 'null', '""']) {
    await assert.rejects(renderBuildingVectorTile(z, x, y, async () => invalid))
  }
})

test('a crossing footprint paints both dateline columns exactly like translated interior tiles', async () => {
  const z = 14, y = 5580, axis = 2 ** z, middle = axis / 2
  const ring = [[-32, 40], [32, 40], [32, 120], [-32, 120], [-32, 40]].map(([px, py]) => [
    Math.atan(Math.sinh(Math.PI * (1 - 2 * (y + py / 256) / axis))) * 180 / Math.PI,
    (middle + px / 256) / axis * 360 - 180,
  ])
  const crossing = ring.map(([lat, lon]) => [lat, lon < 0 ? lon + 180 : lon - 180])
  const footprints = (o: number[][]) => JSON.stringify([{ p: [[o]], h: 45, t: 0, c: false }])
  for (const [interior, dateline, edgeColumn] of [[middle - 1, axis - 1, 255], [middle, 0, 0]]) {
    const expected = await renderBuildingVectorTile(z, interior, y, async () => footprints(ring))
    const actual = await renderBuildingVectorTile(z, dateline, y, async () => footprints(crossing))
    assert.deepEqual(actual, expected, `complete PNG at dateline column ${dateline}`)
    const pixels = rgbaFromPng(actual)
    assert.equal(pixels[(80 * 256 + edgeColumn) * 4 + 3], 210)
    assert.equal(pixels[(80 * 256 + 255 - edgeColumn) * 4 + 3], 0)
  }
})


test('multipart courtyards subtract only their own polygon in either winding and row order', async () => {
  const z = 14, x = 8840, y = 5580
  const point = (px: number, py: number) => [
    Math.atan(Math.sinh(Math.PI * (1 - 2 * (y + py / 256) / 2 ** z))) * 180 / Math.PI,
    (x + px / 256) / 2 ** z * 360 - 180,
  ]
  const rectangle = (left: number, top: number, right: number, bottom: number) =>
    [[left, top], [right, top], [right, bottom], [left, bottom]].map(([px, py]) => point(px, py))
  for (const reverse of [false, true]) {
    const ring = (coordinates: number[][]) => reverse ? coordinates.toReversed() : coordinates
    const block = { p: [
      [ring(rectangle(10, 10, 110, 110)), ring(rectangle(30, 30, 90, 90))],
      [ring(rectangle(150, 10, 200, 110))],
    ], h: 20, t: 0, c: false }
    const courtyardHouse = { p: [[rectangle(50, 50, 70, 70)]], h: 3, t: 0, c: false }
    for (const rows of [[block, courtyardHouse], [courtyardHouse, block]]) {
      const pixels = rgbaFromPng(await renderBuildingVectorTile(z, x, y, async () => JSON.stringify(rows)))
      const pixel = (px: number, py: number) => [...pixels.subarray((py * 256 + px) * 4, (py * 256 + px) * 4 + 4)]
      assert.deepEqual(pixel(20, 60), [244, 109, 67, 210], 'outer building')
      assert.deepEqual(pixel(40, 60), [0, 0, 0, 0], 'open courtyard')
      assert.deepEqual(pixel(60, 60), [254, 224, 139, 210], 'separate courtyard house survives')
      assert.deepEqual(pixel(130, 60), [0, 0, 0, 0], 'gap between parts')
      assert.deepEqual(pixel(175, 60), [244, 109, 67, 210], 'second part')
    }
  }
})
