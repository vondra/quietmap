// Whole source boundaries come from the requested year's read-only index, including separate parts.
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import test from 'node:test'
import Fastify from 'fastify'
import { sourceOutlineRoutes } from './source-outline.ts'

function markComplete(yearRoot: string, year: string): void {
  const manifest = JSON.stringify({ year, as_of: `${year}-12-31` })
  writeFileSync(join(yearRoot, 'year.json'), manifest)
  const digest = createHash('sha256').update(manifest).digest('hex')
  writeFileSync(join(yearRoot, 'complete'), `qmsq1\n${JSON.stringify({ manifest_sha256: digest, as_of: `${year}-12-31`, note: 'test' })}`)
}

test('outlines preserve every mapped part and distinguish absent geometry from unavailable data', async t => {
  const preparedDir = mkdtempSync(join(tmpdir(), 'source-outline-'))
  t.after(() => rmSync(preparedDir, { recursive: true, force: true }))
  const yearRoot = join(preparedDir, '2026')
  mkdirSync(yearRoot)
  markComplete(yearRoot, '2026')
  const file = join(yearRoot, 'source-outlines.sqlite')
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

test('outlines refuse a year directory whose completion manifest names another year or is missing', async t => {
  const preparedDir = mkdtempSync(join(tmpdir(), 'source-outline-bad-'))
  t.after(() => rmSync(preparedDir, { recursive: true, force: true }))
  const yearRoot = join(preparedDir, '2024')
  mkdirSync(yearRoot)
  const database = new DatabaseSync(join(yearRoot, 'source-outlines.sqlite'))
  database.exec('CREATE TABLE outlines (id TEXT NOT NULL, geometry TEXT NOT NULL, PRIMARY KEY (id, geometry)) WITHOUT ROWID')
  database.close()
  await assert.rejects(async () => {
    const app = Fastify()
    await app.register(sourceOutlineRoutes, { preparedDir, years: ['2024'] })
  })
  markComplete(yearRoot, '2026')
  await assert.rejects(async () => {
    const app = Fastify()
    await app.register(sourceOutlineRoutes, { preparedDir, years: ['2024'] })
  }, /2026.*2024/)

  // A database stamped for 2026 copied into an otherwise valid 2024 root is refused too.
  markComplete(yearRoot, '2024')
  const tagged = new DatabaseSync(join(yearRoot, 'source-outlines.sqlite'))
  tagged.exec("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID; INSERT INTO meta VALUES ('year', '2026');")
  tagged.close()
  await assert.rejects(async () => {
    const app = Fastify()
    await app.register(sourceOutlineRoutes, { preparedDir, years: ['2024'] })
  }, /source-outlines\.sqlite is for year 2026, not 2024/)
})
