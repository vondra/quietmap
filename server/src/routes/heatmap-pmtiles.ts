// Versioned heatmap tile route — serves HM3 tiles out of per-layer pmtiles
// archives (`{layer}.{build}.pmtiles` in the tiles directory). A build ("b546", …)
// is an immutable published generation, so both hits and misses are cached hard
// by the browser.

import { constants } from 'node:fs'
import { open } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'
import { gunzip } from 'node:zlib'
import type { FastifyInstance } from 'fastify'
import {
  Compression,
  PMTiles,
  SharedPromiseCache,
  type RangeResponse,
  type Source,
} from 'pmtiles'
import { parseTileParams } from './heatmap-shared.ts'

const gunzipAsync = promisify(gunzip)

const BUILD_ID = /^b\d+$/

// Retain parsed directories for up to four generations of the eight layers.
const ARCHIVE_CACHE_MAX = 32

/** Cache parsed directories, but retain an archive descriptor only during a range read:
 * an idle cached descriptor would pin hundreds of GB after publication GC unlinks it. */
class FileRangeSource implements Source {
  private readonly path: string

  constructor(path: string) {
    this.path = path
  }

  getKey(): string {
    return this.path
  }

  async getBytes(offset: number, length: number): Promise<RangeResponse> {
    const handle = await open(
      this.path,
      constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
    )
    try {
      if (!(await handle.stat()).isFile()) throw new Error(`${this.path} is not a regular file`)
      // Positional reads may be short; only EOF ends the range early. The initial
      // header probe asks for 16 KiB even when an empty archive is smaller.
      const data = new ArrayBuffer(length)
      const view = new Uint8Array(data)
      let filled = 0
      while (filled < length) {
        const { bytesRead } = await handle.read(view, filled, length - filled, offset + filled)
        if (bytesRead === 0) break
        filled += bytesRead
      }
      return filled === length ? { data } : { data: data.slice(0, filled) }
    } finally {
      await handle.close()
    }
  }
}

/**
 * Decompressor for our archives: gunzip directories, pass tile bytes through.
 *
 * The pmtiles lib runs this over BOTH internal directories (internalCompression,
 * gzip — the packer's default) and tile entries (tileCompression). Tile bytes
 * are whole-file-Brotli HM3 that the route ships verbatim with
 * `Content-Encoding: br` (browser inflates in its network stack; server stays a
 * dumb byte reader), so Brotli passthrough here is the point, not an omission.
 * `openHeatmapArchive` rejects any archive whose header contradicts these
 * assumptions instead of ever serving corrupt bytes.
 */
async function gunzipDirectoriesPassthroughTiles(
  buf: ArrayBuffer,
  compression: Compression,
): Promise<ArrayBuffer> {
  if (compression === Compression.Gzip) {
    const out = await gunzipAsync(buf)
    const copy = new ArrayBuffer(out.byteLength)
    new Uint8Array(copy).set(out)
    return copy
  }
  if (compression === Compression.Zstd) throw new Error('zstd archives not supported')
  return buf // None | Unknown | Brotli → verbatim
}

async function openHeatmapArchive(path: string): Promise<PMTiles> {
  const pmtiles = new PMTiles(
    new FileRangeSource(path),
    new SharedPromiseCache(100, true, gunzipDirectoriesPassthroughTiles),
    gunzipDirectoriesPassthroughTiles,
  )
  const header = await pmtiles.getHeader()
  if (header.internalCompression !== Compression.Gzip
    && header.internalCompression !== Compression.None) {
    throw new Error(`unsupported internal compression ${header.internalCompression}`)
  }
  // The route declares Brotli without re-encoding, so a different header is unsafe.
  if (header.tileCompression !== Compression.Brotli) {
    throw new Error(`tile compression ${header.tileCompression} contradicts verbatim-Brotli serving`)
  }
  return pmtiles
}

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
      entry = openHeatmapArchive(join(tilesDir, `${layer}.${build}.pmtiles`))
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
