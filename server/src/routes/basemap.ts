// The standard base map, served from one directory: OpenStreetMap vector tiles in a Protomaps
// planet archive, the fonts of its labels and the sprites of its icons.

import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import type { FastifyInstance, FastifyReply } from 'fastify'
import { Compression, TileType, type PMTiles, type RangeResponse } from 'pmtiles'
import { openArchiveServedAsStored } from './pmtiles-file.ts'

/** The archive is replaced by a newer planet under the same name, so nothing is kept longer. */
const CACHE_A_DAY = 'public, max-age=86400'

// A planet's leaf directory names some 60,000 tiles and takes 6 MB of heap once parsed (measured on
// the planet of 2026-11-17: 2,917 leaves for 179 million entries); each zoom of a view reads one.
const PLANET_DIRECTORIES_MAX = 48

/** The deepest zoom a tile address may name; the archive answers no tile beyond its own. */
const MAX_ZOOM = 22

const FONTSTACK = /^[A-Za-z0-9 ]+$/
const GLYPH_RANGE = /^\d{1,5}-\d{1,5}$/
const SPRITE_FILE = /^light(?:@2x)?\.(json|png)$/

async function sendFile(reply: FastifyReply, path: string, contentType: string): Promise<FastifyReply> {
  let body: Buffer
  try {
    body = await readFile(path)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return reply.code(404).send('no such file')
    throw error
  }
  return reply.header('Cache-Control', CACHE_A_DAY).type(contentType).send(body)
}

/**
 * `basemapDir` holds `planet.pmtiles`, `fonts/<fontstack>/<range>.pbf` and `sprites/<file>`;
 * without it the routes do not exist, and the map draws its noise on blank paper.
 */
export async function basemapRoutes(app: FastifyInstance, { basemapDir }: { basemapDir: string | null }): Promise<void> {
  if (!basemapDir) return
  let archive: Promise<PMTiles> | undefined

  async function readTile(z: number, x: number, y: number): Promise<RangeResponse | undefined> {
    const opened = archive ??= openArchiveServedAsStored(join(basemapDir!, 'planet.pmtiles'), Compression.Gzip, PLANET_DIRECTORIES_MAX)
      .then(async planet => {
        // The route names the tiles vector tiles to the browser: an archive of anything else is not the base map.
        const { tileType } = await planet.getHeader()
        if (tileType !== TileType.Mvt) throw new Error(`planet.pmtiles holds tiles of type ${tileType}, not vector tiles`)
        return planet
      })
    try {
      return await (await opened).getZxy(z, x, y)
    } catch (error) {
      // The lib's cache retains a rejected directory read; retry with a fresh instance.
      if (archive === opened) archive = undefined
      throw error
    }
  }

  app.get<{ Params: { z: string; x: string; y: string } }>(
    '/api/basemap/:z/:x/:y.mvt',
    // The tiles are stored gzip-compressed and sent as they are.
    { compress: false },
    async (req, reply) => {
      const [z, x, y] = [req.params.z, req.params.x, req.params.y].map(part => (/^\d{1,8}$/.test(part) ? Number(part) : NaN))
      if (!(z <= MAX_ZOOM && x < 2 ** z && y < 2 ** z)) return reply.code(400).send('no such tile address')
      let tile: RangeResponse | undefined
      try {
        tile = await readTile(z, x, y)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'ENOENT') return reply.code(404).send('no base map archive')
        app.log.error(`basemap read ${z}/${x}/${y}: ${(error as Error).message}`)
        return reply.code(500).send('archive read failed')
      }
      reply.header('Cache-Control', CACHE_A_DAY)
      if (tile === undefined) return reply.code(204).send()
      return reply
        .header('Content-Type', 'application/x-protobuf')
        .header('Content-Encoding', 'gzip')
        .send(Buffer.from(tile.data))
    },
  )

  app.get<{ Params: { fontstack: string; range: string } }>(
    '/api/basemap/fonts/:fontstack/:range.pbf',
    // A range of glyphs is 76 kB of bitmaps and 36 kB compressed; protobuf is not compressed by default.
    { compress: { customTypes: /^application\/x-protobuf$/ } },
    async (req, reply) => {
      const { fontstack, range } = req.params
      if (!FONTSTACK.test(fontstack) || !GLYPH_RANGE.test(range)) return reply.code(404).send('no such file')
      return sendFile(reply, join(basemapDir, 'fonts', fontstack, `${range}.pbf`), 'application/x-protobuf')
    },
  )

  app.get<{ Params: { file: string } }>('/api/basemap/sprites/:file', async (req, reply) => {
    const kind = req.params.file.match(SPRITE_FILE)?.[1]
    if (!kind) return reply.code(404).send('no such file')
    return sendFile(reply, join(basemapDir, 'sprites', req.params.file), kind === 'png' ? 'image/png' : 'application/json')
  })
}
