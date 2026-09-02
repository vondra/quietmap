// Contract test for GET /api/tiles-manifest: the route serves THIS environment's per-env pin
// (current.{TILE_ENV}.json), selected via
// the shared tile-manifest-reader.ts, instead of the packer's shared current.json merge head.
// Run: cd server && npx tsx --test src/routes/tiles-manifest.test.ts

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { chmodSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WORLD_BASE_ZOOM, sha256Identity } from '../generation-contract.mjs'

// PMTILES_BASE and TILE_ENV are captured from the env when heatmap-shared/tile-manifest-reader
// load — point them at the fixture dir BEFORE importing (mirrors heatmap-pmtiles.test.ts's
// pattern). TILE_ENV is fixed at 'dev1' for this whole file; every scenario below is driven by
// changing what's ON DISK in `dir`, not by re-reading env vars mid-file (module-level consts
// can't be re-evaluated once imported).
const dir = mkdtempSync(join(tmpdir(), 'tiles-manifest-route-test-'))
process.env.PMTILES_DIR = dir
process.env.TILE_ENV = 'dev1'

const { tilesManifestRoutes } = await import('./tiles-manifest.js')
const { ALLOWED_LAYERS } = await import('./heatmap-shared.js')
const { default: Fastify } = await import('fastify')

async function buildApp() {
  const app = Fastify()
  await app.register(tilesManifestRoutes)
  return app
}

const pinPath = join(dir, 'current.dev1.json')
const legacyPath = join(dir, 'current.json')
const clearFixture = () => {
  for (const name of readdirSync(dir)) rmSync(join(dir, name), { force: true, recursive: true })
}
const ACCEPTED_PROFILE = 'w2-z13-accepted-v1'
const worker = (
  artifactFamily: string,
  binary: string,
  modelRole: string,
  resolvedRole: string,
) => ({
  artifact_family: artifactFamily,
  binary,
  model_role: modelRole,
  resolved_role: resolvedRole,
  selection_epoch: null,
})
/** One painting run's identity; two of these in one manifest is an ordinary partial publish. */
function acceptedGeneration(rasterGenerationId = 'b'.repeat(16)) {
  const quality = {
    schema: 1,
    profile_name: ACCEPTED_PROFILE,
    product_commit: 'a'.repeat(40),
    dataset_year: 2026,
    model_role_contract: {
      schema: 1,
      line_model_role_sha256: '1'.repeat(64),
      model_source_recipe_sha256: '2'.repeat(64),
      numerical_selection_record_sha256: null,
      output_abi_version: 3,
      role_spec_sha256: '3'.repeat(64),
      workers: {
        'cpu-cruise': worker(
          'aircraft-cpu-production', 'build-heatmap-aircraft', 'stock', 'aircraft-cpu-stock-v1',
        ),
        'gpu-airborne': worker(
          'airborne-production', 'gpu-airborne', 'stock', 'airborne-stock-v1',
        ),
        'gpu-surface': worker(
          'surface-production', 'gpu-surface', 'w2-merged', 'surface-w2-z13-accepted-v1',
        ),
      },
    },
    numerical_environment: {},
    producer_requirements: {
      worker_model_roles: {
        'cpu-cruise': 'stock',
        'gpu-airborne': 'stock',
        'gpu-surface': 'w2-merged',
      },
    },
    scorer_contract: {
      bias_db_max: 0.5,
      presence_mismatch_percent_max: 0.25,
      quiet_floor_db: 10,
      threshold_percent_max: { 0.5: 20, 1: 1, 3: 0.01, 6: 0.001 },
      unified_threshold_db: 6,
    },
    wave: 'w2',
  }
  const qualityProfileId = sha256Identity(quality)
  const identity = {
    schema: 1,
    zoom: WORLD_BASE_ZOOM,
    dataset_year: 2026,
    raster_generation_id: rasterGenerationId,
    quality_profile_id: qualityProfileId,
    quality_profile_name: ACCEPTED_PROFILE,
  }
  return {
    ...identity,
    generation_id: sha256Identity(identity),
    quality,
  }
}
function validManifest(build = 'b3') {
  const layers: Record<string, {
    file: string
    build: string
    bytes: number
    sha256: string
    generation: unknown
  }> = {}
  const generation = acceptedGeneration()
  for (const layer of ALLOWED_LAYERS) {
    const file = `${layer}.${build}.pmtiles`
    const content = `archive-${layer}-${build}`
    writeFileSync(join(dir, file), content)
    layers[layer] = {
      file,
      build,
      bytes: Buffer.byteLength(content),
      sha256: createHash('sha256').update(content).digest('hex'),
      generation,
    }
  }
  return { build, layers }
}

