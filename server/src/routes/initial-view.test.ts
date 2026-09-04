// Contract test for GET /api/initial-view — the country-level first-view guess.
// The GeoIP database is OPTIONAL runtime data (not in git), so these tests
// cover the data-free contract: absent or corrupt database must mean a clean
// {source:'none'} — never a 500 — and the response must never be cacheable
// by a shared cache (the answer depends on the caller's IP).
// Run: cd server && npx tsx --test src/routes/initial-view.test.ts

import assert from 'node:assert/strict'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import Fastify from 'fastify'
import type { CityResponse } from 'mmdb-lib'
import { countryFromRecord, initialViewRoutes } from './initial-view.js'

const dir = mkdtempSync(join(tmpdir(), 'initial-view-route-test-'))

// Registered exactly as production does (app.register + options object) —
// a direct initialViewRoutes(app, path) call once masked a register-passes-
// opts-object bug that only production hit.
async function buildApp(databasePath: string) {
  const app = Fastify()
  await app.register(initialViewRoutes, { databasePath })
  return app
}

test('missing database is a clean source:none, never a 500', async () => {
  const app = await buildApp(join(dir, 'does-not-exist.mmdb'))
  const res = await app.inject('/api/initial-view')
  assert.equal(res.statusCode, 200)
  assert.deepEqual(res.json(), { source: 'none' })
  assert.equal(res.headers['cache-control'], 'private, no-store')
})

test('corrupt database is a clean source:none, never a 500', async () => {
  const corrupt = join(dir, 'corrupt.mmdb')
  writeFileSync(corrupt, 'this is not an mmdb file')
  const app = await buildApp(corrupt)
  const res = await app.inject('/api/initial-view')
  assert.equal(res.statusCode, 200)
  assert.deepEqual(res.json(), { source: 'none' })
})

test('repeated requests reuse the failed-load decision (no retry storm)', async () => {
  const app = await buildApp(join(dir, 'also-missing.mmdb'))
  const first = await app.inject('/api/initial-view')
  const second = await app.inject('/api/initial-view')
  assert.equal(first.statusCode, 200)
  assert.deepEqual(second.json(), { source: 'none' })
})

// The record → response mapping is the wire contract; it is pure and testable
// without an mmdb fixture (the DB itself is optional runtime data, not in git).
const asRecord = (r: unknown) => r as CityResponse | null

test('countryFromRecord returns ip-country for a valid ISO alpha-2', () => {
  assert.deepEqual(countryFromRecord(asRecord({ country: { iso_code: 'CZ' } })), {
    source: 'ip-country',
    country: 'CZ',
  })
})

test('countryFromRecord is source:none when the record has no country', () => {
  assert.deepEqual(countryFromRecord(null), { source: 'none' })
  assert.deepEqual(countryFromRecord(asRecord({})), { source: 'none' })
  assert.deepEqual(countryFromRecord(asRecord({ country: {} })), { source: 'none' })
})

test('countryFromRecord rejects codes that are not ISO alpha-2 shape', () => {
  // Anonymous-proxy markers (A1/A2), lowercase, or junk must not reach the
  // client as ip-country — the client applies the identical /^[A-Z]{2}$/ guard.
  for (const iso_code of ['cz', 'CZE', 'C', '7!', 'A1', '']) {
    assert.deepEqual(countryFromRecord(asRecord({ country: { iso_code } })), { source: 'none' })
  }
})
