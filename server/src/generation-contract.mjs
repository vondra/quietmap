//! Shared fail-closed validation for published base and refinement generation contracts.

import { createHash } from 'node:crypto'

const SHA256 = /^[0-9a-f]{64}$/
const COMMIT = /^[0-9a-f]{40}$/
const RASTER_GENERATION = /^[0-9a-f]{16}$/
const NAME = /^[a-z][a-z0-9-]*$/
const ENV_NAME = /^[A-Z][A-Z0-9_]*$/
const QUALIFICATION_FILE = /^qualification-[0-9a-f]{64}\.json$/
const W1_ACCEPTED_PROFILE = 'w1-z12-accepted-v1'
const W2_SPATIAL_PROFILE = 'w2-z13-spatial-v1'
const W2_SPATIAL_IMPLEMENTATION_SHA256 =
  'a75d6e0633c9417bf90cb201a7bdd22fa7d6d46fe39f0518bfe9323cf5d27c68'
const W2_SPATIAL_WARM_REFERENCE_FINGERPRINT =
  'c92bc8ac4159c2759645cbf5948077ce024d55d633373a6b2aed5c1a7b547dc9'
export const W2_SPATIAL_POPULATION_SCOPES = Object.freeze({
  'wbench-orig': 'four-h3/490-rows/per-changed-layer',
  'wbench-s': 'mixed-pieces/374-rows/per-changed-layer',
})
const STOCK_PRODUCER_ROLES = Object.freeze({
  'cpu-airborne': 'stock',
  'cpu-building': 'stock',
  'cpu-cruise': 'stock',
  'cpu-ground': 'stock',
  'cpu-industrial': 'stock',
  'gpu-airborne': 'stock',
})
const W1_ACCEPTED_PRODUCER_ROLES = Object.freeze({
  ...STOCK_PRODUCER_ROLES,
  'gpu-line': 'w1',
})
const W2_SPATIAL_PRODUCER_ROLES = Object.freeze({
  ...STOCK_PRODUCER_ROLES,
  'gpu-line': 'w2-stride4',
})
const W1_ACCEPTED_NUMERICAL_ENVIRONMENT = Object.freeze({
  QM_W1_INDUSTRIAL_POLICY: 'adaptive-stride5',
})
const W1_ACCEPTED_SCORER_CONTRACT = Object.freeze({
  bias_db_max: 0.5,
  presence_mismatch_percent_max: 6,
  quiet_floor_db: 26,
  threshold_percent_max: Object.freeze({ 1: 30, 2: 15, 6: 1.5 }),
})
const W2_SPATIAL_SCORER_CONTRACT = Object.freeze({
  schema: 'w2-z13-spatial-scorer-v2',
  implementation_sha256: W2_SPATIAL_IMPLEMENTATION_SHA256,
  population_scopes: W2_SPATIAL_POPULATION_SCOPES,
  spatial_tolerance_pixels: 1,
  spatial_match_policy: 'symmetric-chebyshev-r1-directional-min-plus-histogram-capacity-v1',
  threshold_percent_max: Object.freeze({ 0.5: 2, 1: 1, 3: 0.25, 6: 0.05 }),
  quiet_threshold_percent_max: Object.freeze({ 10: 0.01, 15: 0.001 }),
  presence_multiplicity_percent_max: 0.25,
  bias_db_max: 0.5,
  warm_reference_fingerprint: W2_SPATIAL_WARM_REFERENCE_FINGERPRINT,
})
const ALLOWED_NUMERICAL_ENVIRONMENT = new Set([
  'NOISE_GPU_DISABLE_PRUNE',
  'NOISE_GPU_HALO_M',
  'QM_ARC_DISABLE_CELL_PRUNE',
  'QM_ARC_EXACT',
  'QM_ARC_MAX_ARCS',
  'QM_ARC_MAX_INTERVALS',
  'QM_ARC_MIN_SPAN_DEG',
  'QM_ARC_NEED_CLIP_M',
  'QM_ARC_RADIUS_M',
  'QM_GPU_BARRIERS',
  'QM_OBSTACLES_ALLOW_PARTIAL',
  'QM_OBSTACLES_DIR',
  'QM_SEG_SAMPLES',
  'QM_VECTOR_BUILDINGS',
  'QM_W1_INDUSTRIAL_POLICY',
  'SURFACE_BLOCK_PX',
  'SURFACE_BOUND_M3',
  'SURFACE_BUDGET_ETA',
  'SURFACE_SHADOW_RX_ZONE_M',
  'SURFACE_SHADOW_SRC_ZONE_M',
  'SURFACE_SHADOW_STRIDE',
])

function fail(message) {
  throw new Error('generation contract: ' + message)
}

function requireCondition(condition, message) {
  if (!condition) fail(message)
}

