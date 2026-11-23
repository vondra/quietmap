// Quiet zones: a green wash over every heatmap pixel at or below the chosen threshold, in sone or dB.
import { TileLayer } from '@deck.gl/geo-layers'
import { BitmapLayer } from '@deck.gl/layers'

import { QUIET_RGB } from './heatmap-palette'
import { cellByte, fetchAndDecodeHM3, HM3_NO_LEVEL, HM3_SILENCE, TILE_PX, type Metric } from './hm3-decoder'
import { buildKey, HEATMAP_LAYERS, sourceOf, tileUrl, type TileBuilds } from './tile-urls'

const QUIET_RGBA = [...QUIET_RGB, 110] as const // @ ~0.43

// Display floor for the wash — zoomed further out the green blobs read as
// noise, not places (the `total` data itself goes down to z2).
const MIN_VIEW_ZOOM = 5

type QuietTile = { cells: Uint8Array }

/** The total's green mask, after the noise heatmap and before its shared data/label anchor. */
export function makeQuietZonesLayer(
  metric: Metric, threshold: number, build: TileBuilds, beforeId: string | undefined, zoomOffset: number,
) {
  const maxByte = cellByte(metric, threshold)
  const total = sourceOf(HEATMAP_LAYERS, metric)!
  return new TileLayer<QuietTile | null>({
    // A generation flip must discard cached tiles for the previous total.
    id: `quiet-zones-${buildKey(build, total)}`,
    ...(beforeId ? { beforeId } : {}),
    // Without an extent, deck hides the wash below this zoom after adding the offset.
    minZoom: MIN_VIEW_ZOOM + zoomOffset,
    zoomOffset,
    // deck upscales the base texture for deeper viewport zooms.
    maxZoom: build.zoom,
    tileSize: TILE_PX,
    // Two 24-tile desktop views. A slider change rebuilds a 1 MiB mask per cached tile.
    maxCacheSize: 48,
    // A loaded child's transparent cells must not show green through from a coarser parent.
    refinementStrategy: 'no-overlap',
    // getTileData returns the raw decoded cells (threshold-independent), so a
    // threshold change keeps the tile cache instead of refetching. The object
    // wrapper is required — deck.gl's TileLayer mishandles a bare typed array
    // as tile data (it never reaches renderSubLayers as-is).
    getTileData: async ({ index, signal }) => {
      const url = tileUrl(build, total, index.z, index.x, index.y)
      try {
        const decoded = await fetchAndDecodeHM3(url, signal)
        return decoded ? { cells: decoded.cells } : null
      } catch (err) {
        if ((err as DOMException)?.name === 'AbortError') throw err
        return null
      }
    },
    // Dragging the slider re-masks the cached tiles (CPU only) rather than
    // refetching the network — only renderSubLayers re-runs on a threshold change.
    updateTriggers: { renderSubLayers: [maxByte] },
    renderSubLayers: (props) => {
      const data = props.data as QuietTile | null
      if (!data) return null
      const { west, south, east, north } = props.tile.bbox as {
        west: number; south: number; east: number; north: number
      }
      return new BitmapLayer({
        id: `${props.id}-bitmap`,
        // Coplanar raster meshes differ when tilted; their map order determines visibility.
        parameters: { depthCompare: 'always' },
        image: quietMaskToImageData(data.cells, maxByte),
        bounds: [west, south, east, north],
      })
    },
  })
}

/**
 * Paint cells with a level byte ≤ `maxByte` green, silence among them; leave
 * the rest transparent. A cell not assessed stays transparent: an unknown level
 * is not a quiet one.
 */
function quietMaskToImageData(cells: Uint8Array, maxByte: number): ImageData {
  const rgba = new Uint8ClampedArray(TILE_PX * TILE_PX * 4)
  const [r, g, b, a] = QUIET_RGBA
  for (let i = 0; i < cells.length; i++) {
    const byte = cells[i]
    if (byte === HM3_NO_LEVEL || (byte !== HM3_SILENCE && byte > maxByte)) continue
    const off = i * 4
    rgba[off] = r
    rgba[off + 1] = g
    rgba[off + 2] = b
    rgba[off + 3] = a
  }
  return new ImageData(rgba, TILE_PX, TILE_PX)
}
