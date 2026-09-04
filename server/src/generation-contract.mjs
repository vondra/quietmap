//! Fail-closed validation of one published tile generation, and the shape of the manifest
//! that carries them.
//!
//! The world is painted ONCE, at `WORLD_BASE_ZOOM`; every lower zoom is a plain pyramid level
//! of that same paint (`build-pyramid --tiles-from`). There is no refinement tier and no
//! second base. A generation describes ONE painting run, and a manifest carries one
//! generation PER LAYER — a run that repaints a single layer carries the other seven forward
//! verbatim, which is what makes a partial repaint cheap.
//!
//! ## The manifest the publisher must emit — `current.<env>.json`
//!
//! Worked example: run `b42` repainted `industrial` only. `road` (and the six layers elided
//! here) were carried forward from run `b40`, so their `build` AND their `generation` are
//! older than the manifest's own `build`. That is the normal steady state, not an edge case.
//!
//! ```json
//! {
//!   "build": "b42",
//!   "created_unix": 1785727363,
//!   "layers": {
//!     "industrial": {
//!       "file": "industrial.b42.pmtiles",
//!       "build": "b42",
//!       "bytes": 28834262640,
//!       "sha256": "<sha256 of the archive>",
//!       "tiles": 1062860,
//!       "generation": { "...": "the contract below, as painted by run b42" }
//!     },
//!     "road": {
//!       "file": "road.b40.pmtiles",
//!       "build": "b40",
//!       "bytes": 124097336119,
//!       "sha256": "<sha256 of the archive>",
//!       "tiles": 2216058,
//!       "generation": { "...": "run b40's contract, byte-identical to what b40 published" }
//!     }
//!   }
//! }
//! ```
//!
//! Per entry: `file`, `bytes`, `sha256` and (once fenced) `generation` are required and
//! enforced. `build` is required of the publisher and enforced whenever present — the build
//! inside `file` is the authority, and the field only restates it. `created_unix`, `tiles`
//! and `publisher_proof` ride along untouched: readiness never judges them, but
//! `publisher_proof` is still written by the packer and consumed by `tile-store-fsck` and
//! `--rebind-verified`, so a publisher must keep emitting it.
//!
//! What the publisher must honour, all of it enforced by `runtime-readiness.ts`:
//!
//!  * every one of the eight `ALLOWED_LAYERS` has an entry, `file` is `<layer>.<build>.pmtiles`
//!    and the build inside the file name equals `entry.build` when that field is present;
//!  * an entry whose `build` differs from the manifest's own `build` IS the carry-forward
//!    case: its bytes, its `sha256` and its `generation` are the earlier run's, unchanged.
//!    A partial publish never repacks an untouched layer;
//!  * every entry carries its own `generation`, each valid on its own, and all eight agree on
//!    `dataset_year` — one manifest is one dataset year (the store path already says which);
//!  * nothing else is generation-fenced, and the retired fields are REFUSED, not ignored: a
//!    manifest carrying a top-level `generation`, `line_model_role_sha256`, `tiers` or
//!    `qualification_closure` fails readiness, because a publisher still writing them may
//!    have painted at another zoom. Each layer's own `quality.model_role_contract` carries
//!    the line-model identity that used to sit at the top level. Rollback is a
//!    whole-manifest pointer flip: every entry is carried by value, so the previous
//!    `current.<env>.json` is complete on its own.
//!
//! One layer's `generation`:
//!
//! ```json
//! {
//!   "schema": 1,
//!   "zoom": 13,
//!   "dataset_year": 2026,
//!   "raster_generation_id": "<16 hex>",
//!   "quality_profile_name": "w2-z13-accepted-v1",
//!   "quality_profile_id": "<sha256 of `quality`, canonical JSON>",
//!   "generation_id": "<sha256 of the identity below, canonical JSON>",
//!   "quality": { "...": "see validateQuality" }
//! }
//! ```
//!
//! The hashed identity is `{schema, zoom, dataset_year, raster_generation_id,
//! quality_profile_id, quality_profile_name}` — the same six values the contract stores. A
//! generation is therefore exactly "this quality payload, over these rasters, at the world
//! zoom, for this dataset year", and two runs with identical inputs share one identity.

import { createHash } from 'node:crypto'

const SHA256 = /^[0-9a-f]{64}$/
const COMMIT = /^[0-9a-f]{40}$/
const RASTER_GENERATION = /^[0-9a-f]{16}$/
const NAME = /^[a-z][a-z0-9-]*$/
const ENV_NAME = /^[A-Z][A-Z0-9_]*$/

/**
 * The one zoom the world is painted at. Lower zooms are pyramid levels of the same
 * generation, so this is the only zoom a contract may claim. Moving it means repainting the
 * whole world — an owner decision, never a publication-time parameter.
 */
export const WORLD_BASE_ZOOM = 13

