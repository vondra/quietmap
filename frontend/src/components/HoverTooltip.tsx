import { useEffect, useMemo, useRef, useState } from 'react'
import { useMap } from 'react-map-gl/maplibre'
import maplibregl from 'maplibre-gl'

import { fetchAndDecodeHM3, NO_DATA, TILE_PX } from '../lib/hm3-decoder'
import type { HeatmapSource } from './HeatmapOverlay'
import { lngLatToTileFloat } from '../lib/tile-math'
import { MIN_ZOOM, tileUrl, useTileBuild } from '../lib/tile-urls'

const TILE_CACHE_MAX = 64

type TileEntry = Uint8Array | 'loading' | 'failed'

interface Props {
  /** Layers to read: the active subset, or `['total']` when all are on. */
  sources: readonly HeatmapSource[]
}

/**
 * Floating tooltip showing the total Lden under the cursor — the
 * energy-sum of every active aircraft heatmap source. Same math as
 * the visual energy-sum in `HeatmapOverlay`, so the number on
 * screen matches the colour pixel under the cursor.
 *
 * Reads at the DISPLAYED tile zoom (deck fetches `round(viewport)` clamped
 * to the layer band): the screen paints a z5 pyramid pixel at world zoom,
 * so reading z5 both matches that pixel exactly AND costs a ~15 KB tile
 * instead of the ~100-200 KB base-zoom tile a fixed read pulled on
 * every hover region (slow-connection report, 2026-07-09).
 */
export default function HoverTooltip({ sources }: Props) {
  const { current: mapRef } = useMap()
  // Generation snapshot — cache keys are full tile URLs, which carry it.
  const build = useTileBuild()
  const tileCache = useRef<Map<string, TileEntry>>(new Map())
  const [hover, setHover] = useState<{
    lat: number; lng: number; clientX: number; clientY: number; zoom: number
  } | null>(null)
  const [tileEpoch, setTileEpoch] = useState(0)

  useEffect(() => {
    if (!mapRef || sources.length === 0) {
      setHover(null)
      return
    }
    const map = mapRef.getMap()
    const onMove = (e: maplibregl.MapMouseEvent) => {
      setHover({
        lat: e.lngLat.lat, lng: e.lngLat.lng,
        clientX: e.point.x, clientY: e.point.y,
        zoom: map.getZoom(),
      })
    }
    const onLeave = () => setHover(null)
    map.on('mousemove', onMove)
    map.on('mouseout', onLeave)
    map.on('dragstart', onLeave)
    return () => {
      map.off('mousemove', onMove)
      map.off('mouseout', onLeave)
      map.off('dragstart', onLeave)
    }
  }, [mapRef, sources])

  // (lat, lng) → displayed-zoom tile coord + per-tile (py, px). Web Mercator
  // floor math mirrors deck.gl's TileLayer (tile z = round(viewport zoom),
  // clamped to the layer band) so the byte we read is the same byte the
  // renderer painted — and past OVERZOOM the base tile is what's magnified.
  const tileInfo = useMemo(() => {
    if (!hover || build === null) return null
    // Below MIN_ZOOM the heat layer itself is invisible (deck renders nothing
    // under a layer's minZoom) — the tooltip goes silent too, instead of
    // clamping up and quoting pixels that aren't painted (/gg Gemini).
    if (Math.round(hover.zoom) < MIN_ZOOM) return null
    // Mirror the renderer's HiDPI zoomOffset: deck requests z+1 on DPR ≥ 1.5,
    // so reading round(zoom) alone would quote a different byte than the
    // painted pixel wherever the finer level exists (gg z13 impl review,
    // Codex #6). The published base zoom is the shared ceiling.
    const zoomOffset = window.devicePixelRatio >= 1.5 ? 1 : 0
    const z = Math.min(build.zoom, Math.round(hover.zoom) + zoomOffset)
    const [xFloat, yFloat] = lngLatToTileFloat(hover.lng, hover.lat, z)
    const tx = Math.floor(xFloat)
    const ty = Math.floor(yFloat)
    const px = Math.min(TILE_PX - 1, Math.floor((xFloat - tx) * TILE_PX))
    const py = Math.min(TILE_PX - 1, Math.floor((yFloat - ty) * TILE_PX))
    return { z, tx, ty, px, py }
  }, [hover, build])

  // Fetch any missing source tiles in the background. Cache is a
  // simple insertion-ordered LRU keyed by the tile URL — the URL carries the
  // tile build, so a mid-session generation flip naturally re-keys the cache.
  useEffect(() => {
    if (!tileInfo || build === null) return
    const { z, tx, ty } = tileInfo
    for (const source of sources) {
      const key = tileUrl(build, source, z, tx, ty)
      if (tileCache.current.has(key)) continue
      tileCache.current.set(key, 'loading')
      ;(async () => {
        try {
          const decoded = await fetchAndDecodeHM3(key, undefined, 'low')
          tileCache.current.set(key, decoded?.cells ?? 'failed')
        } catch {
          // Transient (network/5xx) — DROP the entry so the next hover
          // retries, instead of pinning '—' for the whole session (gg z13
          // impl review, Codex #7). A decoded empty tile still caches as
          // authoritative 'failed'-shaped silence above.
          tileCache.current.delete(key)
        }
        while (tileCache.current.size > TILE_CACHE_MAX) {
          const oldest = tileCache.current.keys().next().value
          if (!oldest) break
          tileCache.current.delete(oldest)
        }
        setTileEpoch((e) => e + 1)
      })()
    }
  }, [tileInfo, sources, build])

  // Energy-sum readout across active sources. Same byte-decode +
  // 10^(dB/10) → sum → 10·log10 chain `sumCellsEnergy` does at
  // render time, so the value here equals the pixel colour under
  // the cursor. Builds the display string directly: '…' while
  // tiles load, '—' outside the data island, 'NN.N dB' otherwise.
  const readout = useMemo(() => {
    // The epoch is an explicit invalidation signal for mutations inside the
    // ref-backed tile cache; reading it makes that dependency visible to the
    // hooks checker without moving large tile arrays into React state.
    void tileEpoch
    if (!tileInfo || build === null) return null
    const { z, tx, ty, px, py } = tileInfo
    let sumLin = 0
    let anyData = false
    let anyLoading = false
    for (const source of sources) {
      const entry = tileCache.current.get(tileUrl(build, source, z, tx, ty))
      if (entry instanceof Uint8Array) {
        const byte = entry[py * TILE_PX + px]
        if (byte !== NO_DATA) {
          sumLin += 10 ** ((byte * 0.5) / 10)
          anyData = true
        }
      } else if (entry === 'loading') {
        anyLoading = true
      }
    }
    if (anyData) return `${(10 * Math.log10(sumLin)).toFixed(1)} dB`
    if (anyLoading) return '…'
    return '—'
  }, [tileInfo, sources, build, tileEpoch])

  if (!hover || readout === null) return null

  return (
    <div
      data-testid="heatmap-hover"
      style={{
        position: 'fixed',
        left: hover.clientX + 14,
        top: hover.clientY + 14,
        pointerEvents: 'none',
        zIndex: 1003,
      }}
      className="rounded-md bg-zinc-900/95 text-zinc-50 border border-zinc-700/60 shadow-xl px-2 py-1 font-mono text-[11px] leading-snug"
    >
      Lden: {readout}
    </div>
  )
}