test('serves this environment pin, with tile_base attached', async () => {
  clearFixture()
  writeFileSync(pinPath, JSON.stringify(validManifest('b3')))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 200)
  assert.equal(res.json().build, 'b3')
  assert.equal(res.json().zoom, WORLD_BASE_ZOOM)
  assert.equal(res.json().layers.total.file, 'total.b3.pmtiles')
  assert.equal(res.json().tile_base, null)
  assert.equal(res.json().generation, undefined)
  assert.equal(res.headers['cache-control'], 'no-cache')
  await app.close()
})

test('projects a mixed publication without exposing any generation payload', async () => {
  clearFixture()
  const manifest = validManifest('b7')
  // industrial was repainted in b7; road was carried forward from b5 with its own older
  // generation. Both are ordinary layer entries to the frontend.
  const carried = 'archive-road-b5'
  writeFileSync(join(dir, 'road.b5.pmtiles'), carried)
  manifest.layers.road = {
    file: 'road.b5.pmtiles',
    build: 'b5',
    bytes: Buffer.byteLength(carried),
    sha256: createHash('sha256').update(carried).digest('hex'),
    generation: acceptedGeneration('c'.repeat(16)),
  }
  ;(manifest.layers.industrial as { publisher_proof?: unknown }).publisher_proof = {
    secret: 'internal',
  }
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 200)
  assert.equal(res.json().build, 'b7')
  assert.equal(res.json().zoom, WORLD_BASE_ZOOM)
  assert.deepEqual(res.json().layers.road, { file: 'road.b5.pmtiles', build: 'b5' })
  assert.deepEqual(res.json().layers.industrial,
    { file: 'industrial.b7.pmtiles', build: 'b7' })
  assert.doesNotMatch(res.body, /quality|scorer|model_role|publisher_proof|secret|generation/)
  await app.close()
})

test('a genuinely fresh checkout (no pin, no legacy manifest) is a 404, not a 500', async () => {
  clearFixture()
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 404)
  assert.deepEqual(res.json(), { error: 'no build published' })
  await app.close()
})

test('an un-seeded checkout (legacy current.json but no per-env pin) is a 500, never a silent fallback to the legacy build', async () => {
  clearFixture()
  writeFileSync(legacyPath, JSON.stringify({ build: 'b1', layers: { total: { file: 'total.b1.pmtiles' } } }))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 500)
  assert.deepEqual(res.json(), { error: 'manifest unreadable' })
  assert.doesNotMatch(res.body, /b1|total\.b1/, 'must never leak the legacy build it refused to serve')
  await app.close()
})

test('a torn/unparseable pin is a 500', async () => {
  clearFixture()
  writeFileSync(pinPath, '{ not json')
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 500)
  assert.deepEqual(res.json(), { error: 'manifest unreadable' })
  await app.close()
})

test('a parseable but readiness-invalid pin is a 500, never served to a goal', async () => {
  clearFixture()
  const manifest = validManifest('b4')
  delete manifest.layers.road
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 500)
  assert.deepEqual(res.json(), { error: 'manifest unreadable' })
  await app.close()
})

test('a present pin that references a missing archive is corruption (500), not no-build (404)', async () => {
  clearFixture()
  const manifest = validManifest('b5')
  rmSync(join(dir, manifest.layers.road.file))
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 500)
  assert.deepEqual(res.json(), { error: 'manifest unreadable' })
  await app.close()
})

test('the pin cache revalidates immutable archives after its short TTL', async () => {
  clearFixture()
  const manifest = validManifest('b6')
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const realNow = Date.now
  let now = realNow()
  Date.now = () => now
  try {
    assert.equal((await app.inject('/api/tiles-manifest')).statusCode, 200)
    rmSync(join(dir, manifest.layers.road.file))
    assert.equal((await app.inject('/api/tiles-manifest')).statusCode, 200)
    now += 10_001
    assert.equal((await app.inject('/api/tiles-manifest')).statusCode, 500)
  } finally {
    Date.now = realNow
    await app.close()
  }
})
