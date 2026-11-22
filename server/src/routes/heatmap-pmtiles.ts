// Serve the stored HM3 Brotli streams by range from immutable per-layer PMTiles archives.

import { stat } from 'node:fs/promises'
import { join } from 'node:path'
import type { FastifyInstance } from 'fastify'
import { Compression, type RangeResponse } from 'pmtiles'
import { requireTilesDirYear } from './heatmap-manifest.ts'
import { parseTileParams } from './heatmap-shared.ts'
import { ArchiveChangedError, openVerifiedArchive, type VerifiedArchive } from './pmtiles-file.ts'

const BUILD_ID = /^b\d+$/
const YEAR_ID = /^\d{4}$/

// Retain parsed directories for up to 32 immutable layer archives, 100 directories each.
const ARCHIVE_CACHE_MAX = 32
const ARCHIVE_DIRECTORIES_MAX = 100

interface CachedArchive {
  archive: VerifiedArchive
  yearMarker: { dev: number; ino: number; mtimeMs: number; size: number }
}

/**
 * GET /api/tiles/:year/:build/:layer/:z/:x/:y.bin
 *
 * Raw HM3 bytes, whole-file Brotli, `Content-Encoding: br`, addressed
 * inside an immutable year and build: hits AND misses get `max-age=31536000,
 * immutable` — for a published generation a missing tile is a permanent fact
 * (every cell not assessed, served as 200 + empty body so a CDN caches it),
 * and the frontend re-keys the URL on the next build or map year anyway.
 */
export async function heatmapPmtilesRoutes(
  app: FastifyInstance,
  { tilesDir, expectedYear }: { tilesDir: string; expectedYear: string },
): Promise<void> {
  // One pinned PMTiles instance per (year, build, layer) retains parsed headers and directories.
  const archiveCache = new Map<string, Promise<CachedArchive>>()
  const yearPath = join(tilesDir, 'year')

  function openEntry(key: string, build: string, layer: string): Promise<CachedArchive> {
    const entry = (async (): Promise<CachedArchive> => {
      const archive = await openVerifiedArchive(
        join(tilesDir, `${layer}.${build}.pmtiles`),
        Compression.Brotli,
        ARCHIVE_DIRECTORIES_MAX,
        expectedYear,
      )
      await requireTilesDirYear(tilesDir, expectedYear)
      const markerStat = await stat(yearPath)
      return {
        archive,
        yearMarker: {
          dev: markerStat.dev,
          ino: markerStat.ino,
          mtimeMs: markerStat.mtimeMs,
          size: markerStat.size,
        },
      }
    })()
    archiveCache.set(key, entry)
    if (archiveCache.size > ARCHIVE_CACHE_MAX) {
      const oldestKey = archiveCache.keys().next().value
      if (oldestKey !== undefined) archiveCache.delete(oldestKey)
    }
    return entry
  }

  async function verifyYearMarker(expected: CachedArchive['yearMarker']): Promise<void> {
    let current
    try {
      current = await stat(yearPath)
    } catch {
      throw new ArchiveChangedError(`${yearPath} changed or disappeared`)
    }
    if (
      current.dev !== expected.dev ||
      current.ino !== expected.ino ||
      current.mtimeMs !== expected.mtimeMs ||
      current.size !== expected.size
    ) {
      throw new ArchiveChangedError(`${yearPath} was modified after validation`)
    }
  }

  async function readHeatmapTile(
    year: string, build: string, layer: string, z: number, x: number, y: number,
  ): Promise<RangeResponse | undefined> {
    const key = `${year}/${build}/${layer}`
    for (let attempt = 0; attempt < 2; attempt++) {
      let entry = archiveCache.get(key)
      const reused = entry !== undefined
      if (!entry) entry = openEntry(key, build, layer)
      try {
        const cached = await entry
        // Check archive inode and directory year marker even on directory-cache misses so an
        // in-place swap can never serve foreign bytes or an immutable empty miss.
        await cached.archive.verifyIdentity()
        await verifyYearMarker(cached.yearMarker)
        return await cached.archive.pmtiles.getZxy(z, x, y)
      } catch (error) {
        // SharedPromiseCache retains rejected leaf reads; evict before propagating or retrying once.
        if (archiveCache.get(key) === entry) archiveCache.delete(key)
        if (reused && error instanceof ArchiveChangedError) continue
        throw error
      }
    }
    throw new Error('unreachable')
  }

  app.get<{ Params: { year: string; build: string; layer: string; z: string; x: string; y: string } }>(
    '/api/tiles/:year/:build/:layer/:z/:x/:y.bin',
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
      if (!YEAR_ID.test(req.params.year) || req.params.year !== expectedYear) {
        reply.header('Cache-Control', 'no-store')
        return reply.code(404).send('unknown year')
      }
      if (!BUILD_ID.test(req.params.build)) return reply.code(404).send('unknown build')
      const parsed = parseTileParams(req.params)
      if (typeof parsed === 'string') return reply.code(400).send(parsed)
      const { year, build } = req.params
      const { layer, z, x, y } = parsed

      let tile: RangeResponse | undefined
      try {
        tile = await readHeatmapTile(year, build, layer, z, x, y)
      } catch (e) {
        if ((e as NodeJS.ErrnoException).code === 'ENOENT') {
          return reply.code(404).send('no such archive')
        }
        reply.header('Cache-Control', 'no-store')
        app.log?.error?.(`heatmap-pmtiles read ${year}/${layer}.${build}/${z}/${x}/${y}: ${(e as Error).message}`)
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
