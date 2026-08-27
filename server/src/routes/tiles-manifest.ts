// GET /api/tiles-manifest — the currently published pmtiles generation FOR THIS ENVIRONMENT.

import type { FastifyInstance } from 'fastify'
import { PMTILES_BASE } from './heatmap-shared.js'
import {
  PmtilesManifestPinMissingError,
  readCachedValidatedPmtilesManifest,
  type PmtilesManifest,
} from '../runtime-readiness.js'
import type { PublishedLineModel } from '../published-line-model.js'

type PublicManifestLayer = { file: string; build?: string }
type PublicTierPack = { pack: string; coverage_r4: string[]; layers: string[] }

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
  const tiers: Record<string, { packs: PublicTierPack[] }> = {}
  if (manifest.tiers && typeof manifest.tiers === 'object' && !Array.isArray(manifest.tiers)) {
    for (const [zoom, value] of Object.entries(
      manifest.tiers as Record<string, { packs?: unknown }>,
    )) {
      if (!Array.isArray(value?.packs)) continue
      tiers[zoom] = {
        packs: value.packs.map((pack) => {
          const entry = pack as Record<string, unknown>
          return {
            pack: entry.pack as string,
            coverage_r4: [...entry.coverage_r4 as string[]],
            layers: [...entry.layers as string[]],
          }
        }),
      }
    }
  }
  return {
    build: manifest.build,
    layers,
    ...(Object.keys(tiers).length > 0 ? { tiers } : {}),
  }
}

/**
 * Serve `current.{TILE_ENV}.json` (docs/dev/checkout-restructure-plan.md Track 2 — per-env
 * pmtiles pins, resolved by `tile-manifest-reader.ts`), which the Rust packer's fan-out /
 * `worldctl promote` write atomically. The shared boot-readiness validator rejects a torn,
 * malformed, or semantically invalid manifest with a 500; this route then projects only the
 * fields needed to fetch tiles. Internal generation, model-role, quality, scorer, hash, and
 * publisher-proof data never crosses the public boundary. ONE deployment field is added on
 * top: `tile_base` (env PUBLIC_TILE_BASE) tells
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
  app.get('/api/tiles-manifest', async (_req, reply) => {
    reply.header('Cache-Control', 'no-cache')
    if (options.publishedLineModel?.enabled) {
      const published = options.publishedLineModel.mapManifest()
      if (!published) {
        return reply.code(503).send({ error: 'published line model unavailable' })
      }
      return { ...publicManifest(published), tile_base: tileBase }
    }
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
