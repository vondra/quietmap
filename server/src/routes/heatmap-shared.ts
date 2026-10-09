// Shared validation for the heatmap tile routes (heatmap-pmtiles.ts, heatmap-manifest.ts): one
// layer allowlist and one z/x/y bound, so the two can never drift apart.

// The world is painted once at WORLD_BASE_ZOOM (512-px tiles); every zoom
// below it is a pyramid level of that same paint, down to z2. Moving the base
// zoom means repainting the whole world — an owner decision, never a
// publication-time parameter.
export const WORLD_BASE_ZOOM = 13
export const MIN_ZOOM = 2
export const MAX_ZOOM = WORLD_BASE_ZOOM

// Each ID is its own tile tree / pmtiles archive with a distinct HM3
// `source_id` byte in the header. `total` is the precomputed energy-sum of
// every layer painted together — the default all-layers-on view served as one
// tile fetch.
export const ALLOWED_LAYERS = new Set([
  'total',
  'road',
  'railway',
  'industrial',
  'building',
  'ship',
  'aircraft',
])

/** The manifest naming the published archives, inside the tiles directory. */
export const MANIFEST_FILENAME = 'current.json'

export interface TileParams {
  layer: string
  z: number
  x: number
  y: number
}

/**
 * Validate the `:layer/:z/:x/:y` route params shared by both tile routes.
 * Returns the parsed params, or a human-readable error string the route
 * replies 400 with.
 */
export function parseTileParams(p: { layer: string; z: string; x: string; y: string }):
  | TileParams
  | string {
  if (!ALLOWED_LAYERS.has(p.layer)) {
    return `layer must be one of ${[...ALLOWED_LAYERS].join(', ')}`
  }
  const z = Number(p.z); const x = Number(p.x); const y = Number(p.y)
  if (!Number.isInteger(z) || z < MIN_ZOOM || z > MAX_ZOOM) return 'bad zoom'
  const max = 2 ** z
  if (!Number.isInteger(x) || x < 0 || x >= max) return 'bad x'
  if (!Number.isInteger(y) || y < 0 || y >= max) return 'bad y'
  return { layer: p.layer, z, x, y }
}
