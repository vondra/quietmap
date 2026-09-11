import { useEffect, useMemo, useState } from 'react'
import { useMap } from 'react-map-gl/maplibre'

import { displayedTileZoom, energySumLdenDb, hm3CellAt, tileCells } from '../lib/hm3-sample'
import { buildKey, tileUrl, useTileBuild } from '../lib/tile-urls'
import { useMapHover } from '../lib/use-map-hover'
import MapHoverBox from './MapHoverBox'
import type { HeatmapSource } from './HeatmapOverlay'

interface Props {
  /** Layers to read: the active subset, or `['total']` when all are on. */
  sources: readonly HeatmapSource[]
}

/**
 * Floating readout of the Lden under the cursor — the energy-sum of every
 * active heatmap source, read from the SAME tile the renderer painted (the
 * displayed pyramid level), so the number is that of the HM3 cell under the
 * cursor (the painted pixel blends neighbouring cells by linear filtering
 * and quantises to 0.5 dB) and costs a ~15 KB pyramid tile at world zoom
 * instead of a base-zoom tile per hover region.
 */
export default function HoverTooltip({ sources }: Props) {
  const { current: mapRef } = useMap()
  const build = useTileBuild()
  const hover = useMapHover(mapRef, sources.length > 0)

  const cell = useMemo(
    () => (hover && build ? hm3CellAt(hover.lng, hover.lat, displayedTileZoom(build, hover.zoom, window.devicePixelRatio)) : null),
    [hover, build],
  )
  // The tile changes far less often than the cell: the source tiles are
  // loaded once per tile × source set × generation (primitive deps, so a
  // mousemove inside one tile re-uses the sample) and held here so a cache
  // eviction cannot take them away from the readout.
  const tileZ = cell?.z, tileX = cell?.tx, tileY = cell?.ty
  const sample = useMemo(() => {
    if (tileZ === undefined || tileX === undefined || tileY === undefined || !build) return null
    return {
      key: `${buildKey(build, sources)}|${tileZ}/${tileX}/${tileY}`,
      urls: sources.map((s) => tileUrl(build, s, tileZ, tileX, tileY)),
    }
  }, [tileZ, tileX, tileY, build, sources])
  const [loaded, setLoaded] = useState<{ key: string; tiles: (Uint8Array | null)[] } | null>(null)
  useEffect(() => {
    if (!sample) return
    let cancelled = false
    void Promise.all(sample.urls.map(tileCells)).then((tiles) => {
      if (!cancelled) setLoaded({ key: sample.key, tiles })
    })
    return () => { cancelled = true }
  }, [sample])

  if (!hover || !cell || !sample) return null
  // '…' until every source tile of this sample has landed (a partial or
  // previous-set sum would mislead), then '—' outside the data island or
  // 'NN.N dB'. A failed source tile reads as silence, as the renderer paints it.
  const db = loaded?.key === sample.key ? energySumLdenDb(loaded.tiles, cell) : undefined
  const readout = db === undefined ? '…' : db === null ? '—' : `${db.toFixed(1)} dB`
  return <MapHoverBox hover={hover} placement="above" testId="heatmap-hover">Lden: {readout}</MapHoverBox>
}
