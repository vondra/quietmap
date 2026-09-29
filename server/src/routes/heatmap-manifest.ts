// GET /api/tiles-manifest — the currently published pmtiles generation.
//
// The manifest `{build, zoom?, layers: {layer: {file, build?}}}` is MANIFEST_FILENAME in the
// tiles directory. The route projects only the fields needed to fetch tiles: generation, hashes
// and publisher proofs never cross the public boundary. `zoom` is the zoom the world was painted
// at (the deepest natively-servable zoom); a manifest without it serves at WORLD_BASE_ZOOM.

import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import type { FastifyInstance } from 'fastify'
import { ALLOWED_LAYERS, MANIFEST_FILENAME, MIN_ZOOM, WORLD_BASE_ZOOM } from './heatmap-shared.ts'

const BUILD_ID = /^b\d+$/
const SHA256 = /^[a-f0-9]{64}$/
const ARCHIVE_NAME = new RegExp(`^(${[...ALLOWED_LAYERS].join('|')})\\.(b\\d+)\\.pmtiles$`)

export type ManifestLayer = {
  file?: unknown
  build?: unknown
  bytes?: unknown
  sha256?: unknown
  [key: string]: unknown
}

export type PmtilesManifest = {
  build?: unknown
  zoom?: unknown
  layers?: Record<string, ManifestLayer>
  [key: string]: unknown
}

export class ManifestMissingError extends Error {
  readonly code = 'QM_PMTILES_MANIFEST_MISSING'
}

/** The zoom every archive in this manifest was painted at — the deepest zoom
 *  the tile routes may serve from it, and the number the frontend receives. */
export function manifestBaseZoom(manifest: PmtilesManifest): number {
  return Number.isInteger(manifest.zoom) && (manifest.zoom as number) >= MIN_ZOOM
    ? manifest.zoom as number
    : WORLD_BASE_ZOOM
}

/** Structural validation of one already-parsed manifest. Archive presence on
 *  disk is deliberately NOT checked: a mid-sync directory still serves its pin
 *  and the tile route 404s the not-yet-synced archives. */
export function validatePmtilesManifest(manifest: PmtilesManifest, manifestPath: string): void {
  if (typeof manifest.build !== 'string' || !BUILD_ID.test(manifest.build)) {
    throw new Error(`${manifestPath} has an invalid build id`)
  }
  if (!manifest.layers || typeof manifest.layers !== 'object') {
    throw new Error(`${manifestPath} has no layers object`)
  }
  for (const layer of ALLOWED_LAYERS) {
    if (!(layer in manifest.layers)) throw new Error(`${manifestPath} is missing layer ${layer}`)
  }
  for (const [layer, entry] of Object.entries(manifest.layers)) {
    if (!ALLOWED_LAYERS.has(layer)) throw new Error(`${manifestPath} has unexpected layer ${layer}`)
    const file = entry?.file
    if (typeof file !== 'string') throw new Error(`${manifestPath} layer ${layer} has no file`)
    const match = ARCHIVE_NAME.exec(file)
    // Basename-only by construction (no slashes can match), and the name must
    // be exactly this layer's own archive — never another layer's.
    if (!match || match[1] !== layer) {
      throw new Error(`${manifestPath} layer ${layer} file does not match the served archive name`)
    }
    if (entry.build !== undefined) {
      if (typeof entry.build !== 'string' || !BUILD_ID.test(entry.build)) {
        throw new Error(`${manifestPath} layer ${layer} has an invalid build id`)
      }
      if (entry.build !== match[2]) {
        throw new Error(`${manifestPath} layer ${layer} build does not match its archive file`)
      }
    }
    // bytes/sha256 are advisory: validated when present so a half-written
    // manifest still fails loud, ignored when absent.
    if (entry.bytes !== undefined
      && (!Number.isSafeInteger(entry.bytes) || (entry.bytes as number) <= 0)) {
      throw new Error(`${manifestPath} layer ${layer} has invalid bytes`)
    }
    if (entry.sha256 !== undefined
      && (typeof entry.sha256 !== 'string' || !SHA256.test(entry.sha256))) {
      throw new Error(`${manifestPath} layer ${layer} has invalid sha256`)
    }
  }
}

/** Read and validate the manifest. Throws ManifestMissingError when nothing
 *  was ever published. Deliberately uncached: the manifest is a few KiB and
 *  validation is pure structure (no archive stats), so every request sees a
 *  republish immediately. */
export async function readValidatedPmtilesManifest(tilesDir: string): Promise<PmtilesManifest> {
  const manifestPath = join(tilesDir, MANIFEST_FILENAME)
  let raw: string
  try {
    raw = await readFile(manifestPath, 'utf8')
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === 'ENOENT') {
      throw new ManifestMissingError(`${manifestPath} does not exist`)
    }
    throw e
  }
  const manifest = JSON.parse(raw) as PmtilesManifest
  validatePmtilesManifest(manifest, manifestPath)
  return manifest
}

type PublicManifestLayer = { file: string; build?: string }

/** Keep everything but the fetch coordinates server-side. */
function publicManifest(manifest: PmtilesManifest) {
  const layers: Record<string, PublicManifestLayer> = {}
  for (const [name, value] of Object.entries(manifest.layers ?? {})) {
    if (!value || typeof value.file !== 'string') continue
    layers[name] = {
      file: value.file,
      ...(typeof value.build === 'string' ? { build: value.build } : {}),
    }
  }
  return { build: manifest.build, zoom: manifestBaseZoom(manifest), layers }
}

export async function heatmapManifestRoutes(app: FastifyInstance, { tilesDir }: { tilesDir: string }): Promise<void> {
  app.get('/api/tiles-manifest', async (_req, reply) => {
    reply.header('Cache-Control', 'no-cache')
    let manifest
    try {
      manifest = await readValidatedPmtilesManifest(tilesDir)
    } catch (e) {
      if (e instanceof ManifestMissingError) {
        return reply.code(404).send({ error: 'no build published' })
      }
      app.log?.error?.(`tiles-manifest: ${(e as Error).message}`)
      return reply.code(500).send({ error: 'manifest unreadable' })
    }
    return publicManifest(manifest)
  })
}
