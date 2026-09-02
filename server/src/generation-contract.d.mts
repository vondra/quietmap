/** Fail-closed validation of one published tile generation; see the .mjs header for the
 *  exact `current.<env>.json` shape the publisher must emit. */
export type GenerationContract = Record<string, unknown> & {
  dataset_year: number
  generation_id: string
  quality: Record<string, unknown>
  quality_profile_id: string
  quality_profile_name: string
  raster_generation_id: string
  schema: 1
  zoom: number
}

export const WORLD_BASE_ZOOM: 13

export function canonicalJson(value: unknown): string
export function sha256Identity(value: unknown): string
export function numericalEnvironmentKeys(): string[]
export function validateModelRoleContract(contract: unknown): Record<string, unknown>
export function validateScorerContract(
  contract: unknown,
  label?: string,
): Record<string, unknown>
export function validateGenerationContract(contract: unknown): GenerationContract
export function validatePublishedGenerationContract(contract: unknown): GenerationContract
