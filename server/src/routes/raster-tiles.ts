/** Raster overlays: building-height footprints plus the native-lattice grid
 * channels (DEM elevation, forest cover, imperviousness). */

import type { FastifyInstance } from 'fastify'
import { renderBuildingVectorTile, type QueryObstacleFootprints } from '../engine/raster-tile-renderer.js'
import { renderGridTile, type GridChannel } from '../engine/raster-grid-renderer.js'
import { PREPARED_YEAR_DIR } from '../runtime-paths.js'

const CACHE_MAX = 500

const GRID_ZOOM: Record<GridChannel, [number, number]> = {
  dem: [6, 16],
  forest: [8, 16],
  imd: [10, 16],
}

export async function rasterTileRoutes(
  app: FastifyInstance,
  options: { queryObstacleFootprints: QueryObstacleFootprints },
): Promise<void> {
  // App-local: a second app/provider must never inherit another release's PNGs.
  const cache = new Map<string, Buffer>()
  const cached = async (key: string, render: () => Promise<Buffer>): Promise<Buffer> => {
    const hit = cache.get(key)
    if (hit) {
      cache.delete(key)
      cache.set(key, hit)
      return hit
    }
    const png = await render()
    cache.delete(key)
    cache.set(key, png)
    while (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value!)
    return png
  }
  app.get<{ Params: { z: string; x: string; y: string } }>(
    '/api/raster/building/:z/:x/:y.png',
    async (request, reply) => {
      const z = Number(request.params.z)
      const x = Number(request.params.x)
      const y = Number(request.params.y)
      // The existing frontend displays building heights only at zoom 10–16.
      if (!Number.isInteger(z) || z < 10 || z > 16) return reply.code(400).send('Invalid zoom')
      const axis = 2 ** z
      if (!Number.isInteger(x) || x < 0 || x >= axis || !Number.isInteger(y) || y < 0 || y >= axis) {
        return reply.code(400).send('Invalid coordinates')
      }
      const png = await cached(`building/${z}/${x}/${y}`, () =>
        renderBuildingVectorTile(z, x, y, options.queryObstacleFootprints))
      // Attach public caching only after successful rendering. A 5xx must not
      // become a cached transparent tile or a cached outage.
      return reply.type('image/png').header('Cache-Control', 'public, max-age=3600').send(png)
    },
  )
  for (const channel of Object.keys(GRID_ZOOM) as GridChannel[]) {
    const [zMin, zMax] = GRID_ZOOM[channel]
    app.get<{ Params: { z: string; x: string; y: string } }>(
      `/api/raster/${channel}/:z/:x/:y.png`,
      async (request, reply) => {
        const z = Number(request.params.z)
        const x = Number(request.params.x)
        const y = Number(request.params.y)
        if (!Number.isInteger(z) || z < zMin || z > zMax) return reply.code(400).send('Invalid zoom')
        const axis = 2 ** z
        if (!Number.isInteger(x) || x < 0 || x >= axis || !Number.isInteger(y) || y < 0 || y >= axis) {
          return reply.code(400).send('Invalid coordinates')
        }
        const png = await cached(`${channel}/${z}/${x}/${y}`, () =>
          renderGridTile(PREPARED_YEAR_DIR, channel, z, x, y))
        return reply.type('image/png').header('Cache-Control', 'public, max-age=3600').send(png)
      },
    )
  }
}
