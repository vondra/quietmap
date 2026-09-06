/** Admission and stable strict-radius matching of preserved national building observations. */

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { NATIONAL_BUILDING_SOURCES, indexNationalBuildings } from './buildings-national-source.js'

const [cz, es] = NATIONAL_BUILDING_SOURCES

test('array/JSONL admission, original identity, strict 30m and first-observation ties', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'national-source-test-'))
  try {
    for (const source of [cz, es]) {
      const lat = source.country === 'CZ' ? 50 : 40
      const lon = source.country === 'CZ' ? 14 : -3
      const records = [
        { lat, lon, floors: 2, useCode: 10 },
        { lat, lon, floors: 7, useCode: 9 },
        { lat: lat + 0.01, lon, floors: source.country === 'CZ' ? 300 : 255, useCode: 999 },
      ]
      const path = resolve(work, source.country + '.json')
      const bytes = source.country === 'CZ' ? JSON.stringify(records) : records.map(r => JSON.stringify(r)).join('\n')
      writeFileSync(path, bytes)
      const before = statSync(path, { bigint: true })
      const index = await indexNationalBuildings(path, resolve(work, source.country + '.sqlite'), source)
      try {
        assert.equal(index.receipt.sha256, createHash('sha256').update(bytes).digest('hex'))
        assert.equal(index.receipt.records, 3)
        assert.equal(index.nearest(lat, lon)?.floors, 2, 'equal-distance records retain source order')
        assert.equal(index.nearest(lat + 29.999 / 110540, lon)?.floors, 2)
        assert.equal(index.nearest(lat + 30.001 / 110540, lon), null)
        assert.equal(index.nearest(lat + 0.01, lon)?.floors, 255)
        assert.equal(index.nearest(lat + 0.01, lon)?.buildingType, null, 'unknown use does not invent a type')
        assert.equal(index.nearest(lat, lon)?.buildingType, source.country === 'CZ' ? 1 : null)
      } finally { index.close() }
      const after = statSync(path, { bigint: true })
      for (const field of ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs'] as const) assert.equal(after[field], before[field])
      assert.equal(readFileSync(path, 'utf8'), bytes)
    }
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('missing, empty, malformed, nonfinite and inconsistent source records fail admission', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'national-admission-test-'))
  try {
    await assert.rejects(indexNationalBuildings(resolve(work, 'missing'), resolve(work, 'missing.sqlite'), cz), /ENOENT/)
    const inputs = [
      [cz, '[]'], [es, ' \n'], [cz, '{}'], [es, '{"lat":40,"lon":-3,"floors":2}\nbroken'],
      [cz, '[{"lat":1e999,"lon":14,"floors":2,"useCode":7}]'],
      [cz, '[{"lat":50,"lon":14,"floors":2.5,"useCode":7}]'],
      [cz, '[{"lat":50,"lon":14,"floors":2,"useCode":"7"}]'],
      [es, '{"lat":40,"lon":-3,"floors":0}'],
      [es, '{"lat":40,"lon":-3,"floors":256}'],
      [es, '{"lat":50,"lon":-3,"floors":2}'],
    ] as const
    for (const [i, [source, content]] of inputs.entries()) {
      const path = resolve(work, `${i}.json`)
      writeFileSync(path, content)
      await assert.rejects(indexNationalBuildings(path, resolve(work, `${i}.sqlite`), source))
      assert.equal(readFileSync(path, 'utf8'), content)
    }
  } finally { rmSync(work, { recursive: true, force: true }) }
})
