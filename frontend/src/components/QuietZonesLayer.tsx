import { useEffect, useState } from 'react'
import { MapboxOverlay } from '@deck.gl/mapbox'
import { TileLayer } from '@deck.gl/geo-layers'
import { BitmapLayer } from '@deck.gl/layers'
import { useMap } from 'react-map-gl/maplibre'

import { fetchAndDecodeHM3, TILE_PX, NO_DATA } from '../lib/hm3-decoder'
import { buildKey, tileUrl, useTileBuild, type TileBuilds } from '../lib/tile-urls'

// Translucent green wash over pixels whose total Lden is at or below the
// user threshold. The legacy feature traced H3 hex clusters into outlined
// polygons (h3-js + flood-fill + boundary stitching) — all of which existed
// only because hexes had no raster to shade. The heatmap is now a raster,
// so thresholding its pixels into a green bitmap is pixel-accurate and reuses
// the HM3 tile machinery with none of that complexity.
const QUIET_RGBA = [22, 163, 74, 110] as const // green-600 @ ~0.43

// Display floor for the wash — zoomed further out the green blobs read as
// noise, not places (the `total` data itself goes down to z2).
const MIN_ZOOM = 5

interface Props {
  enabled: boolean
  /** Quiet if total Lden ≤ this (dB). NO_DATA pixels are excluded so the
   *  slider cleanly controls the highlighted band. */
  threshold: number
}

/**
 * Highlight quiet areas: a deck.gl `TileLayer` over the precomputed `total`
 * raster that paints every pixel at or below `threshold` dB green. deck
 * handles viewport tile loading; a threshold change re-keys the layer so the
 * mask recomputes. Its own overlay sits above the heatmap so the wash shows
 * over the faint sub-40 dB heatmap colours.
 */
export default function QuietZonesLayer({ enabled, threshold }: Props): null {
  const { current: mapRef } = useMap()
  // Generation snapshot — the layer id + fetch URLs are keyed by it (a flip
  // re-renders and re-keys instead of mixing generations in deck's cache).
  const build = useTileBuild()
  const [overlay, setOverlay] = useState<MapboxOverlay | null>(null)

  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const next = new MapboxOverlay({ interleaved: false, layers: [] })
    map.addControl(next)
    setOverlay(next)
    return () => {
      map.removeControl(next)
      setOverlay(null)
    }
  }, [mapRef])

  useEffect(() => {
    if (!overlay) return
    // build === null → no published generation yet → nothing to mount.
    overlay.setProps({
      layers: enabled && build !== null ? [makeQuietLayer(threshold, build)] : [],
    })
  }, [overlay, enabled, threshold, build])

  return null
}

type QuietTile = { cells: Uint8Array }

function makeQuietLayer(threshold: number, build: TileBuilds) {
  const maxByte = threshold * 2 // HM3 encodes dB × 2
  return new TileLayer<QuietTile | null>({
    // Generation snapshot in the id: a flip re-keys the layer so deck drops
    // its tile cache instead of masking stale-generation tiles.
    id: `quiet-zones-${buildKey(build, ['total'])}`,
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
      const url = tileUrl(build, 'total', index.z, index.x, index.y)
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
        image: quietMaskToImageData(data.cells, maxByte),
        bounds: [west, south, east, north],
      })
    },
  })
}

/**
 * Paint cells with a computed Lden ≤ `maxByte` green; leave the rest
 * transparent. NO_DATA stays transparent — "no computed value" is not the
 * same as "quiet", and treating it as quiet would flood every gap between
 * sources and make the threshold slider meaningless.
 */
function quietMaskToImageData(cells: Uint8Array, maxByte: number): ImageData {
  const rgba = new Uint8ClampedArray(TILE_PX * TILE_PX * 4)
  const [r, g, b, a] = QUIET_RGBA
  for (let i = 0; i < cells.length; i++) {
    const byte = cells[i]
    if (byte === NO_DATA || byte > maxByte) continue
    const off = i * 4
    rgba[off] = r
    rgba[off + 1] = g
    rgba[off + 2] = b
    rgba[off + 3] = a
  }
  return new ImageData(rgba, TILE_PX, TILE_PX)
}
