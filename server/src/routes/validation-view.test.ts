import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import Fastify from 'fastify'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { validationViewRoutes } from './validation-view.js'

test('QA route joins approved truth and optional model artifacts by ID', async (t) => {
  const root = await mkdtemp(join(tmpdir(), '0db-validation-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const cohort = {
    schema_version: 1 as const,
    cohort_id: 'c'.repeat(64),
    cache_ttl_ms: 30_000,
    data_year: '2026',
    runtime_sha256: 'a'.repeat(64),
    prepared_sha256: 'b'.repeat(64),
  }
  const writeJson = async (relative: string, value: unknown) => {
    const path = join(root, relative)
    await mkdir(join(path, '..'), { recursive: true })
    const bytes = JSON.stringify(value)
    await writeFile(path, bytes)
    return createHash('sha256').update(bytes).digest('hex')
  }
  const fixturesSha256 = await writeJson('benchmarks/world-points.json', [{
    id: 'fixture', lat: 50, lng: 14, mode: 'total', regime: 'road', external: {},
    commensurability: {}, role: 'regression', tags: [], tolerance_note: 'fixture',
  }])
  await writeJson('benchmarks/validation/approved-snapshots.v1.json', { version: 1, files: ['network.2025.json'] })
  const snapshotSha256 = await writeJson('benchmarks/validation/snapshots/network.2025.json', {
    schema_version: 2, network: 'network', country_code: 'CZ', year: 2025,
    measured_metric_field: 'lden', model_metric_field: 'lden', comparison_mode: 'upper_bound',
    comparison_tolerance_db: 2, comparison_tolerance_basis: 'fixture', commensurability: {},
    stations: [{ station_id: 'station', name: 'Station', lat: 50.1, lng: 14.1, lden: 61 }],
  })
  await writeJson('data/validation/world-lastrun.json', {
    schema_version: 2,
    server: 'http://model', runner_commit: 'abc123', timestamp: '2026-07-13T00:00:00.000Z',
    requested_data_year: 2026, fixtures_sha256: fixturesSha256, model_cohort: cohort.cohort_id,
    results: [{ id: 'fixture', value: 55, status: 'OK', drift: 0, ext: null }],
  })
  await writeJson('data/validation/deltas/network.2025.json', {
    schema_version: 2,
    network: 'network', year: 2025, server: 'http://model', generated_at: '2026-07-13T00:01:00.000Z',
    snapshot_sha256: snapshotSha256, model_cohort: cohort.cohort_id,
    rows: [{
      station_id: 'station', model: { lden: 64, ld: null, le: null, ln: null },
      query_status: 'ok', dominant_source: 'road',
    }],
  })

  const app = Fastify()
  t.after(() => app.close())
  await app.register(validationViewRoutes, { repoRoot: root, cohortProvider: async () => cohort })
  const cohortResponse = await app.inject('/api/validation/cohort')
  assert.equal(cohortResponse.statusCode, 200)
  assert.deepEqual(cohortResponse.json(), cohort)
  const response = await app.inject('/api/validation/points')
  assert.equal(response.statusCode, 200)
  const body = response.json()
  assert.deepEqual(body.lastrun, {
    generated_at: '2026-07-13T00:00:00.000Z', server: 'http://model',
    model_cohort: cohort.cohort_id,
    runner_commit: 'abc123', runner_dirty: null,
    requested_data_year: 2026,
  })
  assert.equal(body.fixtures[0].model_value, 55)
  assert.equal(body.networks[0].stations[0].measured_value, 61)
  assert.equal(body.networks[0].stations[0].model_value, 64)
  assert.equal(body.networks[0].stations[0].delta_db, 3)
  assert.equal(body.networks[0].stations[0].verdict, 'above')
  assert.deepEqual(body.warnings, [])

  await writeJson('data/validation/world-lastrun.json', {
    schema_version: 2, server: 'http://invalid', timestamp: '2026-07-13T00:02:00.000Z',
    fixtures_sha256: fixturesSha256, model_cohort: cohort.cohort_id,
    results: [{ id: 'fixture', value: 55, status: 'SURPRISE', drift: 0, ext: { delta: 'boom', side: 'above' } }],
  })
  await writeJson('data/validation/deltas/network.2025.json', {
    schema_version: 2, network: 'network', year: 2025, server: 'http://invalid',
    generated_at: '2026-07-13T00:02:00.000Z', snapshot_sha256: snapshotSha256,
    model_cohort: cohort.cohort_id,
    rows: [{ station_id: 'station', model: {}, query_status: 'ok' }],
  })
  const invalid = (await app.inject('/api/validation/points')).json()
  assert.equal(invalid.lastrun, null)
  assert.equal(invalid.fixtures[0].model_value, null)
  assert.equal(invalid.networks[0].delta_meta, null)
  assert.equal(invalid.networks[0].stations[0].model_value, null)
  assert.ok(invalid.warnings.some((warning: string) => warning.includes('world model run/fixture has invalid data')))
  assert.ok(invalid.warnings.some((warning: string) => warning.includes('network delta/station has invalid data')))

  await writeJson('data/validation/world-lastrun.json', {
    schema_version: 2,
    server: 'http://stale', timestamp: '2026-07-13T00:02:00.000Z', fixtures_sha256: '0'.repeat(64),
    results: [{ id: 'fixture', value: 99 }],
  })
  await writeJson('data/validation/deltas/network.2025.json', {
    schema_version: 2,
    network: 'network', year: 2025, server: 'http://partial', generated_at: '2026-07-13T00:03:00.000Z',
    snapshot_sha256: snapshotSha256, model_cohort: cohort.cohort_id, rows: [],
  })
  const degraded = (await app.inject('/api/validation/points')).json()
  assert.equal(degraded.lastrun, null)
  assert.equal(degraded.fixtures[0].model_value, null)
  assert.equal(degraded.networks[0].delta_meta, null)
  assert.equal(degraded.networks[0].stations[0].model_value, null)
  assert.ok(degraded.warnings.some((warning: string) => warning.includes('different fixture content')))
  assert.ok(degraded.warnings.some((warning: string) => warning.includes('covers 0/1 stations')))

  await writeJson('data/validation/world-lastrun.json', {
    schema_version: 2, server: 'http://old-model', timestamp: '2026-07-13T00:04:00.000Z',
    fixtures_sha256: fixturesSha256, model_cohort: 'd'.repeat(64),
    results: [{ id: 'fixture', value: 55, status: 'OK', drift: 0, ext: null }],
  })
  const wrongCohort = (await app.inject('/api/validation/points')).json()
  assert.equal(wrongCohort.lastrun, null)
  assert.equal(wrongCohort.fixtures[0].model_value, null)
  assert.ok(wrongCohort.warnings.some((warning: string) => warning.includes('different model/data cohort')))

  await writeJson('data/validation/world-lastrun.json', {
    schema_version: 2, server: 'http://current-model', timestamp: '2026-07-13T00:05:00.000Z',
    fixtures_sha256: fixturesSha256, model_cohort: cohort.cohort_id,
    results: [{ id: 'fixture', value: 55, status: 'OK', drift: 0, ext: null }],
  })
  await writeJson('data/validation/deltas/network.2025.json', {
    schema_version: 2, network: 'network', year: 2025, server: 'http://old-model',
    generated_at: '2026-07-13T00:05:00.000Z', snapshot_sha256: snapshotSha256,
    model_cohort: 'd'.repeat(64),
    rows: [{
      station_id: 'station', model: { lden: 64, ld: null, le: null, ln: null },
      query_status: 'ok', dominant_source: 'road',
    }],
  })
  const wrongDeltaCohort = (await app.inject('/api/validation/points')).json()
  assert.equal(wrongDeltaCohort.fixtures[0].model_value, 55)
  assert.equal(wrongDeltaCohort.networks[0].delta_meta, null)
  assert.equal(wrongDeltaCohort.networks[0].stations[0].model_value, null)
  assert.ok(wrongDeltaCohort.warnings.some((warning: string) =>
    warning.includes('network/2025 model delta belongs to a different model/data cohort')))

  const redirect = await app.inject('/validation')
  assert.equal(redirect.statusCode, 302)
  assert.equal(redirect.headers.location, '/#val=1')
})

test('QA route fails closed when the live model cohort requires restart', async (t) => {
  const root = await mkdtemp(join(tmpdir(), '0db-validation-stale-process-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const app = Fastify()
  t.after(() => app.close())
  await app.register(validationViewRoutes, {
    repoRoot: root,
    cohortProvider: async () => { throw new Error('restart required') },
  })

  const cohort = await app.inject('/api/validation/cohort')
  assert.equal(cohort.statusCode, 503)
  assert.equal(cohort.headers['cache-control'], 'no-store')
  assert.deepEqual(cohort.json(), { error: 'validation cohort unavailable' })

  const points = await app.inject('/api/validation/points')
  assert.equal(points.statusCode, 200, 'committed catalogs remain independently inspectable')
  assert.equal(points.json().model_cohort, null)
  assert.ok(points.json().warnings.some((warning: string) =>
    warning.includes('model cohort unavailable — model results hidden; restart required')))
})
