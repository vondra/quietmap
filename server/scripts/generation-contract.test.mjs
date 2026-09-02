//! Fail-closed tests for the published z13 world generation and its accepted quality profile.

import assert from 'node:assert/strict'
import test from 'node:test'

import {
  WORLD_BASE_ZOOM,
  sha256Identity,
  validateGenerationContract,
  validatePublishedGenerationContract,
} from '../src/generation-contract.mjs'

const ACCEPTED_PROFILE = 'w2-z13-accepted-v1'

// The Wave-2 ladder of engine/tile-painter/src/accuracy_contract.rs, exactly as the
// published quality profile must carry it.
const acceptedScorer = {
  bias_db_max: 0.5,
  presence_mismatch_percent_max: 0.25,
  quiet_floor_db: 10,
  threshold_percent_max: { 0.5: 20, 1: 1, 3: 0.01, 6: 0.001 },
  unified_threshold_db: 6,
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

// The worker set is cluster topology (scripts/layer-spec.json), deliberately not pinned by
// the serving contract — only the role↔selection cross-check below is.
const producerRoles = {
  'cpu-cruise': 'stock',
  'gpu-airborne': 'stock',
  'gpu-surface': 'w2-merged',
}

function modelRoleContract() {
  return {
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
      'gpu-airborne': worker('airborne-production', 'gpu-airborne', 'stock', 'airborne-stock-v1'),
      'gpu-surface': worker(
        'surface-production', 'gpu-surface', 'w2-merged', 'surface-w2-z13-accepted-v1',
      ),
    },
  }
}

/** Recompute both immutable identities after a fixture mutation. */
function resealGeneration(contract) {
  contract.quality_profile_id = sha256Identity(contract.quality)
  contract.generation_id = sha256Identity({
    schema: 1,
    zoom: contract.zoom,
    dataset_year: contract.dataset_year,
    raster_generation_id: contract.raster_generation_id,
    quality_profile_id: contract.quality_profile_id,
    quality_profile_name: contract.quality_profile_name,
  })
  return contract
}

function acceptedGeneration() {
  return resealGeneration({
    schema: 1,
    zoom: WORLD_BASE_ZOOM,
    dataset_year: 2026,
    raster_generation_id: '4'.repeat(16),
    quality_profile_id: '',
    quality_profile_name: ACCEPTED_PROFILE,
    generation_id: '',
    quality: {
      schema: 1,
      profile_name: ACCEPTED_PROFILE,
      product_commit: 'a'.repeat(40),
      dataset_year: 2026,
      model_role_contract: modelRoleContract(),
      numerical_environment: {},
      producer_requirements: { worker_model_roles: { ...producerRoles } },
      scorer_contract: structuredClone(acceptedScorer),
      wave: 'w2',
    },
  })
}

test('the canonical accepted z13 generation validates on both entry points', () => {
  assert.equal(validateGenerationContract(acceptedGeneration()).zoom, 13)
  assert.equal(
    validatePublishedGenerationContract(acceptedGeneration()).quality_profile_name,
    ACCEPTED_PROFILE,
  )
})

test('a base at any zoom but the world zoom is refused, however self-consistent', () => {
  const contract = acceptedGeneration()
  contract.zoom = 12
  resealGeneration(contract)
  assert.throws(() => validateGenerationContract(contract), /header is invalid/)
})

test('published boundary rejects a self-consistent unknown quality profile', () => {
  const contract = acceptedGeneration()
  contract.quality_profile_name = 'w2-z13-accepted-v2'
  contract.quality.profile_name = contract.quality_profile_name
  resealGeneration(contract)
  assert.equal(validateGenerationContract(contract).quality_profile_name, 'w2-z13-accepted-v2')
  assert.throws(
    () => validatePublishedGenerationContract(contract),
    /published quality profile is unsupported/,
  )
})

test('the accepted name cannot carry a spoofed payload, even resealed', () => {
  const cases = [
    ['wave', (contract) => { contract.quality.wave = 'exact' }],
    ['numerical environment', (contract) => {
      contract.quality.numerical_environment.SURFACE_BUDGET_ETA = '0'
    }],
    ['scorer bias', (contract) => { contract.quality.scorer_contract.bias_db_max = 0.6 }],
    ['scorer ladder', (contract) => {
      contract.quality.scorer_contract.threshold_percent_max[3] = 0.04
    }],
    ['presence area', (contract) => {
      contract.quality.scorer_contract.presence_mismatch_percent_max = 1.5
    }],
    ['unselected role', (contract) => {
      contract.quality.producer_requirements.worker_model_roles['gpu-surface'] = 'stock'
    }],
    ['absent worker', (contract) => {
      delete contract.quality.model_role_contract.workers['gpu-airborne']
    }],
    ['unowned environment key', (contract) => {
      contract.quality.numerical_environment.QM_W1_BUILDING_POLICY = 'adaptive-stride5'
    }],
  ]
  for (const [label, mutate] of cases) {
    const contract = acceptedGeneration()
    mutate(contract)
    resealGeneration(contract)
    assert.throws(
      () => validateGenerationContract(contract),
      undefined,
      `accepted a resealed contract with a wrong ${label}`,
    )
  }
})

test('both identities are recomputed, never trusted', () => {
  const wrongQualityId = acceptedGeneration()
  wrongQualityId.quality.product_commit = 'b'.repeat(40)
  assert.throws(
    () => validateGenerationContract(wrongQualityId),
    /quality_profile_id does not match its payload/,
  )
  const wrongGenerationId = acceptedGeneration()
  wrongGenerationId.raster_generation_id = '5'.repeat(16)
  assert.throws(
    () => validateGenerationContract(wrongGenerationId),
    /generation_id does not match its payload/,
  )
})

test('a retired tier field makes the contract unrecognizable', () => {
  const contract = acceptedGeneration()
  contract.tier = 'z13'
  assert.throws(
    () => validateGenerationContract(contract),
    /generation contract has missing or unexpected fields/,
  )
})
