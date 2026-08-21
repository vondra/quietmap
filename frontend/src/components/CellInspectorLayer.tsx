import { useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { Source, Layer, useMap } from 'react-map-gl/maplibre'
import { HEATMAP_LAYERS } from './HeatmapOverlay'
import { lngLatToTile, tileXToLng, tileYToLat } from '../lib/tile-math'

const TILE_SIZE = 64
const CELL_STEP_DEG = 1 / 3600
const TILE_CACHE_MAX = 256
const MIN_ZOOM = 14

const DATA_LAYERS = ['dem', 'building', 'forest'] as const
type DataLayer = (typeof DATA_LAYERS)[number]

// The Overture building-height raster is keyed `building-height` in
// rasterOverlays (distinct from the `building` noise heatmap layer); its tile
// + readout path stays `building`.
const overlayKey = (l: DataLayer): string => (l === 'building' ? 'building-height' : l)

interface CellInspectorLayerProps {
  rasterOverlays: Record<string, boolean>
}

type TileEntry = ArrayBuffer | 'loading' | 'failed'

/**
 * Hover tooltip that shows the raw raster-cell values (DEM elevation /
 * Overture building height / WorldCover forest flag) of the 1°/3600 cell
 * under the cursor. Active only while at least one Advanced overlay is on
 * AND every noise-source layer is off — otherwise the popup takes priority.
 */
export default function CellInspectorLayer({
  rasterOverlays,
}: CellInspectorLayerProps) {
  const { current: mapRef } = useMap()
  const tileCache = useRef<Map<string, TileEntry>>(new Map())
  const [hover, setHover] = useState<{
    lat: number
    lng: number
    clientX: number
    clientY: number
    zoom: number
  } | null>(null)
  const [tileEpoch, setTileEpoch] = useState(0)

  const activeLayers: DataLayer[] = useMemo(
    () => DATA_LAYERS.filter(id => rasterOverlays[overlayKey(id)]),
    [rasterOverlays],
  )

  // Defer to the noise popup/heatmap: show raw raster cell values only when
  // no noise heatmap layer is on.
  const noiseLayersOff = useMemo(
    () => !HEATMAP_LAYERS.some(id => rasterOverlays[id]),
    [rasterOverlays],
  )

  const enabled = activeLayers.length > 0 && noiseLayersOff
  const shouldRenderCellOutline = activeLayers.some(layer => layer !== 'building')

  useEffect(() => {
    if (!enabled || !mapRef) {
      setHover(null)
      return
    }
    const map = mapRef.getMap()
    const onMove = (e: maplibregl.MapMouseEvent) => {
      const zoom = map.getZoom()
      if (zoom < MIN_ZOOM) { setHover(null); return }
      setHover({
        lat: e.lngLat.lat,
        lng: e.lngLat.lng,
        clientX: e.point.x,
        clientY: e.point.y,
        zoom,
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
  }, [mapRef, enabled])

  // Raster cells are centered on integer multiples of 1/3600° (the sample
  // grid positions), so `Math.round` snaps the hover point to the nearest
  // cell-center — NOT `Math.floor`, which would offset the outline by half
  // a cell and straddle four real cells.
  const cellI = hover ? Math.round(hover.lat / CELL_STEP_DEG) : 0
  const cellJ = hover ? Math.round(hover.lng / CELL_STEP_DEG) : 0
  const cellCenterLat = cellI * CELL_STEP_DEG
  const cellCenterLon = cellJ * CELL_STEP_DEG

  // Kick off tile fetches at the CELL CENTER (not the raw hover position)
  // so every point inside the outline reads from the same raster sample.
  const dataZ = hover ? clamp(Math.floor(hover.zoom), MIN_ZOOM, 16) : MIN_ZOOM
  useEffect(() => {
    if (!hover) return
    const { x, y } = lngLatToTile(cellCenterLon, cellCenterLat, dataZ)
    for (const layer of activeLayers) {
      ensureTileFetched(layer, dataZ, x, y, tileCache.current, () =>
        setTileEpoch(e => e + 1),
      )
    }
  }, [hover, dataZ, activeLayers, cellCenterLat, cellCenterLon])

  const values = useMemo(() => {
    if (!hover) return null
    const { x, y } = lngLatToTile(cellCenterLon, cellCenterLat, dataZ)
    const out: Partial<Record<DataLayer, number | null>> = {}
    for (const layer of activeLayers) {
      out[layer] = readCellValue(
        layer, dataZ, cellCenterLat, cellCenterLon, x, y, tileCache.current,
      )
    }
    return out
    // tileEpoch re-reads cached entries once a background fetch lands.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hover, dataZ, activeLayers, cellCenterLat, cellCenterLon, tileEpoch])

  // Memoise the cell outline on cell identity so MapLibre only rebuilds the
  // polygon when the cursor crosses into a new cell.
  const cellKey = hover ? `${cellI}:${cellJ}` : null
  const outline = useMemo(() => {
    if (!hover || !shouldRenderCellOutline) return null
    const half = CELL_STEP_DEG / 2
    const lat0 = cellCenterLat - half
    const lon0 = cellCenterLon - half
    const lat1 = cellCenterLat + half
    const lon1 = cellCenterLon + half
    return {
      type: 'FeatureCollection' as const,
      features: [{
        type: 'Feature' as const,
        geometry: {
          type: 'Polygon' as const,
          coordinates: [[[lon0, lat0], [lon1, lat0], [lon1, lat1], [lon0, lat1], [lon0, lat0]]],
        },
        properties: {},
      }],
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cellKey, shouldRenderCellOutline])

  if (!enabled || !hover || !values) return null

  return (
    <>
      {outline ? (
        <Source id="cell-inspector-outline" type="geojson" data={outline}>
          <Layer
            id="cell-inspector-outline-line"
            type="line"
            paint={{ 'line-color': 'rgba(0,0,0,0.65)', 'line-width': 1 }}
          />
          <Layer
            id="cell-inspector-outline-fill"
            type="fill"
            paint={{ 'fill-color': 'rgba(255,255,255,0.12)' }}
          />
        </Source>
      ) : null}
      <div
        style={{
          position: 'fixed',
          left: hover.clientX + 14,
          top: hover.clientY + 14,
          pointerEvents: 'none',
          zIndex: 1002,
        }}
        className="rounded-md bg-zinc-900/95 text-zinc-50 border border-zinc-700/60 shadow-xl px-2 py-1 font-mono text-[11px] leading-snug"
      >
        {renderLines(values)}
      </div>
    </>
  )
}

const TOOLTIP_ROWS: Array<{
  key: DataLayer
  label: string
  fmt: (v: number) => string
}> = [
  { key: 'dem', label: 'Elevation', fmt: v => `${Math.round(v)} m` },
  { key: 'building', label: 'Building', fmt: v => v > 0 ? `${v} m` : 'none' },
  // Canopy density 0–100 % (geodata-v2 2a continuous tiles; the legacy
  // binary raster reads 100 % where forested, so this stays truthful).
  { key: 'forest', label: 'Forest', fmt: v => v > 0 ? `${Math.round(v)} %` : 'no' },
]

function renderLines(values: Partial<Record<DataLayer, number | null>>): ReactNode {
  const rows: ReactNode[] = []
  for (const row of TOOLTIP_ROWS) {
    if (!(row.key in values)) continue
    const v = values[row.key]
    rows.push(
      <div key={row.key}>
        {row.label}: {v == null ? '…' : row.fmt(v)}
      </div>,
    )
  }
  return rows
}

function tileBbox(z: number, x: number, y: number) {
  return {
    lonWest: tileXToLng(x, z),
    lonEast: tileXToLng(x + 1, z),
    latNorth: tileYToLat(y, z),
    latSouth: tileYToLat(y + 1, z),
  }
}

function ensureTileFetched(
  layer: DataLayer,
  z: number,
  x: number,
  y: number,
  cache: Map<string, TileEntry>,
  onLoaded: () => void,
): void {
  const key = `${layer}/${z}/${x}/${y}`
  if (cache.has(key)) return
  cache.set(key, 'loading')
  promoteLru(cache, key)
  fetch(`/api/raster-data/${layer}/${z}/${x}/${y}.bin`)
    .then(res => {
      // 204 (missing tile) is `res.ok = true` but a 0-byte body would
      // make `DataView.getInt16` throw and unmount the React tree.
      // Mark failed alongside real errors so the reader returns `null`.
      if (res.status === 204 || !res.ok) throw new Error(`status ${res.status}`)
      return res.arrayBuffer()
    })
    .then(buf => {
      if (buf.byteLength === 0) {
        cache.set(key, 'failed')
      } else {
        cache.set(key, buf)
      }
      promoteLru(cache, key)
      onLoaded()
    })
    .catch(() => { cache.set(key, 'failed'); onLoaded() })
}

function readCellValue(
  layer: DataLayer,
  z: number,
  lat: number,
  lng: number,
  tileX: number,
  tileY: number,
  cache: Map<string, TileEntry>,
): number | null {
  const entry = cache.get(`${layer}/${z}/${tileX}/${tileY}`)
  if (!entry || entry === 'loading' || entry === 'failed') return null
  const { lonWest, lonEast, latNorth, latSouth } = tileBbox(z, tileX, tileY)
  const mercYNorth = Math.log(Math.tan(Math.PI / 4 + (latNorth * Math.PI) / 360))
  const mercYSouth = Math.log(Math.tan(Math.PI / 4 + (latSouth * Math.PI) / 360))
  const mercY = Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360))
  const fracY = (mercY - mercYNorth) / (mercYSouth - mercYNorth)
  const py = clamp(Math.floor(fracY * TILE_SIZE), 0, TILE_SIZE - 1)
  const fracX = (lng - lonWest) / (lonEast - lonWest)
  const px = clamp(Math.floor(fracX * TILE_SIZE), 0, TILE_SIZE - 1)
  const idx = py * TILE_SIZE + px
  // Guard a truncated tile: getInt16 past the buffer throws synchronously, and
  // this runs inside a render useMemo, so it would unmount the tree
  // (ensureTileFetched only rejects 0-byte bodies, not short-but-nonzero ones).
  if (layer === 'dem') {
    return idx * 2 + 2 <= entry.byteLength
      ? new DataView(entry).getInt16(idx * 2, false)
      : null
  }
  return idx < entry.byteLength ? new Uint8Array(entry)[idx] : null
}

function promoteLru(cache: Map<string, TileEntry>, key: string): void {
  const v = cache.get(key)
  if (v === undefined) return
  cache.delete(key)
  cache.set(key, v)
  while (cache.size > TILE_CACHE_MAX) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    cache.delete(oldest)
  }
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v))
}
