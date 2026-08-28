//! Focused fail-closed tests for the accepted W1 and W2 spatial generation profiles.

import assert from 'node:assert/strict'
import test from 'node:test'

import {
  W2_SPATIAL_POPULATION_SCOPES,
  sha256Identity,
  validateGenerationContract,
  validatePublishedGenerationContract,
  validateScorerContract,
} from '../src/generation-contract.mjs'

const W1_PROFILE = 'w1-z12-accepted-v1'
const W2_PROFILE = 'w2-z13-spatial-v1'
const W2_IMPLEMENTATION_SHA256 =
  '4864c9f2925a2146a72e08f026deca75b3f099150d789c268e28ad2693ff638d'
const W2_WARM_REFERENCE_FINGERPRINT =
  'c92bc8ac4159c2759645cbf5948077ce024d55d633373a6b2aed5c1a7b547dc9'

const genericScorer = {
  bias_db_max: 0.5,
  presence_mismatch_percent_max: 0.25,
  quiet_floor_db: 10,
  threshold_percent_max: { 0.5: 20, 1: 1, 3: 0.01, 6: 0.001 },
  unified_threshold_db: 6,
}
const w1Scorer = {
  bias_db_max: 0.5,
  presence_mismatch_percent_max: 6,
  quiet_floor_db: 26,
  threshold_percent_max: { 1: 30, 2: 15, 6: 1.5 },
}
const spatialScorer = {
  schema: 'w2-z13-spatial-scorer-v2',
  implementation_sha256: W2_IMPLEMENTATION_SHA256,
  population_scopes: structuredClone(W2_SPATIAL_POPULATION_SCOPES),
  spatial_tolerance_pixels: 1,
  spatial_match_policy: 'symmetric-chebyshev-r1-directional-min-plus-histogram-capacity-v1',
  threshold_percent_max: { 0.5: 2, 1: 1, 3: 0.25, 6: 0.05 },
  quiet_threshold_percent_max: { 10: 0.01, 15: 0.001 },
  presence_multiplicity_percent_max: 0.25,
  bias_db_max: 0.5,
  warm_reference_fingerprint: W2_WARM_REFERENCE_FINGERPRINT,
}

const stockProducerRoles = {
  'cpu-airborne': 'stock',
  'cpu-building': 'stock',
  'cpu-cruise': 'stock',
  'cpu-ground': 'stock',
  'cpu-industrial': 'stock',
  'gpu-airborne': 'stock',
}

function worker(artifactFamily, binary, modelRole, resolvedRole) {
  return {
    artifact_family: artifactFamily,
    binary,
    model_role: modelRole,
    resolved_role: resolvedRole,
    selection_epoch: null,
  }
}

function modelRoleContract(lineModelRole) {
  const lineResolvedRole = lineModelRole === 'w1'
    ? 'surface-w1-z12-accepted-v1'
    : 'surface-w2-z13-stride4-v1'
  return {
    schema: 1,
    line_model_role_sha256: '1'.repeat(64),
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
        'surface-production', 'gpu-surface', lineModelRole, lineResolvedRole,
      ),
    },
  }
}

function generationIdentity(contract) {
  const tiered = contract.tier !== ''
  return {
    schema: 1,
    deployment: contract.deployment,
    zoom: contract.zoom,
    tier: contract.tier,
    dataset_year: contract.dataset_year,
    raster_generation_id: contract.raster_generation_id,
    quality_profile_id: contract.quality_profile_id,
    quality_profile_name: contract.quality_profile_name,
    base_generation_id: tiered ? contract.base_generation_id : null,
    base_quality_profile_id: tiered ? contract.base_quality_profile_id : null,
    base_quality_profile_name: tiered ? contract.base_quality_profile_name : null,
  }
}

function resealGeneration(contract) {
  contract.quality_profile_id = sha256Identity(contract.quality)
  if (contract.tier === '') {
    contract.base_quality_profile_id = contract.quality_profile_id
    contract.base_quality_profile_name = contract.quality_profile_name
  }
  contract.generation_id = sha256Identity(generationIdentity(contract))
  if (contract.tier === '') contract.base_generation_id = contract.generation_id
  return contract
}

function acceptedW1Generation() {
  const quality = {
    schema: 1,
    profile_name: W1_PROFILE,
    product_commit: 'a'.repeat(40),
    dataset_year: 2026,
    model_role_contract: modelRoleContract('w1'),
    numerical_environment: { QM_W1_INDUSTRIAL_POLICY: 'adaptive-stride5' },
    producer_requirements: {
      worker_model_roles: { ...stockProducerRoles, 'gpu-line': 'w1' },
    },
    scorer_contract: structuredClone(w1Scorer),
    wave: 'w1',
  }
  return resealGeneration({
    schema: 1,
    deployment: 'base',
    zoom: 12,
    tier: '',
    dataset_year: 2026,
    raster_generation_id: '4'.repeat(16),
    quality_profile_id: '',
    quality_profile_name: W1_PROFILE,
    base_generation_id: '',
    base_quality_profile_id: '',
    base_quality_profile_name: W1_PROFILE,
    generation_id: '',
    quality,
  })
}

