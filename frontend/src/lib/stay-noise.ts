import { fetchAndDecodeHM3, TILE_PX, NO_DATA } from './hm3-decoder'
import { lngLatToTileFloat } from './tile-math'
import { tileUrl, WORLD_EXTENT, type TileBuilds } from './tile-urls'

// Sample the computed total Lden at arbitrary points by reading the same
// precomputed HM3 tiles the heatmap renders — no server round-trip per point,
// and the browser's HTTP cache already holds most tiles for the viewport.
// One tile covers 512×512 cells, so a screenful of markers usually costs
// zero to two extra fetches. Always samples at the published base zoom: this is
// a value readout at the raster's native resolution, not a rendered-pixel match
// (unlike HoverTooltip, which reads the pyramid level actually displayed).

const tileCache = new Map<string, Promise<Uint8Array | null>>()
const TILE_CACHE_MAX = 64

function getTileCells(build: TileBuilds, x: number, y: number): Promise<Uint8Array | null> {
  const url = tileUrl(build, 'total', build.zoom, x, y)
  let p = tileCache.get(url)
  if (!p) {
    p = fetchAndDecodeHM3(url)
      .then(d => (d ? d.cells : null))
      // An empty tile is a cacheable null; a fetch/decode ERROR must not be —
      // it would pin "no data" for the tile until eviction.
      .catch(() => { tileCache.delete(url); return null })
    tileCache.set(url, p)
    while (tileCache.size > TILE_CACHE_MAX) tileCache.delete(tileCache.keys().next().value!)
  }
  return p
}

/** Total Lden (dB) at each point, `null` where the world has no computed value. */
export async function sampleNoiseAt(
  build: TileBuilds,
  points: { lat: number; lng: number }[],
): Promise<(number | null)[]> {
  const n = 1 << build.zoom
  return Promise.all(points.map(async ({ lat, lng }) => {
    if (!Number.isFinite(lat) || Math.abs(lat) > WORLD_EXTENT[3]) return null
    const [xf, yf] = lngLatToTileFloat(lng, lat, build.zoom)
    const tx = Math.floor(xf)
    const ty = Math.floor(yf)
    if (ty < 0 || ty >= n) return null
    const cells = await getTileCells(build, ((tx % n) + n) % n, ty)
    if (!cells) return null
    const px = Math.min(TILE_PX - 1, Math.floor((xf - tx) * TILE_PX))
    const py = Math.min(TILE_PX - 1, Math.floor((yf - ty) * TILE_PX))
    const byte = cells[py * TILE_PX + px]
    return byte === NO_DATA ? null : byte / 2 // HM3 encodes dB × 2
  }))
}