const W2_ACCEPTED_PROFILE = 'w2-z13-accepted-v1'
const PUBLISHED_QUALITY_PROFILES = new Set([W2_ACCEPTED_PROFILE])

/**
 * The Wave-2 ladder the published world is qualified against, in the units the quality
 * profile stores: dB thresholds as keys, percentages of aggregate reference-painted cells
 * as values. Every number is the owner's consolidated ladder of 2026-08-13, and its one
 * source of truth is the scorer that enforces it,
 * `engine/tile-painter/src/accuracy_contract.rs`: `WAVE_TWO_BASELINE_FRACTION` 0.20 at
 * >0.5 dB, `WAVE_TWO_MIDDLE_FRACTION` 0.01 at >1 dB, `WAVE_TWO_HIGH_FRACTION` 0.0001 at
 * >3 dB, `WAVE_TWO_UNIFIED_TAIL_FRACTION` 0.00001 at >6 dB (`WAVE_TWO_UNIFIED_TAIL_OVER_DB`),
 * `WAVE_TWO_PRESENCE_FRACTION` 0.0025, `WAVE_TWO_QUIET_MAX_DB` 10, and the bias bound
 * `MAX_AGGREGATE_SIGNED_MEAN_DB` 0.5. Moving any of them is an owner ruling, never
 * candidate-driven tuning.
 */
const W2_ACCEPTED_SCORER_CONTRACT = Object.freeze({
  bias_db_max: 0.5,
  presence_mismatch_percent_max: 0.25,
  quiet_floor_db: 10,
  threshold_percent_max: Object.freeze({ 0.5: 20, 1: 1, 3: 0.01, 6: 0.001 }),
  unified_threshold_db: 6,
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
  'QM_SEG_SAMPLES',
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

export function validateScorerContract(contract, label = 'quality') {
  requireCondition(contract && !Array.isArray(contract) && typeof contract === 'object',
    label + ' scorer_contract is not an object')
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

/**
 * What the published profile NAME promises: the accuracy ladder it was qualified against, and
 * that it painted with no numerical environment override. Deliberately NOT pinned here: which
 * worker paints which layer. That is cluster topology (`scripts/layer-spec.json`), it is
 * already bound worker-by-worker in validateQuality against the selected model-role contract,
 * and a second copy of the map inside the serving contract would be a duplicate truth.
 */
function validateNamedQualitySemantics(quality, profileName) {
  if (profileName !== W2_ACCEPTED_PROFILE) return
  requireCondition(quality.wave === 'w2', 'accepted W2 profile wave differs')
  requireCanonicalValue(quality.numerical_environment, {},
    'accepted W2 numerical environment')
  requireCanonicalValue(quality.scorer_contract, W2_ACCEPTED_SCORER_CONTRACT,
    'accepted W2 scorer contract')
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
    && ['exact', 'w2'].includes(quality.wave),
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
  validateScorerContract(quality.scorer_contract)
  validateNamedQualitySemantics(quality, profileName)
}

/** Validate one persisted layer generation and recompute both immutable identities. */
export function validateGenerationContract(contract) {
  exactKeys(contract, [
    'dataset_year',
    'generation_id',
    'quality',
    'quality_profile_id',
    'quality_profile_name',
    'raster_generation_id',
    'schema',
    'zoom',
  ], 'generation contract')
  requireCondition(contract.schema === 1
    && contract.zoom === WORLD_BASE_ZOOM
    && Number.isSafeInteger(contract.dataset_year)
    && contract.dataset_year >= 2000 && contract.dataset_year <= 2200,
  'header is invalid')
  requireCondition(SHA256.test(contract.generation_id)
    && SHA256.test(contract.quality_profile_id)
    && RASTER_GENERATION.test(contract.raster_generation_id),
  'identity is invalid')
  requireCondition(NAME.test(contract.quality_profile_name), 'profile name is invalid')
  validateQuality(contract.quality, contract.quality_profile_name, contract.dataset_year)
  requireCondition(sha256Identity(contract.quality) === contract.quality_profile_id,
    'quality_profile_id does not match its payload')
  const identity = {
    schema: 1,
    zoom: contract.zoom,
    dataset_year: contract.dataset_year,
    raster_generation_id: contract.raster_generation_id,
    quality_profile_id: contract.quality_profile_id,
    quality_profile_name: contract.quality_profile_name,
  }
  requireCondition(sha256Identity(identity) === contract.generation_id,
    'generation_id does not match its payload')
  return contract
}

/** Reject structurally valid benchmark/test profiles at the public serving boundary. */
export function validatePublishedGenerationContract(contract) {
  const validated = validateGenerationContract(contract)
  requireCondition(PUBLISHED_QUALITY_PROFILES.has(validated.quality_profile_name),
    'published quality profile is unsupported: ' + validated.quality_profile_name)
  return validated
}
