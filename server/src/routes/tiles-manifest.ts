// GET /api/tiles-manifest — the shared development or separately approved production tiles.

import type { FastifyInstance } from 'fastify'
import { PMTILES_BASE } from './heatmap-shared.js'
import {
  manifestBaseZoom,
  PmtilesManifestPinMissingError,
  readCachedValidatedPmtilesManifest,
  type PmtilesManifest,
} from '../runtime-readiness.js'

type PublicManifestLayer = { file: string; build?: string }

/** Keep generation, scorer, model-role, hashes, and publisher proofs server-side. */
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

/** Serve the shared development pin or the approved production pin, never the packer merge head.
 * Validation stays server-side; the response contains only tile URLs and native zoom.
 * PUBLIC_TILE_BASE selects the tile origin without a frontend rebuild. */
export async function tilesManifestRoutes(app: FastifyInstance): Promise<void> {
  const tileBase = (process.env.PUBLIC_TILE_BASE || '').replace(/\/$/, '') || null
  app.get('/api/tiles-manifest', async (_req, reply) => {
    reply.header('Cache-Control', 'no-cache')
    let manifest
    try {
      manifest = await readCachedValidatedPmtilesManifest(PMTILES_BASE)
    } catch (e) {
      if (e instanceof PmtilesManifestPinMissingError) {
        return reply.code(404).send({ error: 'no build published' })
      }
      app.log?.error?.(`tiles-manifest: ${(e as Error).message}`)
      return reply.code(500).send({ error: 'manifest unreadable' })
    }
    return { ...publicManifest(manifest), tile_base: tileBase }
  })
}
