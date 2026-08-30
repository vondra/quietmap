import type { FastifyInstance } from 'fastify'
import { renderTile, renderDataTile, renderBuildingVectorTile, preloadBarriers } from '../engine/raster-tile-renderer.js'

const VALID_LAYERS = new Set(['dem', 'building', 'forest', 'barriers'])
const VALID_DATA_LAYERS = new Set(['dem', 'forest'])
const MIN_ZOOM = 6
const MAX_ZOOM = 16
const CACHE_MAX = 500

// Map insertion order = LRU order; `delete + set` promotes to newest in O(1).
const pngCache = new Map<string, Buffer>()
const dataCache = new Map<string, Buffer>()

function lruGet(cache: Map<string, Buffer>, key: string): Buffer | undefined {
  const v = cache.get(key)
  if (v === undefined) return undefined
  cache.delete(key)
  cache.set(key, v)
  return v
}

function lruSet(cache: Map<string, Buffer>, key: string, value: Buffer): void {
  cache.set(key, value)
  while (cache.size > CACHE_MAX) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    cache.delete(oldest)
  }
}

function parseTileParams(
  params: { layer: string; z: string; x: string; y: string },
  validLayers: Set<string>,
): { layer: string; z: number; x: number; y: number } | string {
  const { layer, z: zStr, x: xStr, y: yStr } = params
  if (!validLayers.has(layer)) return 'Invalid layer'
  const z = Number(zStr); const x = Number(xStr); const y = Number(yStr)
  if (!Number.isInteger(z) || z < MIN_ZOOM || z > MAX_ZOOM) return 'Invalid zoom'
  const max = 2 ** z
  if (!Number.isInteger(x) || x < 0 || x >= max || !Number.isInteger(y) || y < 0 || y >= max) {
    return 'Invalid coordinates'
  }
  return { layer, z, x, y }
}

export type RasterTileRouteOptions = {
  preloadRuntimeData?: boolean
  /** Popup-engine footprint provider — the ONLY source of the `building`
   *  layer (MODEL-TRUTH vector footprints at as-used heights, incl. the
   *  low-profile cap). Required: without it the layer has no data at all,
   *  and a route that quietly served nothing would be worse than no route. */
  queryObstacleFootprints: (south: number, west: number, north: number, east: number) => Promise<string>
}

export async function rasterTileRoutes(
  app: FastifyInstance,
  options: RasterTileRouteOptions,
): Promise<void> {
  if (options.preloadRuntimeData) {
    preloadBarriers().catch(err => app.log.error(err, 'barrier preload failed'))
  }

  app.get<{ Params: { layer: string; z: string; x: string; y: string } }>(
    '/api/raster/:layer/:z/:x/:y.png',
    async (request, reply) => {
      const parsed = parseTileParams(request.params, VALID_LAYERS)
      if (typeof parsed === 'string') return reply.code(400).send(parsed)
      const { layer, z, x, y } = parsed

      const cacheKey = `${layer}/${z}/${x}/${y}`
      // Headers are attached only on the success paths: the error handler
      // reuses this reply, and a 5xx carrying `public, max-age=3600` would let
      // Caddy and every browser cache the outage as if it were a tile.
      const sendPng = (png: Buffer): Buffer => {
        reply.header('Content-Type', 'image/png')
        // Model-truth footprints change with the obstacle store + cap logic —
        // cache them shorter than the static rasters.
        reply.header('Cache-Control', layer === 'building' ? 'public, max-age=3600' : 'public, max-age=86400')
        return png
      }

      const cached = lruGet(pngCache, cacheKey)
      if (cached !== undefined) return sendPng(cached)

      // No catch: a render failure propagates to Fastify's 5xx handler. The
      // former fallback answered 200 with a transparent tile, which on a noise
      // map reads as "nothing here" — data loss disguised as a quiet place.
      const png = layer === 'building'
        ? await renderBuildingVectorTile(z, x, y, options.queryObstacleFootprints)
        : await renderTile(layer as 'dem' | 'forest' | 'barriers', z, x, y)
      lruSet(pngCache, cacheKey, png)
      return sendPng(png)
    },
  )

  // Raw cell-value tiles for the hover cell inspector. Same z/x/y grid as
  // the PNG endpoint; payload is Int16 (DEM) / u8 (forest) so the client can
  // look up per-cell values locally without per-hover round-trips. Buildings
  // have no entry here: their heights are vector footprints, not a cell grid.
  app.get<{ Params: { layer: string; z: string; x: string; y: string } }>(
    '/api/raster-data/:layer/:z/:x/:y.bin',
    async (request, reply) => {
      const parsed = parseTileParams(request.params, VALID_DATA_LAYERS)
      if (typeof parsed === 'string') return reply.code(400).send(parsed)
      const { layer, z, x, y } = parsed

      const cacheKey = `${layer}/${z}/${x}/${y}`
      reply.header('Content-Type', 'application/octet-stream')
      reply.header('Cache-Control', 'public, max-age=86400')

      const cached = lruGet(dataCache, cacheKey)
      if (cached !== undefined) return cached

      const data = await renderDataTile(layer as 'dem' | 'forest', z, x, y)
      lruSet(dataCache, cacheKey, data)
      return data
    },
  )
}
