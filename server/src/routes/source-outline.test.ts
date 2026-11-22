// Whole source boundaries come from the requested year's read-only index, including separate parts.
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import test from 'node:test'
import Fastify from 'fastify'
import { sourceOutlineRoutes } from './source-outline.ts'

test('outlines preserve every mapped part and distinguish absent geometry from unavailable data', async t => {
  const preparedDir = mkdtempSync(join(tmpdir(), 'source-outline-'))
  t.after(() => rmSync(preparedDir, { recursive: true, force: true }))
  mkdirSync(join(preparedDir, '2026'))
  const file = join(preparedDir, '2026', 'source-outlines.sqlite')
  const database = new DatabaseSync(file)
  database.exec('CREATE TABLE outlines (id TEXT NOT NULL, geometry TEXT NOT NULL, PRIMARY KEY (id, geometry)) WITHOUT ROWID')
  const id = 'fedcba9876543210'
  const polygons = [0, 10].map(x => ({ type: 'Polygon', coordinates: [[[x, 0], [x + 1, 0], [x, 1], [x, 0]]] }))
  for (const polygon of polygons) database.prepare('INSERT INTO outlines VALUES (?, ?)').run(id, JSON.stringify(polygon))
  database.close()
  const app = Fastify()
  t.after(() => app.close())
  await app.register(sourceOutlineRoutes, { preparedDir, years: ['2026', '2025'] })
  const response = await app.inject(`/api/source-outline/${id}`)
  assert.equal(response.statusCode, 200)
  assert.equal(response.headers['cache-control'], 'no-cache')
  assert.deepEqual(response.json(), {
    type: 'FeatureCollection',
    features: polygons.map(geometry => ({ type: 'Feature', properties: {}, geometry })),
  })
  const absent = await app.inject('/api/source-outline/0000000000000000')
  assert.equal(absent.statusCode, 200)
  assert.deepEqual(absent.json().features, [])
  assert.equal((await app.inject(`/api/source-outline/${id}?year=2025`)).statusCode, 503)
  for (const url of [`${id}?year=2024`, `${id}?year=../../2026`, 'not-an-id']) {
    assert.equal((await app.inject(`/api/source-outline/${url}`)).statusCode, 400)
  }
})
