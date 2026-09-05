/** Serve only building-height footprints; acoustic heatmaps are not registered. */

import type { FastifyInstance } from 'fastify'
import { renderBuildingVectorTile, type QueryObstacleFootprints } from '../engine/raster-tile-renderer.js'

const CACHE_MAX = 500

export async function rasterTileRoutes(
  app: FastifyInstance,
  options: { queryObstacleFootprints: QueryObstacleFootprints },
): Promise<void> {
  // App-local: a second app/provider must never inherit another release's PNGs.
  const cache = new Map<string, Buffer>()
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
      const key = `${z}/${x}/${y}`
      const png = cache.get(key) ?? await renderBuildingVectorTile(z, x, y, options.queryObstacleFootprints)
      cache.delete(key)
      cache.set(key, png)
      while (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value!)
      // Attach public caching only after successful rendering. A 5xx must not
      // become a cached transparent tile or a cached outage.
      return reply.type('image/png').header('Cache-Control', 'public, max-age=3600').send(png)
    },
  )
}
