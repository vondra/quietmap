/** Cached heatmap cell readouts from the same grid and byte decoding as rendering. */
import { cellLevel, HM3_BYTE_SONE, HM3_NO_LEVEL, HM3_SILENCE, TILE_PX } from './hm3-decoder.ts'
import { fetchSourceTile, type SourceTile } from './hm3-combine.ts'
import { lngLatToTileFloat } from './tile-math.ts'
import { MIN_ZOOM, WORLD_EXTENT, type TileBuilds } from './tile-urls.ts'

/** One pyramid level finer on HiDPI screens, where one data cell ≈ one device
 *  pixel wherever a finer level exists — the renderer's TileLayer `zoomOffset`. */
export function hiDpiTileZoomOffset(devicePixelRatio: number): number {
  return devicePixelRatio >= 1.5 ? 1 : 0
}

/**
 * The tile zoom the heatmap paints at a viewport zoom — deck's TileLayer rule
 * for HeatmapOverlay's layer: round the viewport zoom, add the HiDPI offset,
 * clamp into the published band (deck under-zooms to MIN_ZOOM with a world
 * extent and never requests past the base zoom).
 */
export function displayedTileZoom(build: TileBuilds, viewZoom: number, devicePixelRatio: number): number {
  return Math.min(build.zoom, Math.max(MIN_ZOOM, Math.round(viewZoom) + hiDpiTileZoomOffset(devicePixelRatio)))
}

/** One HM3 cell: the tile index at zoom `z` and the cell inside it. */
export interface HM3CellAddress {
  z: number
  tx: number
  ty: number
  px: number
  py: number
}

/**
 * The tile + cell holding (lng, lat) at zoom `z`, with the same floor math the
 * renderer uses, so the byte read is the byte painted. `null` off the Mercator
 * world (polar latitudes); x wraps across the antimeridian.
 */
export function hm3CellAt(lng: number, lat: number, z: number): HM3CellAddress | null {
  if (!Number.isFinite(lat) || !Number.isFinite(lng) || Math.abs(lat) > WORLD_EXTENT[3]) return null
  const span = 2 ** z
  const [xFloat, yFloat] = lngLatToTileFloat(lng, lat, z)
  const tx = Math.floor(xFloat)
  const ty = Math.floor(yFloat)
  if (ty < 0 || ty >= span) return null
  return {
    z,
    tx: ((tx % span) + span) % span,
    ty,
    px: Math.min(TILE_PX - 1, Math.floor((xFloat - tx) * TILE_PX)),
    py: Math.min(TILE_PX - 1, Math.floor((yFloat - ty) * TILE_PX)),
  }
}

/** One layer's tile at a readout: its cells, absent from the archive (every
 *  cell without a level), or failed to load (level unknown, retried later). */
export type SampledTile = SourceTile | 'absent' | 'failed'

// LRU keyed by the full tile URL — the URL carries the tile build, so a
// mid-session generation flip re-keys the cache by itself. Each entry is
// ~256 KiB of cells; 64 ≈ 16 MiB, a few screenfuls of sample tiles.
type TileCellsEntry = { promise: Promise<SampledTile>; done: boolean }
const tileCellsCache = new Map<string, TileCellsEntry>()
const TILE_CELLS_CACHE_MAX = 64
// A failed tile is forgotten after this, so a transient error is retried
// instead of pinning "unavailable" for the session.
const FAILED_TILE_RETRY_MS = 30_000

/**
 * Decoded cells of one tile. Callers hold the returned array, so an eviction
 * never takes a sample away from a readout already using it. 'low' fetch
 * priority keeps the heatmap's own sharp tiles ahead in the queue.
 */
export function tileCells(url: string): Promise<SampledTile> {
  const hit = tileCellsCache.get(url)
  if (hit) {
    tileCellsCache.delete(url) // refresh recency
    tileCellsCache.set(url, hit)
    return hit.promise
  }
  const entry: TileCellsEntry = {
    promise: fetchSourceTile(url, undefined, 'low')
      .then((decoded): SampledTile => decoded ?? 'absent')
      .catch((): SampledTile => {
        setTimeout(() => { if (tileCellsCache.get(url) === entry) tileCellsCache.delete(url) }, FAILED_TILE_RETRY_MS)
        return 'failed'
      })
      .then((tile) => { entry.done = true; evictSettledTilesBeyondLimit(); return tile }),
    done: false,
  }
  tileCellsCache.set(url, entry)
  evictSettledTilesBeyondLimit()
  return entry.promise
}

// Oldest SETTLED entries go first — never an in-flight promise a reader is
// about to share; a burst of pending fetches may overshoot the limit until
// they settle, which is why every settle trims again.
function evictSettledTilesBeyondLimit(): void {
  for (const [key, cached] of tileCellsCache) {
    if (tileCellsCache.size <= TILE_CELLS_CACHE_MAX) return
    if (cached.done) tileCellsCache.delete(key)
  }
}

/** What a cell reads: Nden in sone, Lden in dB. */
export type HeatmapCellReadout =
  | { kind: 'level'; sone: number }
  | { kind: 'lden'; db: number }
  | { kind: 'silence' }
  | { kind: 'no-level' }
  | { kind: 'unavailable' }

/**
 * The level at one cell of the shown source's tile, in its quantity: the byte the renderer
 * painted. A failed tile reads unavailable (its level is unknown), an absent one has no level; a
 * cell assessed under the quantity's floor (1/16 sone, 0 dB) is silence.
 */
export function readHeatmapCell(tile: SampledTile, cell: HM3CellAddress): HeatmapCellReadout {
  if (tile === 'failed') return { kind: 'unavailable' }
  const byte = tile === 'absent' ? HM3_NO_LEVEL : tile.cells[cell.py * TILE_PX + cell.px]
  if (byte === HM3_NO_LEVEL) return { kind: 'no-level' }
  if (byte === HM3_SILENCE) return { kind: 'silence' }
  return (tile as SourceTile).metric === 'lden'
    ? { kind: 'lden', db: cellLevel(byte) }
    : { kind: 'level', sone: HM3_BYTE_SONE[byte] }
}