function exactKeys(value, keys, label) {
  requireCondition(value && !Array.isArray(value) && typeof value === 'object',
    label + ' is not an object')
  requireCondition(Object.keys(value).sort().join(',') === [...keys].sort().join(','),
    label + ' has missing or unexpected fields')
}

/** Return the single canonical JSON encoding used by every generation identity. */
export function canonicalJson(value) {
  if (Array.isArray(value)) return '[' + value.map(canonicalJson).join(',') + ']'
  if (value && typeof value === 'object') {
    return '{' + Object.keys(value).sort().map(key =>
      JSON.stringify(key) + ':' + canonicalJson(value[key])).join(',') + '}'
  }
  return JSON.stringify(value)
}

/** Hash a value after canonical JSON encoding. */
export function sha256Identity(value) {
  return createHash('sha256').update(canonicalJson(value)).digest('hex')
}

export function numericalEnvironmentKeys() {
  return [...ALLOWED_NUMERICAL_ENVIRONMENT].sort()
}

/** Validate the content-addressed leaf that closes a qualified tier publication. */
export function validateQualificationClosureReference(reference) {
  exactKeys(reference, ['file', 'sha256'], 'qualification closure reference')
  requireCondition(SHA256.test(reference.sha256),
    'qualification closure reference sha256 is invalid')
  requireCondition(QUALIFICATION_FILE.test(reference.file)
    && reference.file === `qualification-${reference.sha256}.json`,
  'qualification closure reference is not content-addressed')
  return reference
}

export function validateModelRoleContract(contract) {
  exactKeys(contract, [
    'line_model_role_sha256',
    'model_source_recipe_sha256',
    'numerical_selection_record_sha256',
    'output_abi_version',
    'role_spec_sha256',
    'schema',
    'workers',
  ], 'model-role contract')
  requireCondition(contract.schema === 1, 'model-role contract schema is not 1')
  for (const field of [
    'line_model_role_sha256',
    'model_source_recipe_sha256',
    'role_spec_sha256',
  ]) {
    requireCondition(SHA256.test(contract[field]),
      'model-role contract ' + field + ' is invalid')
  }
  requireCondition(contract.numerical_selection_record_sha256 === null
    || SHA256.test(contract.numerical_selection_record_sha256),
  'model-role contract numerical_selection_record_sha256 is invalid')
  requireCondition(Number.isSafeInteger(contract.output_abi_version)
    && contract.output_abi_version > 0, 'model-role contract output ABI is invalid')
  requireCondition(contract.workers && !Array.isArray(contract.workers)
    && typeof contract.workers === 'object' && Object.keys(contract.workers).length > 0,
  'model-role contract workers are invalid')
  for (const [workerName, worker] of Object.entries(contract.workers)) {
    requireCondition(NAME.test(workerName), 'model-role contract worker name is invalid')
    exactKeys(worker, [
      'artifact_family',
      'binary',
      'model_role',
      'resolved_role',
      'selection_epoch',
    ], 'model-role contract worker ' + workerName)
    for (const field of ['artifact_family', 'binary', 'model_role', 'resolved_role']) {
      requireCondition(NAME.test(worker[field]),
        'model-role contract worker ' + workerName + ' ' + field + ' is invalid')
    }
    requireCondition(worker.selection_epoch === null
      || (Number.isSafeInteger(worker.selection_epoch) && worker.selection_epoch > 0),
    'model-role contract worker ' + workerName + ' selection_epoch is invalid')
  }
  return contract
}

