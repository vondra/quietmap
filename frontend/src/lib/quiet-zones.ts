// Quiet zones: a green wash over every heatmap pixel at or below the chosen threshold, in sone or dB.
import { TileLayer } from '@deck.gl/geo-layers'
import { BitmapLayer } from '@deck.gl/layers'

import { QUIET_RGB } from './heatmap-palette'
import { cellByte, fetchAndDecodeHM3, HM3_NO_LEVEL, HM3_SILENCE, TILE_PX, type Metric } from './hm3-decoder'
import { buildKey, HEATMAP_LAYERS, sourceOf, tileUrl, type TileBuilds } from './tile-urls'

const QUIET_RGBA = [...QUIET_RGB, 110] as const // @ ~0.43

// Display floor for the wash — zoomed further out the green blobs read as
// noise, not places (the `total` data itself goes down to z2).
const MIN_ZOOM = 5

type QuietTile = { cells: Uint8Array }

/** The total's green mask, after the noise heatmap and before its shared data/label anchor. */
export function makeQuietZonesLayer(metric: Metric, threshold: number, build: TileBuilds, beforeId: string | undefined) {
  const maxByte = cellByte(metric, threshold)
  const total = sourceOf(HEATMAP_LAYERS, metric)!
  return new TileLayer<QuietTile | null>({
    // Generation snapshot in the id: a flip re-keys the layer so deck drops
    // its tile cache instead of masking stale-generation tiles.
    id: `quiet-zones-${buildKey(build, total)}`,
    ...(beforeId ? { beforeId } : {}),
    // Deliberately NO `extent` prop: without one deck renders nothing below
    // minZoom, which is exactly the z5 display floor above — an extent would
    // clamp-and-render the green wash at world zooms instead.
    minZoom: MIN_ZOOM,
    // deck upscales the base texture for deeper viewport zooms.
    maxZoom: build.zoom,
    tileSize: TILE_PX,
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
