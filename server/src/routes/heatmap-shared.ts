// Shared validation + path constants for the heatmap tile routes:
// heatmap-pmtiles.ts (versioned pmtiles archives) and heatmap-manifest.ts.
// One source of truth for the layer allowlist and the z/x/y bounds so the
// two serving paths can never drift apart.

import { resolve } from 'node:path'
import { WORLD_BASE_ZOOM } from '../generation-contract.mjs'
import { DATA_YEAR as YEAR } from '../data-year.js'

// The world is painted once at WORLD_BASE_ZOOM (512-px tiles); every zoom
// below it is a pyramid level of that same paint, down to z2.
export const MIN_ZOOM = 2
export const MAX_ZOOM = WORLD_BASE_ZOOM

// Each ID is its own tile tree / pmtiles archive with a distinct HM3
// `source_id` byte in the header. `total` is the precomputed energy-sum of
// every layer (build-heatmap-combine) — the default all-layers-on view served
// as one tile fetch.
export const ALLOWED_LAYERS = new Set([
  'total',
  'road',
  'rail',
  'industrial',
  'building',
  'aircraft-ground',
  'aircraft-airborne',
  'aircraft-cruise',
])

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

// TILES_PMTILES_DIR lets a server instance read another checkout's archives.
// Defaults to this checkout's release-layout data dir. Holds
// `{layer}.{build}.pmtiles` + the manifest file below.
export const PMTILES_BASE = process.env.TILES_PMTILES_DIR
  ? resolve(process.env.TILES_PMTILES_DIR)
  : resolve(import.meta.dirname, '..', '..', 'data', 'tiles', YEAR, 'pmtiles')

// The single manifest format: `{build, layers: {layer: {file, build?}}}`.
// TILES_MANIFEST_FILE overrides the file name (default: the dev1 pin the
// b546 set was published under).
export const MANIFEST_FILENAME = process.env.TILES_MANIFEST_FILE || 'current.dev1.json'
