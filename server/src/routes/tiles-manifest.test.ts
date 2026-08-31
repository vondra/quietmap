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
import {
  W2_SPATIAL_SCORER_CONTRACT,
  sha256Identity,
} from '../generation-contract.mjs'

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
const stockProducerRoles = {
  'cpu-airborne': 'stock',
  'cpu-building': 'stock',
  'cpu-cruise': 'stock',
  'cpu-ground': 'stock',
  'cpu-industrial': 'stock',
  'gpu-airborne': 'stock',
}
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
const modelRoleContract = (
  lineModelRole: 'w1' | 'w2-stride4',
  lineModelRoleSha256 = '1'.repeat(64),
) => ({
  schema: 1,
  line_model_role_sha256: lineModelRoleSha256,
  model_source_recipe_sha256: '2'.repeat(64),
  numerical_selection_record_sha256: null,
  output_abi_version: 3,
  role_spec_sha256: '3'.repeat(64),
  workers: {
    'cpu-airborne': worker(
      'aircraft-cpu-production', 'build-heatmap-aircraft', 'stock', 'aircraft-cpu-stock-v1',
    ),
    'cpu-building': worker(
      'surface-cpu-production', 'build-heatmap-surface', 'stock', 'surface-cpu-stock-v1',
    ),
    'cpu-cruise': worker(
      'aircraft-cpu-production', 'build-heatmap-aircraft', 'stock', 'aircraft-cpu-stock-v1',
    ),
    'cpu-ground': worker(
      'surface-cpu-production', 'build-heatmap-surface', 'stock', 'surface-cpu-stock-v1',
    ),
    'cpu-industrial': worker(
      'surface-cpu-production', 'build-heatmap-surface', 'stock', 'surface-cpu-stock-v1',
    ),
    'gpu-airborne': worker(
      'airborne-production', 'gpu-airborne', 'stock', 'airborne-stock-v1',
    ),
    'gpu-line': worker(
      'surface-production',
      'gpu-surface',
      lineModelRole,
      lineModelRole === 'w1'
        ? 'surface-w1-z12-accepted-v1'
        : 'surface-w2-z13-stride4-v1',
    ),
  },
})
function baseGeneration(lineModelRoleSha256 = '1'.repeat(64)) {
  const quality = {
    schema: 1,
    profile_name: 'w1-z12-accepted-v1',
    product_commit: 'a'.repeat(40),
    dataset_year: 2026,
    model_role_contract: modelRoleContract('w1', lineModelRoleSha256),
    numerical_environment: {
        QM_W1_INDUSTRIAL_POLICY: 'adaptive-stride5',
        QM_W1_BUILDING_POLICY: 'adaptive-stride5',
      },
    producer_requirements: {
      worker_model_roles: { ...stockProducerRoles, 'gpu-line': 'w1' },
    },
    scorer_contract: {
      bias_db_max: 0.5,
      presence_mismatch_percent_max: 6,
      quiet_floor_db: 26,
      threshold_percent_max: { 1: 30, 2: 15, 6: 1.5 },
    },
    wave: 'w1',
  }
  const qualityProfileId = sha256Identity(quality)
  const identity = {
    schema: 1,
    deployment: 'base',
    zoom: 12,
    tier: '',
    dataset_year: 2026,
    raster_generation_id: 'b'.repeat(16),
    quality_profile_id: qualityProfileId,
    quality_profile_name: quality.profile_name,
    base_generation_id: null,
    base_quality_profile_id: null,
    base_quality_profile_name: null,
  }
  const generationId = sha256Identity(identity)
  return {
    ...identity,
    generation_id: generationId,
    base_generation_id: generationId,
    base_quality_profile_id: qualityProfileId,
    base_quality_profile_name: quality.profile_name,
    quality,
  }
}
function validManifest(build = 'b3') {
  const layers: Record<string, { file: string; build: string; bytes: number; sha256: string }> = {}
  for (const layer of ALLOWED_LAYERS) {
    const file = `${layer}.${build}.pmtiles`
    const content = `archive-${layer}-${build}`
    writeFileSync(join(dir, file), content)
    layers[layer] = {
      file,
      build,
      bytes: Buffer.byteLength(content),
      sha256: createHash('sha256').update(content).digest('hex'),
    }
  }
  const generation = baseGeneration()
  return {
    build,
    generation,
    line_model_role_sha256: generation.quality.model_role_contract.line_model_role_sha256,
    layers,
  }
}

