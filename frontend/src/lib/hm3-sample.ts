// Point readout of the HM3 heatmap tiles in the browser — the one decoded-tile
// cache and dB arithmetic behind every per-point sample (the hover readout and
// the stay pins at the displayed pyramid level, the stay card at the base
// zoom). No server round-trip per point: a tile covers 512×512 cells and the
// browser's HTTP cache already holds most tiles the heatmap painted.

import { fetchAndDecodeHM3, NO_DATA, TILE_PX } from './hm3-decoder.ts'
import { HM3_BYTE_ENERGY } from './hm3-compose.ts'
import { lngLatToTileFloat } from './tile-math.ts'
import { MIN_ZOOM, WORLD_EXTENT, tileUrl, type TileBuilds } from './tile-urls.ts'

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

// LRU keyed by the full tile URL — the URL carries the tile build, so a
// mid-session generation flip re-keys the cache by itself. Each entry is
// ~256 KiB of cells; 64 ≈ 16 MiB, a few screenfuls of sample tiles.
type TileCellsEntry = { promise: Promise<Uint8Array | null>; done: boolean }
const tileCellsCache = new Map<string, TileCellsEntry>()
const TILE_CELLS_CACHE_MAX = 64
// A tile whose fetch failed reads as silence (the renderer paints a failed
// tile blank too) and is forgotten after this, so a transient error is
// retried instead of pinning "no data" for the session.
const FAILED_TILE_RETRY_MS = 30_000

/**
 * Decoded cells of one tile (`null` = no tile: the world is silent there).
 * Callers hold the returned array, so an eviction never takes a sample away
 * from a readout already using it. 'low' fetch priority keeps the heatmap's
 * own sharp tiles ahead in the queue.
 */
export function tileCells(url: string): Promise<Uint8Array | null> {
  const hit = tileCellsCache.get(url)
  if (hit) {
    tileCellsCache.delete(url) // refresh recency
    tileCellsCache.set(url, hit)
    return hit.promise
  }
  const entry: TileCellsEntry = {
    promise: fetchAndDecodeHM3(url, undefined, 'low')
      .then((decoded) => decoded?.cells ?? null)
      .catch(() => {
        setTimeout(() => { if (tileCellsCache.get(url) === entry) tileCellsCache.delete(url) }, FAILED_TILE_RETRY_MS)
        return null
      })
      .then((cells) => { entry.done = true; evictSettledTilesBeyondLimit(); return cells }),
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

/**
 * Lden (dB) at one cell, energy-summed across the given tiles — the same
 * byte → 10^(dB/10) → Σ → 10·log10 chain the renderer paints with, so the
 * number equals the pixel colour. `null` where no tile carries data.
 */
export function energySumLdenDb(tiles: readonly (Uint8Array | null)[], cell: HM3CellAddress): number | null {
  let sumLinear = 0
  let anyData = false
  for (const cells of tiles) {
    const byte = cells?.[cell.py * TILE_PX + cell.px]
    if (byte === undefined || byte === NO_DATA) continue
    sumLinear += HM3_BYTE_ENERGY[byte]
    anyData = true
  }
  return anyData ? 10 * Math.log10(sumLinear) : null
}
