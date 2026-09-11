// Resolve the shared development tile manifest or the separately approved production manifest.
import { existsSync } from 'node:fs'
import { join } from 'node:path'

export const ALLOWED_TILE_ENVS = ['prod', 'dev1', 'dev2', 'dev3'] as const
export type TileEnv = (typeof ALLOWED_TILE_ENVS)[number]

function isAllowedTileEnv(value: string | undefined): value is TileEnv {
  return !!value && (ALLOWED_TILE_ENVS as readonly string[]).includes(value)
}

/**
 * Mandatory, allowlisted, fail-closed: an unset or unrecognized TILE_ENV must NEVER silently
 * select production or development tiles by accident. `envOverride` exists for tests only — every
 * real caller reads `process.env.TILE_ENV` (the default when the argument is omitted).
 */
export function resolveTileEnv(envOverride?: string): TileEnv {
  const raw = envOverride ?? process.env.TILE_ENV
  if (!isAllowedTileEnv(raw)) {
    throw new Error(
      `TILE_ENV must be one of ${ALLOWED_TILE_ENVS.join('|')} (got ${JSON.stringify(raw ?? '')}) `
      + '— set it in the service environment',
    )
  }
  return raw
}

/** Every development checkout follows dev1; the packer's merge head is never a served pin. */
export function resolveManifestPath(pmtilesDir: string, envOverride?: string): string {
  const env = resolveTileEnv(envOverride)
  const pointer = join(pmtilesDir, `current.${env === 'prod' ? 'prod' : 'dev1'}.json`)
  if (!existsSync(pointer) && existsSync(join(pmtilesDir, 'current.json'))) {
    throw new Error(`${pointer} is missing while the packer merge head exists — finish publication first`)
  }
  return pointer
}