function tierGeneration(base: ReturnType<typeof baseGeneration>) {
  const quality = {
    schema: 1,
    profile_name: 'w2-z13-spatial-v1',
    product_commit: 'b'.repeat(40),
    dataset_year: 2026,
    model_role_contract: modelRoleContract('w2-stride4'),
    numerical_environment: {},
    producer_requirements: {
      worker_model_roles: { ...stockProducerRoles, 'gpu-line': 'w2-stride4' },
    },
    scorer_contract: structuredClone(W2_SPATIAL_SCORER_CONTRACT),
    wave: 'w2',
  }
  const qualityProfileId = sha256Identity(quality)
  const identity = {
    schema: 1,
    deployment: 'z13',
    zoom: 13,
    tier: 'z13',
    dataset_year: base.dataset_year,
    raster_generation_id: base.raster_generation_id,
    quality_profile_id: qualityProfileId,
    quality_profile_name: quality.profile_name,
    base_generation_id: base.generation_id,
    base_quality_profile_id: base.quality_profile_id,
    base_quality_profile_name: base.quality_profile_name,
  }
  return { ...identity, generation_id: sha256Identity(identity), quality }
}

test('serves this environment pin, with tile_base attached', async () => {
  clearFixture()
  writeFileSync(pinPath, JSON.stringify(validManifest('b3')))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 200)
  assert.equal(res.json().build, 'b3')
  assert.equal(res.json().layers.total.file, 'total.b3.pmtiles')
  assert.equal(res.json().tile_base, null)
  assert.equal(res.json().generation, undefined)
  assert.equal(res.json().line_model_role_sha256, undefined)
  assert.equal(res.headers['cache-control'], 'no-cache')
  await app.close()
})

test('projects tier metadata without exposing generation or publisher attestations', async () => {
  clearFixture()
  const manifest = validManifest('b7')
  const tierTokens = [...ALLOWED_LAYERS].map(layer => `${layer}-z13-p001`)
  for (const token of tierTokens) {
    const content = `archive-${token}-b7`
    writeFileSync(join(dir, `${token}.b7.pmtiles`), content)
    manifest.layers[token] = {
      file: `${token}.b7.pmtiles`,
      build: 'b7',
      bytes: Buffer.byteLength(content),
      sha256: createHash('sha256').update(content).digest('hex'),
      publisher_proof: { secret: 'internal' },
    } as typeof manifest.layers[string]
  }
  const qualificationBytes = Buffer.from('{"schema":"test-qualified-tier"}')
  const qualificationSha256 = createHash('sha256').update(qualificationBytes).digest('hex')
  const qualificationFile = `qualification-${qualificationSha256}.json`
  writeFileSync(join(dir, qualificationFile), qualificationBytes)
  chmodSync(join(dir, qualificationFile), 0o444)
  const tiered = { ...manifest, qualification_closure: {
    file: qualificationFile,
    sha256: qualificationSha256,
  }, tiers: {
    z13: {
      packs: [{
        pack: 'p001',
        coverage_r4: ['841e355ffffffff'],
        layers: tierTokens,
        generation: tierGeneration(manifest.generation),
      }],
    },
  } }
  writeFileSync(pinPath, JSON.stringify(tiered))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 200)
  assert.deepEqual(res.json().layers[tierTokens[0]], {
    file: `${tierTokens[0]}.b7.pmtiles`,
    build: 'b7',
  })
  assert.deepEqual(res.json().tiers.z13.packs, [{
    pack: 'p001',
    coverage_r4: ['841e355ffffffff'],
    layers: tierTokens,
  }])
  assert.equal(res.json().qualification_closure, undefined)
  assert.doesNotMatch(res.body, /quality|scorer|model_role|publisher_proof|secret/)
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