function spatialGeneration() {
  const base = acceptedW1Generation()
  const quality = {
    schema: 1,
    profile_name: W2_PROFILE,
    product_commit: 'b'.repeat(40),
    dataset_year: 2026,
    model_role_contract: modelRoleContract('w2-stride4'),
    numerical_environment: {},
    producer_requirements: {
      worker_model_roles: { ...stockProducerRoles, 'gpu-line': 'w2-stride4' },
    },
    scorer_contract: structuredClone(spatialScorer),
    wave: 'w2',
  }
  return resealGeneration({
    schema: 1,
    deployment: 'z13',
    zoom: 13,
    tier: 'z13',
    dataset_year: 2026,
    raster_generation_id: base.raster_generation_id,
    quality_profile_id: '',
    quality_profile_name: W2_PROFILE,
    base_generation_id: base.generation_id,
    base_quality_profile_id: base.quality_profile_id,
    base_quality_profile_name: base.quality_profile_name,
    generation_id: '',
    quality,
  })
}

test('generic scorer payload remains valid and unchanged', () => {
  assert.deepEqual(validateScorerContract(structuredClone(genericScorer)), genericScorer)
})

test('spatial scorer pins the exact implementation, reference, policy, and bounds', () => {
  assert.deepEqual(
    validateScorerContract(structuredClone(spatialScorer), 'test', W2_PROFILE),
    spatialScorer,
  )
  for (const [label, mutate] of [
    ['implementation', scorer => { scorer.implementation_sha256 = '7'.repeat(64) }],
    ['warm reference', scorer => { scorer.warm_reference_fingerprint = '8'.repeat(64) }],
    ['spatial radius', scorer => { scorer.spatial_tolerance_pixels = 2 }],
    ['population', scorer => { delete scorer.population_scopes['wbench-s'] }],
    ['threshold', scorer => { scorer.threshold_percent_max[3] = 0.5 }],
  ]) {
    const wrong = structuredClone(spatialScorer)
    mutate(wrong)
    assert.throws(
      () => validateScorerContract(wrong, 'test', W2_PROFILE),
      undefined,
      `accepted wrong ${label}`,
    )
  }
  assert.throws(() => validateScorerContract(structuredClone(spatialScorer), 'test', 'test-v1'))
})

test('canonical accepted W1 base and W2 spatial tier both validate', () => {
  assert.equal(validateGenerationContract(acceptedW1Generation()).deployment, 'base')
  assert.equal(validateGenerationContract(spatialGeneration()).tier, 'z13')
  assert.equal(validatePublishedGenerationContract(acceptedW1Generation()).deployment, 'base')
  assert.equal(validatePublishedGenerationContract(spatialGeneration()).tier, 'z13')
})

test('published boundary rejects a self-consistent unknown quality profile', () => {
  const contract = spatialGeneration()
  contract.quality_profile_name = 'w2-z13-spatial-v2'
  contract.quality.profile_name = contract.quality_profile_name
  contract.quality.scorer_contract = structuredClone(genericScorer)
  resealGeneration(contract)
  assert.equal(validateGenerationContract(contract).quality_profile_name, 'w2-z13-spatial-v2')
  assert.throws(
    () => validatePublishedGenerationContract(contract),
    /published quality profile is unsupported/,
  )
})

test('W2 named profile rejects semantic drift after both identities are recomputed', () => {
  const cases = [
    ['wrong role', contract => {
      contract.quality.producer_requirements.worker_model_roles['gpu-line'] = 'stock'
      contract.quality.model_role_contract.workers['gpu-line'] = worker(
        'surface-production', 'gpu-surface', 'stock', 'surface-stock-v1',
      )
    }],
    ['missing worker', contract => {
      delete contract.quality.producer_requirements.worker_model_roles['cpu-ground']
      delete contract.quality.model_role_contract.workers['cpu-ground']
    }],
    ['wrong environment', contract => {
      contract.quality.numerical_environment.SURFACE_BUDGET_ETA = '0'
    }],
    ['wrong scorer implementation', contract => {
      contract.quality.scorer_contract.implementation_sha256 = '7'.repeat(64)
    }],
    ['wrong warm reference', contract => {
      contract.quality.scorer_contract.warm_reference_fingerprint = '8'.repeat(64)
    }],
    ['wrong base', contract => { contract.base_quality_profile_name = 'exact-z12-v1' }],
    ['wrong deployment', contract => { contract.deployment = 'refinement' }],
  ]
  for (const [label, mutate] of cases) {
    const contract = spatialGeneration()
    mutate(contract)
    resealGeneration(contract)
    assert.throws(
      () => validateGenerationContract(contract),
      undefined,
      `accepted recomputed W2 contract with ${label}`,
    )
  }
})

test('accepted W1 name cannot carry a self-consistent spoofed payload', () => {
  const cases = [
    ['wave', contract => { contract.quality.wave = 'exact' }],
    ['environment', contract => { contract.quality.numerical_environment = {} }],
    ['role', contract => {
      contract.quality.producer_requirements.worker_model_roles['gpu-line'] = 'stock'
      contract.quality.model_role_contract.workers['gpu-line'] = worker(
        'surface-production', 'gpu-surface', 'stock', 'surface-stock-v1',
      )
    }],
    ['worker set', contract => {
      delete contract.quality.producer_requirements.worker_model_roles['cpu-building']
      delete contract.quality.model_role_contract.workers['cpu-building']
    }],
    ['scorer', contract => { contract.quality.scorer_contract.bias_db_max = 0.6 }],
    ['deployment', contract => { contract.deployment = 'other-base' }],
  ]
  for (const [label, mutate] of cases) {
    const contract = acceptedW1Generation()
    mutate(contract)
    resealGeneration(contract)
    assert.throws(
      () => validateGenerationContract(contract),
      undefined,
      `accepted recomputed W1 contract with wrong ${label}`,
    )
  }
})
