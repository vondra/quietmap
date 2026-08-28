/** Shared fail-closed validation for published base and refinement generation contracts. */
export type GenerationContract = Record<string, unknown> & {
  base_generation_id: string
  base_quality_profile_id: string
  base_quality_profile_name: string
  dataset_year: number
  deployment: string
  generation_id: string
  quality: Record<string, unknown>
  quality_profile_id: string
  quality_profile_name: string
  raster_generation_id: string
  schema: 1
  tier: string
  zoom: number
}

export const W2_SPATIAL_POPULATION_SCOPES: Readonly<{
  'wbench-orig': 'four-h3/490-rows/per-changed-layer'
  'wbench-s': 'mixed-pieces/374-rows/per-changed-layer'
}>
export const W2_SPATIAL_SCORER_CONTRACT: Readonly<{
  schema: string
  implementation_sha256: string
  population_scopes: typeof W2_SPATIAL_POPULATION_SCOPES
  spatial_tolerance_pixels: number
  spatial_match_policy: string
  threshold_percent_max: Readonly<Record<string, number>>
  quiet_threshold_percent_max: Readonly<Record<string, number>>
  presence_multiplicity_percent_max: number
  bias_db_max: number
  warm_reference_fingerprint: string
}>

export function canonicalJson(value: unknown): string
export function sha256Identity(value: unknown): string
export function numericalEnvironmentKeys(): string[]
export function validateQualificationClosureReference(reference: unknown): {
  file: string
  sha256: string
}
export function validateModelRoleContract(contract: unknown): Record<string, unknown>
export function validateScorerContract(
  contract: unknown,
  label?: string,
  profileName?: string,
): Record<string, unknown>
export function validateGenerationContract(contract: unknown): GenerationContract
export function validatePublishedGenerationContract(contract: unknown): GenerationContract
export function lineModelRoleSha256ForGeneration(contract: unknown): string
export function validateTierGenerationAnchor(
  base: unknown,
  tier: unknown,
  expectedTier?: string,
): GenerationContract
