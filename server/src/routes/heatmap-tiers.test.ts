// Zoom-tier serving contract: the token grammar, the
// per-tier zoom bound, and the tiers-index referential validation — the
// server side is the contract most likely to rot (the Rust packer and the
// frontend resolver both carry their own tests; gg z13 impl review, Kimi #6).

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { sha256Identity } from '../generation-contract.mjs'
import { ALLOWED_LAYERS, parseTierToken, parseTileParams } from './heatmap-shared.js'
import { validateTiersIndex, type PmtilesManifest } from '../runtime-readiness.js'

test('parseTierToken accepts canonical tokens and rejects near-misses', () => {
  assert.deepEqual(parseTierToken('road-z13-p001'), { base: 'road', tier: 13, pack: 'p001' })
  assert.deepEqual(parseTierToken('aircraft-ground-z14-p2'), {
    base: 'aircraft-ground', tier: 14, pack: 'p2',
  })
  assert.equal(parseTierToken('road'), null, 'base names are not tokens')
  assert.equal(parseTierToken('road-z12-p1'), null, 'z12 is the base band')
  assert.equal(parseTierToken('road-z013-p1'), null, 'non-canonical zoom digits (Rust lockstep)')
  assert.equal(parseTierToken('bogus-z13-p1'), null, 'unknown base layer')
  assert.equal(parseTierToken('road-z13-q1'), null, 'pack id must be p<N>')
})

test('tile params: base band keeps 2..12, a tier token serves EXACTLY its zoom', () => {
  assert.deepEqual(parseTileParams({ layer: 'road', z: '12', x: '2212', y: '1387' }),
    { layer: 'road', z: 12, x: 2212, y: 1387 })
  assert.equal(typeof parseTileParams({ layer: 'road', z: '13', x: '0', y: '0' }), 'string',
    'base layer refuses z13')
  assert.deepEqual(parseTileParams({ layer: 'road-z13-p001', z: '13', x: '4424', y: '2774' }),
    { layer: 'road-z13-p001', z: 13, x: 4424, y: 2774 })
  assert.equal(typeof parseTileParams({ layer: 'road-z13-p001', z: '12', x: '0', y: '0' }), 'string',
    'tier token refuses the base band')
})

function generationPair(rasterGenerationId = 'b'.repeat(16)) {
  const modelRoleContract = {
    schema: 1,
    line_model_role_sha256: '1'.repeat(64),
    model_source_recipe_sha256: '2'.repeat(64),
    numerical_selection_record_sha256: null,
    output_abi_version: 3,
    role_spec_sha256: '3'.repeat(64),
    workers: {
      'gpu-line': {
        artifact_family: 'surface-production',
        binary: 'gpu-surface',
        model_role: 'stock',
        resolved_role: 'surface-stock-v1',
        selection_epoch: null,
      },
    },
  }
  const makeQuality = (profileName: string, wave: string) => ({
    schema: 1,
    profile_name: profileName,
    product_commit: 'a'.repeat(40),
    dataset_year: 2026,
    model_role_contract: modelRoleContract,
    numerical_environment: {},
    producer_requirements: { worker_model_roles: { 'gpu-line': 'stock' } },
    scorer_contract: {
      bias_db_max: 0.5,
      presence_mismatch_percent_max: 0.25,
      threshold_percent_max: { 0.5: 20, 1: 1, 3: 0.01, 6: 0.001 },
    },
    wave,
  })
  const quality = makeQuality('test-base-v1', 'w1')
  const qualityProfileId = sha256Identity(quality)
  const baseIdentity = {
    schema: 1,
    deployment: 'base',
    zoom: 12,
    tier: '',
    dataset_year: 2026,
    raster_generation_id: rasterGenerationId,
    quality_profile_id: qualityProfileId,
    quality_profile_name: quality.profile_name,
    base_generation_id: null,
    base_quality_profile_id: null,
    base_quality_profile_name: null,
  }
  const baseGenerationId = sha256Identity(baseIdentity)
  const base = {
    ...baseIdentity,
    generation_id: baseGenerationId,
    base_generation_id: baseGenerationId,
    base_quality_profile_id: qualityProfileId,
    base_quality_profile_name: quality.profile_name,
    quality,
  }
  const tierQuality = makeQuality('test-tier-v1', 'w2')
  const tierQualityProfileId = sha256Identity(tierQuality)
  const tierIdentity = {
    schema: 1,
    deployment: 'z13',
    zoom: 13,
    tier: 'z13',
    dataset_year: 2026,
    raster_generation_id: rasterGenerationId,
    quality_profile_id: tierQualityProfileId,
    quality_profile_name: tierQuality.profile_name,
    base_generation_id: baseGenerationId,
    base_quality_profile_id: qualityProfileId,
    base_quality_profile_name: quality.profile_name,
  }
  const tier = {
    ...tierIdentity,
    generation_id: sha256Identity(tierIdentity),
    quality: tierQuality,
  }
  return { base, tier }
}

