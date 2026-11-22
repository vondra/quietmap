// Serve the stored HM3 Brotli streams by range from immutable per-layer PMTiles archives.

import { join } from 'node:path'
import type { FastifyInstance } from 'fastify'
import { Compression, type PMTiles, type RangeResponse } from 'pmtiles'
import { parseTileParams } from './heatmap-shared.ts'
import { openArchiveServedAsStored } from './pmtiles-file.ts'

const BUILD_ID = /^b\d+$/

// Retain parsed directories for up to 32 immutable layer archives, 100 directories each.
const ARCHIVE_CACHE_MAX = 32
const ARCHIVE_DIRECTORIES_MAX = 100

/**
 * GET /api/tiles/:build/:layer/:z/:x/:y.bin
 *
 * Raw HM3 bytes, whole-file Brotli, `Content-Encoding: br`, addressed
 * inside an immutable build: hits AND misses get `max-age=31536000,
 * immutable` — for a published generation a missing tile is a permanent fact
 * (every cell not assessed, served as 200 + empty body so a CDN caches it),
 * and the frontend re-keys the URL on the next build anyway.
 */
export async function heatmapPmtilesRoutes(app: FastifyInstance, { tilesDir }: { tilesDir: string }): Promise<void> {
  // One PMTiles instance per (build, layer) retains parsed headers and directories.
  const archiveCache = new Map<string, Promise<PMTiles>>()

  async function readHeatmapTile(
    build: string, layer: string, z: number, x: number, y: number,
  ): Promise<RangeResponse | undefined> {
    const key = `${build}/${layer}`
    let entry = archiveCache.get(key)
    if (!entry) {
      entry = openArchiveServedAsStored(join(tilesDir, `${layer}.${build}.pmtiles`), Compression.Brotli, ARCHIVE_DIRECTORIES_MAX)
      archiveCache.set(key, entry)
      if (archiveCache.size > ARCHIVE_CACHE_MAX) {
        const oldestKey = archiveCache.keys().next().value
        if (oldestKey !== undefined) archiveCache.delete(oldestKey)
      }
    }
    try {
      return await (await entry).getZxy(z, x, y)
    } catch (error) {
      // SharedPromiseCache retains rejected leaf reads; retry with a fresh instance.
      if (archiveCache.get(key) === entry) archiveCache.delete(key)
      throw error
    }
  }

  app.get<{ Params: { build: string; layer: string; z: string; x: string; y: string } }>(
    '/api/tiles/:build/:layer/:z/:x/:y.bin',
    // Bodies are already Brotli (served with Content-Encoding: br) — opt out of
    // @fastify/compress so it doesn't re-compress incompressible bytes.
    // CORS on EVERY response (hits, misses, errors): tiles are public,
    // immutable, cookie-less bytes — `*` is the correct scope.
    {
      compress: false,
      onSend: async (_req, reply, payload) => {
        reply.header('Access-Control-Allow-Origin', '*')
        // Without this the browser hides the detailed cross-origin resource
        // timing (phase breakdown, transferSize; total duration stays visible).
        reply.header('Timing-Allow-Origin', '*')
        return payload
      },
    },
    async (req, reply) => {
      if (!BUILD_ID.test(req.params.build)) return reply.code(404).send('unknown build')
      const parsed = parseTileParams(req.params)
      if (typeof parsed === 'string') return reply.code(400).send(parsed)
      const { build } = req.params
      const { layer, z, x, y } = parsed

      let tile: RangeResponse | undefined
      try {
        tile = await readHeatmapTile(build, layer, z, x, y)
      } catch (e) {
        if ((e as NodeJS.ErrnoException).code === 'ENOENT') {
          return reply.code(404).send('no such archive')
        }
        reply.header('Cache-Control', 'no-store')
        app.log?.error?.(`heatmap-pmtiles read ${layer}.${build}/${z}/${x}/${y}: ${(e as Error).message}`)
        return reply.code(500).send('archive read failed')
      }

      // Published generations are immutable → cache the miss as hard as the hit.
      reply.header('Cache-Control', 'public, max-age=31536000, immutable')
      // "No tile" is 200 + empty body, NOT 204: CDNs do not cache 204s by
      // default, so every unpainted tile would round-trip to the origin for
      // every visitor forever. The decoder maps an empty body to a tile whose
      // every cell is not assessed — never to silence.
      if (tile === undefined) return reply.send(Buffer.alloc(0))
      // application/octet-stream — the body is a custom binary format (HM3),
      // stored as a whole-file Brotli stream; declare it so the browser
      // decompresses natively. Set AFTER the empty-body return so a miss
      // carries no encoding (an empty stream is not valid Brotli).
      reply.header('Content-Type', 'application/octet-stream')
      reply.header('Content-Encoding', 'br')
      return reply.send(Buffer.from(tile.data))
    },
  )
}
