import { useEffect, useMemo, useState } from 'react'
import { useMap } from 'react-map-gl/maplibre'

import { displayedTileZoom, hm3CellAt, readHeatmapCell, tileCells, type HeatmapCellReadout, type SampledTile } from '../lib/hm3-sample'
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
      layers: sources.map((source) => ({ source, url: tileUrl(build, source, tileZ, tileX, tileY) })),
    }
  }, [tileZ, tileX, tileY, build, sources])
  const [loaded, setLoaded] = useState<{ key: string; layers: { source: string; tile: SampledTile }[] } | null>(null)
  useEffect(() => {
    if (!sample) return
    let cancelled = false
    void Promise.all(sample.layers.map(({ source, url }) => tileCells(url).then((tile) => ({ source, tile })))).then((layers) => {
      if (!cancelled) setLoaded({ key: sample.key, layers })
    })
    return () => { cancelled = true }
  }, [sample])

  if (!hover || !cell || !sample) return null
  // '…' until every source tile of this sample has landed: a partial or
  // previous-set sum would mislead.
  const readout = loaded?.key === sample.key
    ? heatmapCellReadoutText(readHeatmapCell(loaded.layers, cell))
    : '…'
  return <MapHoverBox hover={hover} placement="above" testId="heatmap-hover">Lden: {readout}</MapHoverBox>
}

function heatmapCellReadoutText(readout: HeatmapCellReadout): string {
  switch (readout.kind) {
    case 'level': return `${readout.ldenDb.toFixed(1)} dB`
    case 'no-modelled-source': return 'no modelled source'
    case 'not-assessed': return 'not computed'
    case 'unavailable': return `— (${readout.failedSources.map(heatmapSourceName).join(', ')} unavailable)`
  }
}

function heatmapSourceName(source: string): string {
  return source === 'total' ? 'all layers' : source.replace('-', ' ')
}