function tieredManifest(overrides: Record<string, unknown> = {}): PmtilesManifest {
  const generation = generationPair()
  const tierTokens = [...ALLOWED_LAYERS].map(layer => `${layer}-z13-p001`)
  return {
    build: 'b17',
    generation: generation.base,
    line_model_role_sha256:
      generation.base.quality.model_role_contract.line_model_role_sha256,
    layers: Object.fromEntries(tierTokens.map(token =>
      [token, { file: `${token}.b17.pmtiles` }])),
    tiers: {
      z13: {
        packs: [{
          pack: 'p001',
          generation: generation.tier,
          coverage_r4: ['841e355ffffffff'],
          layers: tierTokens,
        }],
      },
    },
    ...overrides,
  } as PmtilesManifest
}

function legacyTieredManifest(): PmtilesManifest {
  const manifest = structuredClone(tieredManifest())
  delete manifest.generation
  delete manifest.line_model_role_sha256
  delete tierPack(manifest).generation
  return manifest
}

test('legacy tier heads remain serve-only during the generation migration rollout', () => {
  assert.doesNotThrow(() => validateTiersIndex(legacyTieredManifest(), 'legacy-current.json'))
  const popupEra = legacyTieredManifest()
  popupEra.line_model_role_sha256 = '1'.repeat(64)
  assert.doesNotThrow(() => validateTiersIndex(popupEra, 'legacy-popup-current.json'),
    'pre-generation popup manifests already carried a top-level line identity')
  const crossed = legacyTieredManifest()
  tierPack(crossed).generation = generationPair().tier
  assert.throws(() => validateTiersIndex(crossed, 'legacy-current.json'),
    /legacy tier pack .* carries a generation identity/)
  const partial = legacyTieredManifest()
  partial.generation = generationPair().base
  assert.throws(() => validateTiersIndex(partial, 'legacy-current.json'),
    /mixes legacy and generation-fenced manifest fields/)
})

test('validateTiersIndex: a well-formed index passes; absence passes', () => {
  validateTiersIndex(tieredManifest(), 'current.test.json')
  validateTiersIndex({ build: 'b1', layers: {} } as PmtilesManifest, 'current.test.json')
  assert.throws(
    () => validateTiersIndex({
      build: 'b1',
      layers: { 'road-z13-p001': { file: 'road-z13-p001.b1.pmtiles' } },
    } as PmtilesManifest, 'current.test.json'),
    /tier token road-z13-p001 is absent from the tiers index/,
  )
})

test('validateTiersIndex fails closed on every torn shape', () => {
  const bad = (mutate: (manifest: PmtilesManifest) => void, expected: RegExp) => {
    const manifest = tieredManifest()
    mutate(manifest)
    assert.throws(() => validateTiersIndex(manifest, 'current.test.json'), expected)
  }
  bad((m) => { (m.tiers as Record<string, unknown>).z12 = { packs: [] } }, /invalid zoom key z12/)
  bad((m) => { tierPack(m).pack = 'P001' }, /non-canonical pack id/)
  bad((m) => { delete tierPack(m).generation }, /has an invalid generation/)
  bad((m) => {
    const generation = tierPack(m).generation as { quality: { product_commit: string } }
    generation.quality.product_commit = 'f'.repeat(40)
  }, /quality_profile_id does not match its payload/)
  bad((m) => {
    tierPack(m).generation = generationPair('c'.repeat(16)).tier
  }, /tier is not anchored to the live base generation/)
  bad((m) => { tierPack(m).coverage_r4 = [] }, /invalid coverage_r4/)
  bad((m) => { tierPack(m).layers.pop() }, /exact 8-layer bundle/)
  bad((m) => { tierPack(m).coverage_r4 = ['851e355ffffffff'] }, /invalid coverage_r4/)
  bad((m) => { tierPack(m).layers = ['road-z13-p002'] }, /lists foreign token road-z13-p002/)
  bad((m) => { tierPack(m).layers = ['road-z14-p001'] }, /lists foreign token road-z14-p001/)
  bad((m) => { delete (m.layers as Record<string, unknown>)['road-z13-p001'] },
    /token road-z13-p001 has no layers entry/)
  bad((m) => {
    (m.layers as Record<string, unknown>)['road-z13-p002'] =
      { file: 'road-z13-p002.b17.pmtiles' }
  }, /tier token road-z13-p002 is absent from the tiers index/)
  bad((m) => {
    (m.tiers as Record<string, { packs: unknown[] }>).z13.packs.push({
      pack: 'p001', coverage_r4: ['841e309ffffffff'], layers: ['road-z13-p001'],
    })
  }, /pack p001 is duplicated/)
})

function tierPack(manifest: PmtilesManifest): {
  pack: string
  generation?: unknown
  coverage_r4: string[]
  layers: string[]
} {
  return (manifest.tiers as Record<string, { packs: Array<{
    pack: string; generation?: unknown; coverage_r4: string[]; layers: string[]
  }> }>).z13.packs[0]
}
