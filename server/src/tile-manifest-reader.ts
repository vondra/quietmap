// Shared per-environment pmtiles manifest reader. ONE tile CDN origin serves every archive
// by build id — heatmap-pmtiles.ts is
// URL-addressed (`/api/tiles/:build/:layer/...`) and MUST NEVER import this module: gating the
// byte-serving route by environment would 404 a pin the instant another environment published
// something newer. What IS per-environment is the PIN itself — which build counts as "current"
// for prod vs dev1/dev2/dev3. This module is the ONE place that turns TILE_ENV into that pin's
// manifest path, shared by tiles-manifest.ts (serves it to the frontend), runtime-readiness.ts
// (boot-time gate), and cluster-disk.ts (dashboard "published layers" read).
import { existsSync } from 'node:fs'
import { join } from 'node:path'

export const ALLOWED_TILE_ENVS = ['prod', 'dev1', 'dev2', 'dev3'] as const
export type TileEnv = (typeof ALLOWED_TILE_ENVS)[number]

function isAllowedTileEnv(value: string | undefined): value is TileEnv {
  return !!value && (ALLOWED_TILE_ENVS as readonly string[]).includes(value)
}

/**
 * Mandatory, allowlisted, fail-closed: an unset or unrecognized TILE_ENV must NEVER silently
 * serve or report some OTHER environment's pin. `envOverride` exists for tests only — every
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

/**
 * The manifest path this environment must read: `current.{env}.json`. A missing
 * per-environment pointer while the legacy plain `current.json` still
 * exists means this checkout was never seeded — a configuration gap, not "nothing published
 * yet" — so this fails loud with the exact fix instead of silently falling back to the shared
 * pointer (a silent fallback would defeat per-env pins the moment any one checkout forgot to
 * seed). A checkout with NEITHER file is genuinely fresh (before its first-ever publish); this
 * still returns the (not-yet-existing) per-env path so callers see their ordinary "nothing
 * published" behavior (a read ENOENT), not this loud message.
 */
export function resolveManifestPath(pmtilesDir: string, envOverride?: string): string {
  const env = resolveTileEnv(envOverride)
  const perEnvPath = join(pmtilesDir, `current.${env}.json`)
  if (!existsSync(perEnvPath) && existsSync(join(pmtilesDir, 'current.json'))) {
    throw new Error(
      `${perEnvPath} is missing but a legacy current.json exists at ${pmtilesDir} — seed it: `
      + `cp ${join(pmtilesDir, 'current.json')} ${perEnvPath}`,
    )
  }
  return perEnvPath
}
