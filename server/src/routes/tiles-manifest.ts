// GET /api/tiles-manifest — the currently published pmtiles generation FOR THIS ENVIRONMENT.

import type { FastifyInstance } from 'fastify'
import { stat } from 'node:fs/promises'
import { PMTILES_BASE } from './heatmap-shared.js'
import { PmtilesManifestPinMissingError, readValidatedPmtilesManifest } from '../runtime-readiness.js'
import { resolveManifestPath } from '../tile-manifest-reader.js'
import type { PublishedLineModel } from '../published-line-model.js'

const TILES_MANIFEST_VALIDATION_CACHE_MS = 10_000

/**
 * Serve `current.{TILE_ENV}.json` (docs/dev/checkout-restructure-plan.md Track 2 — per-env
 * pmtiles pins, resolved by `tile-manifest-reader.ts`), which the Rust packer's fan-out /
 * `worldctl promote` write atomically: `{build, created, layers: {<layer>: {file, sha256,
 * tiles, bytes}}}`. The packer's fields pass through verbatim — the manifest is the single
 * source of truth and reshaping it here would fork that truth (the shared boot-readiness
 * validator below rejects a torn, malformed, or semantically invalid manifest with a 500).
 * ONE deployment field is added on top: `tile_base` (env PUBLIC_TILE_BASE) tells
 * the frontend which HOSTNAME serves the tiles — that is serving
 * topology, which the packer can't know and which must be changeable per checkout/host
 * without a frontend rebuild. Absent env = null = same-origin (devex, localhost, canaries).
 * `no-cache` so the frontend's 10-minute re-poll revalidates instead of pinning an old
 * generation; 404 = no build published yet for this pin (the frontend then renders no tile
 * layers); 500 = TILE_ENV misconfigured or this checkout was never seeded (see
 * `resolveManifestPath`'s error message — logged, never sent to the client).
 */
export type TilesManifestRouteOptions = {
  publishedLineModel?: PublishedLineModel
}

export async function tilesManifestRoutes(
  app: FastifyInstance,
  options: TilesManifestRouteOptions = {},
): Promise<void> {
  const tileBase = (process.env.PUBLIC_TILE_BASE || '').replace(/\/$/, '') || null
  let cached: { identity: string; validatedAt: number;
    manifest: Awaited<ReturnType<typeof readValidatedPmtilesManifest>> } | null = null
  app.get('/api/tiles-manifest', async (_req, reply) => {
    reply.header('Cache-Control', 'no-cache')
    if (options.publishedLineModel?.enabled) {
      const published = options.publishedLineModel.mapManifest()
      if (!published) {
        return reply.code(503).send({ error: 'published line model unavailable' })
      }
      return { ...published, tile_base: tileBase }
    }
    let manifest
    let pinExists = false
    try {
      const pinPath = resolveManifestPath(PMTILES_BASE)
      const pin = await stat(pinPath, { bigint: true })
      pinExists = true
      const identity = `${pin.dev}:${pin.ino}:${pin.size}:${pin.mtimeNs}`
      const now = Date.now()
      if (cached?.identity === identity
          && now - cached.validatedAt < TILES_MANIFEST_VALIDATION_CACHE_MS) manifest = cached.manifest
      else {
        manifest = await readValidatedPmtilesManifest(PMTILES_BASE)
        cached = { identity, validatedAt: now, manifest }
      }
    } catch (e) {
      if (e instanceof PmtilesManifestPinMissingError
          || ((e as NodeJS.ErrnoException).code === 'ENOENT' && !pinExists)) {
        return reply.code(404).send({ error: 'no build published' })
      }
      app.log?.error?.(`tiles-manifest: ${(e as Error).message}`)
      return reply.code(500).send({ error: 'manifest unreadable' })
    }
    return { ...manifest, tile_base: tileBase }
  })
}
