// Over-zoom composite stitching — the React/deck-independent half of
// HeatmapOverlay's over-zoom mode (see its overzoomFrom): fetch the base-zoom tiles under the
// viewport, energy-sum + palette-map them into ONE seamless ImageData (no
// internal tile borders when magnified). Extracted verbatim from
// HeatmapOverlay.tsx. Sources are
// plain strings here — importing the component's HeatmapSource union back
// would create a type cycle (buildKey/tileUrl accept strings anyway).

import { fetchAndDecodeHM3, TILE_PX, NO_DATA } from './hm3-decoder'
import { composeOffThread } from './compose-off-thread'
import { lngLatToTileFloat, tileXToLng, tileYToLat } from './tile-math'
import { buildKey, tileUrl, type TileBuilds } from './tile-urls'

// One base tile of margin around the viewport so a small pan at deep zoom stays
// covered without an immediate rebuild (a base tile is magnified 2-8x here).
const COMPOSITE_MARGIN = 1
// Safety cap — never stitch a pathologically large composite (only trips if the
// over-zoom threshold is lowered toward normal zoom).
const MAX_COMPOSITE_TILES = 96

export type Range = { z: number; span: number; x0: number; x1: number; y0: number; y1: number; cols: number; rows: number }
type Bounds = [number, number, number, number]
export type Composite = { image: ImageData; bounds: Bounds }
type LngLatBounds = { getWest(): number; getEast(): number; getNorth(): number; getSouth(): number }

/** Identity of a built composite — tile build + source set + base tile range.
 *  `update` skips a rebuild while this is unchanged; `apply` paints the composite
 *  only while it still matches the live view (else the cached one is stale → fall
 *  back to tiles). The build snapshot makes a mid-session generation flip rebuild
 *  the composite instead of keeping stale-generation pixels. */
export function compositeSig(build: TileBuilds, sources: readonly string[], range: Range): string {
  return `${buildKey(build, sources)}|${[...sources].join(',')}|z${range.z}:${range.x0},${range.x1},${range.y0},${range.y1}`
}

/** The composite's tile range covering `bounds` (+ a 1-tile margin), clamped
 *  to the world. It stitches at the published base zoom — the finest level that
 *  exists — so over-zoom never paints a coarser image than the per-tile path it
 *  replaces. */
export function baseRange(bounds: LngLatBounds, build: TileBuilds): Range {
  const z = build.zoom
  const span = 2 ** z
  const [xWest, yNorth] = lngLatToTileFloat(bounds.getWest(), bounds.getNorth(), z)
  const [xEast, ySouth] = lngLatToTileFloat(bounds.getEast(), bounds.getSouth(), z)
  const x0 = Math.floor(xWest) - COMPOSITE_MARGIN
  const x1 = Math.floor(xEast) + COMPOSITE_MARGIN
  const y0 = Math.max(0, Math.floor(yNorth) - COMPOSITE_MARGIN)
  const y1 = Math.min(span - 1, Math.floor(ySouth) + COMPOSITE_MARGIN)
  return { z, span, x0, x1, y0, y1, cols: x1 - x0 + 1, rows: y1 - y0 + 1 }
}

/** Stitch the base-zoom tiles of `range` (energy-summed across sources, palette-mapped)
 *  into ONE seamless `ImageData` + its geo bounds. Null if nothing audible. All
 *  source×tile fetches run in one parallel batch. */
export async function buildComposite(
  range: Range,
  sources: readonly string[],
  build: TileBuilds,
  isStale: () => boolean,
): Promise<Composite | null> {
  const { z, span, x0, x1, y0, y1, cols, rows } = range
  if (cols < 1 || rows < 1 || cols * rows > MAX_COMPOSITE_TILES) return null
  const width = cols * TILE_PX
  const height = rows * TILE_PX
  const jobs: Array<{ source: string; tx: number; ty: number }> = []
  for (const source of sources) {
    for (let ty = y0; ty <= y1; ty++) {
      for (let tx = x0; tx <= x1; tx++) jobs.push({ source, tx, ty })
    }
  }
  const decoded = await Promise.all(
    jobs.map(({ source, tx, ty }) => {
      const wx = ((tx % span) + span) % span
      // 'low' priority: composite fetches carry no abort and a superseded
      // batch (up to 96×|sources| requests) must not outrank the fresh
      // view's sharp tiles.
      return fetchAndDecodeHM3(tileUrl(build, source, z, wx, ty), undefined, 'low')
        .catch(() => null)
    }),
  )
  // Bucket the landed tiles into one grid per source (allocated only on first hit).
  const gridBySource = new Map<string, Uint8Array>()
  decoded.forEach((d, i) => {
    if (!d?.cells) return
    const { source, tx, ty } = jobs[i]
    let grid = gridBySource.get(source)
    if (!grid) {
      grid = new Uint8Array(width * height).fill(NO_DATA)
      gridBySource.set(source, grid)
    }
    blit(d.cells, grid, (tx - x0) * TILE_PX, (ty - y0) * TILE_PX, width)
  })
  const grids = [...gridBySource.values()]
  if (grids.length === 0) return null
  // A pan/zoom during the fetches supersedes this composite — bail before the
  // big compose (up to 96 tiles) so stale work never queues ahead of fresh
  // tile composes in the single worker.
  if (isStale()) return null
  const image = await composeOffThread(grids, width, height)
  const bounds: Bounds = [tileXToLng(x0, z), tileYToLat(y1 + 1, z), tileXToLng(x1 + 1, z), tileYToLat(y0, z)]
  return { image, bounds }
}

/** Copy a TILE_PX×TILE_PX tile into `grid` at pixel offset (ox, oy), row by row. */
function blit(cells: Uint8Array, grid: Uint8Array, ox: number, oy: number, width: number) {
  for (let r = 0; r < TILE_PX; r++) {
    grid.set(cells.subarray(r * TILE_PX, (r + 1) * TILE_PX), (oy + r) * width + ox)
  }
}
