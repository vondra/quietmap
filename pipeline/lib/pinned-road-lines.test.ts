/** Strict source admission and multipart indexing tests for national road lines. */

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { after, test } from 'node:test'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { buildRoadLineVertexGrid, loadPinnedRoadLines, nearestRoadLine } from './pinned-road-lines.js'

const DIRECTORY = mkdtempSync(join(tmpdir(), 'pinned-road-lines-test-'))
after(() => rmSync(DIRECTORY, { recursive: true, force: true }))
const options = { preparedDirectory: DIRECTORY, enrichmentDirectory: DIRECTORY,
  enrichOnly: true, forceDownload: false }

function writeSource(name: string, value: unknown) {
  const bytes = Buffer.from(JSON.stringify(value))
  writeFileSync(join(DIRECTORY, name), bytes)
  return { relativePath: name, sha256: createHash('sha256').update(bytes).digest('hex') }
}

test('line loader validates every feature and preserves multipart separation', () => {
  const file = writeSource('roads.geojson', { type: 'FeatureCollection', features: [
    { type: 'Feature', properties: { class: 'A' }, geometry: { type: 'MultiLineString',
      coordinates: [[[1, 2], [1.01, 2]], [[9, 8], [9.01, 8]]] } },
    { type: 'Feature', geometry: { type: 'Point', coordinates: [1, 2] } },
    { type: 'Feature', geometry: { type: 'LineString', coordinates: [[1, 95], [2, 2]] } },
  ] })
  const loaded = loadPinnedRoadLines(options, [file])
  assert.deepEqual({ rows: loaded.sourceRows, lines: loaded.lines.length,
    invalid: loaded.invalidGeometrySkipped }, { rows: 3, lines: 2, invalid: 2 })
  assert.equal(loaded.lines[0].properties.class, 'A')
  const grid = buildRoadLineVertexGrid(loaded.lines)
  assert.equal(nearestRoadLine(2, 1.005, grid, 1000), loaded.lines[0])
  assert.equal(nearestRoadLine(5, 5, grid, 1000), null, 'multipart endpoints never form a phantom connector')
})

test('line loader rejects a byte mismatch before returning partial sources', () => {
  const file = writeSource('wrong.geojson', { type: 'FeatureCollection', features: [] })
  assert.throws(() => loadPinnedRoadLines(options, [{ ...file, sha256: '0'.repeat(64) }]), /SHA-256/)
})