export function validateScorerContract(contract, label = 'quality', profileName = undefined) {
  requireCondition(contract && !Array.isArray(contract) && typeof contract === 'object',
    label + ' scorer_contract is not an object')
  if (profileName === W2_SPATIAL_PROFILE || Object.hasOwn(contract, 'spatial_match_policy')) {
    requireCondition(profileName === undefined || profileName === W2_SPATIAL_PROFILE,
      label + ' spatial scorer is bound to the wrong profile')
    exactKeys(contract, [
      'bias_db_max',
      'implementation_sha256',
      'population_scopes',
      'presence_multiplicity_percent_max',
      'quiet_threshold_percent_max',
      'schema',
      'spatial_match_policy',
      'spatial_tolerance_pixels',
      'threshold_percent_max',
      'warm_reference_fingerprint',
    ], label + ' spatial scorer_contract')
    requireCondition(contract.schema === 'w2-z13-spatial-scorer-v2'
      && contract.implementation_sha256 === W2_SPATIAL_IMPLEMENTATION_SHA256
      && contract.warm_reference_fingerprint === W2_SPATIAL_WARM_REFERENCE_FINGERPRINT
      && canonicalJson(contract.population_scopes)
        === canonicalJson(W2_SPATIAL_POPULATION_SCOPES)
      && contract.spatial_tolerance_pixels === 1
      && contract.spatial_match_policy
        === 'symmetric-chebyshev-r1-directional-min-plus-histogram-capacity-v1'
      && contract.presence_multiplicity_percent_max === 0.25
      && contract.bias_db_max === 0.5,
    label + ' spatial scorer identity or fixed bounds differ')
    requireCondition(canonicalJson(contract.threshold_percent_max)
      === canonicalJson({ 0.5: 2, 1: 1, 3: 0.25, 6: 0.05 }),
    label + ' spatial painted ladder differs')
    requireCondition(canonicalJson(contract.quiet_threshold_percent_max)
      === canonicalJson({ 10: 0.01, 15: 0.001 }),
    label + ' spatial quiet ladder differs')
    return contract
  }
  const allowed = new Set([
    'bias_db_max',
    'presence_mismatch_percent_max',
    'quiet_floor_db',
    'threshold_percent_max',
    'unified_threshold_db',
  ])
  requireCondition(Object.keys(contract).every(key => allowed.has(key)),
    label + ' scorer_contract has an unexpected field')
  requireCondition(Number.isFinite(contract.bias_db_max) && contract.bias_db_max >= 0,
    label + ' scorer bias is invalid')
  requireCondition(Number.isFinite(contract.presence_mismatch_percent_max)
    && contract.presence_mismatch_percent_max >= 0,
  label + ' scorer presence threshold is invalid')
  requireCondition(contract.threshold_percent_max && !Array.isArray(contract.threshold_percent_max)
    && typeof contract.threshold_percent_max === 'object'
    && Object.keys(contract.threshold_percent_max).length > 0,
  label + ' scorer threshold ladder is empty')
  for (const [db, percent] of Object.entries(contract.threshold_percent_max)) {
    requireCondition(Number.isFinite(+db) && +db > 0
      && Number.isFinite(percent) && percent >= 0,
    label + ' scorer threshold ' + db + ' is invalid')
  }
  for (const optional of ['quiet_floor_db', 'unified_threshold_db']) {
    requireCondition(contract[optional] === undefined
      || (Number.isFinite(contract[optional]) && contract[optional] >= 0),
    label + ' scorer ' + optional + ' is invalid')
  }
  return contract
}

function requireCanonicalValue(actual, expected, label) {
  requireCondition(canonicalJson(actual) === canonicalJson(expected), label + ' differs')
}

function validateNamedQualitySemantics(quality, profileName) {
  if (profileName === W1_ACCEPTED_PROFILE) {
    requireCondition(quality.wave === 'w1', 'accepted W1 profile wave differs')
    requireCanonicalValue(
      quality.numerical_environment,
      W1_ACCEPTED_NUMERICAL_ENVIRONMENT,
      'accepted W1 numerical environment',
    )
    requireCanonicalValue(
      quality.producer_requirements.worker_model_roles,
      W1_ACCEPTED_PRODUCER_ROLES,
      'accepted W1 producer role map',
    )
    requireCanonicalValue(
      quality.scorer_contract,
      W1_ACCEPTED_SCORER_CONTRACT,
      'accepted W1 scorer contract',
    )
  } else if (profileName === W2_SPATIAL_PROFILE) {
    requireCondition(quality.wave === 'w2', 'W2 spatial profile wave differs')
    requireCanonicalValue(quality.numerical_environment, {}, 'W2 spatial numerical environment')
    requireCanonicalValue(
      quality.producer_requirements.worker_model_roles,
      W2_SPATIAL_PRODUCER_ROLES,
      'W2 spatial producer role map',
    )
    requireCanonicalValue(
      quality.scorer_contract,
      W2_SPATIAL_SCORER_CONTRACT,
      'W2 spatial scorer contract',
    )
  }
}

function validateQuality(quality, profileName, datasetYear) {
  exactKeys(quality, [
    'dataset_year',
    'model_role_contract',
    'numerical_environment',
    'producer_requirements',
    'product_commit',
    'profile_name',
    'schema',
    'scorer_contract',
    'wave',
  ], 'quality payload')
  requireCondition(quality.schema === 1
    && quality.profile_name === profileName
    && quality.dataset_year === datasetYear
    && COMMIT.test(quality.product_commit)
    && ['exact', 'w1', 'w2'].includes(quality.wave),
  'quality payload header is invalid')
  validateModelRoleContract(quality.model_role_contract)
  requireCondition(quality.numerical_environment
    && !Array.isArray(quality.numerical_environment)
    && typeof quality.numerical_environment === 'object',
  'quality numerical_environment is not an object')
  for (const [key, value] of Object.entries(quality.numerical_environment)) {
    requireCondition(ENV_NAME.test(key) && ALLOWED_NUMERICAL_ENVIRONMENT.has(key),
      'quality contains unowned numerical environment ' + key)
    requireCondition(typeof value === 'string' && value.length > 0 && value.length <= 256,
      'quality numerical environment ' + key + ' is invalid')
  }
  exactKeys(quality.producer_requirements, ['worker_model_roles'],
    'quality producer_requirements')
  const roles = quality.producer_requirements.worker_model_roles
  requireCondition(roles && !Array.isArray(roles) && typeof roles === 'object'
    && Object.keys(roles).length > 0, 'quality worker role requirements are empty')
  for (const [workerName, role] of Object.entries(roles)) {
    requireCondition(NAME.test(workerName) && NAME.test(role),
      'quality worker role requirement is invalid')
    const selected = quality.model_role_contract.workers[workerName]
    requireCondition(selected,
      'quality requires absent worker ' + workerName)
    requireCondition(selected.model_role === role,
      'quality requires ' + workerName + ' role ' + role
        + ', selected ' + selected.model_role)
  }
  validateScorerContract(quality.scorer_contract, 'quality', profileName)
  validateNamedQualitySemantics(quality, profileName)
}

/** Validate a complete persisted contract and recompute both immutable identities. */
export function validateGenerationContract(contract) {
  exactKeys(contract, [
    'base_generation_id',
    'base_quality_profile_id',
    'base_quality_profile_name',
    'dataset_year',
    'deployment',
    'generation_id',
    'quality',
    'quality_profile_id',
    'quality_profile_name',
    'raster_generation_id',
    'schema',
    'tier',
    'zoom',
  ], 'generation contract')
  requireCondition(contract.schema === 1
    && NAME.test(contract.deployment)
    && Number.isSafeInteger(contract.zoom)
    && contract.zoom >= 0 && contract.zoom <= 30
    && Number.isSafeInteger(contract.dataset_year)
    && contract.dataset_year >= 2000 && contract.dataset_year <= 2200,
  'header is invalid')
  requireCondition(contract.tier === ''
    ? contract.zoom === 12
    : contract.tier === 'z' + contract.zoom,
  'tier does not match the published zoom')
  requireCondition(SHA256.test(contract.generation_id)
    && SHA256.test(contract.base_generation_id)
    && SHA256.test(contract.quality_profile_id)
    && SHA256.test(contract.base_quality_profile_id)
    && RASTER_GENERATION.test(contract.raster_generation_id),
  'identity is invalid')
  requireCondition(NAME.test(contract.quality_profile_name)
    && NAME.test(contract.base_quality_profile_name),
  'profile name is invalid')
  validateQuality(contract.quality, contract.quality_profile_name, contract.dataset_year)
  if (contract.quality_profile_name === W1_ACCEPTED_PROFILE) {
    requireCondition(contract.deployment === 'base'
      && contract.zoom === 12 && contract.tier === '',
    'accepted W1 profile is only valid for the base z12 deployment')
  } else if (contract.quality_profile_name === W2_SPATIAL_PROFILE) {
    requireCondition(contract.deployment === 'z13'
      && contract.zoom === 13 && contract.tier === 'z13'
      && contract.base_quality_profile_name === W1_ACCEPTED_PROFILE,
    'W2 spatial profile requires the z13 refinement deployment and accepted W1 base')
  }
  requireCondition(sha256Identity(contract.quality) === contract.quality_profile_id,
    'quality_profile_id does not match its payload')
  const tiered = contract.tier !== ''
  const identity = {
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
  requireCondition(sha256Identity(identity) === contract.generation_id,
    'generation_id does not match its payload')
  if (!tiered) {
    requireCondition(contract.generation_id === contract.base_generation_id
      && contract.quality_profile_id === contract.base_quality_profile_id
      && contract.quality_profile_name === contract.base_quality_profile_name,
    'base generation does not anchor itself')
  }
  return contract
}

/** Return the product-wide model-role identity bound inside a validated generation. */
export function lineModelRoleSha256ForGeneration(contract) {
  return validateGenerationContract(contract)
    .quality.model_role_contract.line_model_role_sha256
}

/** Require one tier contract to be rooted in the exact currently served base generation. */
export function validateTierGenerationAnchor(base, tier, expectedTier = undefined) {
  validateGenerationContract(base)
  validateGenerationContract(tier)
  requireCondition(base.tier === '' && tier.tier !== '',
    'tier anchor needs one base and one tier contract')
  requireCondition(expectedTier === undefined || tier.tier === expectedTier,
    'tier contract differs from its tier index')
  requireCondition(tier.base_generation_id === base.generation_id
    && tier.base_quality_profile_id === base.quality_profile_id
    && tier.base_quality_profile_name === base.quality_profile_name
    && tier.dataset_year === base.dataset_year
    && tier.raster_generation_id === base.raster_generation_id,
  'tier is not anchored to the live base generation/profile')
  return tier
}
