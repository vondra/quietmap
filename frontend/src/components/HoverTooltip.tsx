// The loudness under the cursor, read from the heatmap tile the renderer painted.
import { useEffect, useMemo, useState } from 'react'
import { useMap } from 'react-map-gl/maplibre'

import { displayedTileZoom, hm3CellAt, readHeatmapCell, tileCells, type HeatmapCellReadout, type SampledTile } from '../lib/hm3-sample'
import { buildKey, tileUrl, useTileBuild, type HeatmapSource } from '../lib/tile-urls'
import { useMapHover } from '../lib/use-map-hover'
import { fmtSone } from '../utils/formatters'
import MapHoverBox from './MapHoverBox'

interface Props {
  /** The source the map shows: the total or one layer; `null` reads nothing. */
  source: HeatmapSource | null
}

/**
 * Floating readout of the loudness under the cursor, read from the SAME tile the renderer painted
 * (the displayed pyramid level), so the number is that of the HM3 cell under the cursor (the
 * painted pixel blends neighbouring cells by linear filtering and quantises to half a phon) and
 * costs a ~15 KB pyramid tile at world zoom instead of a base-zoom tile per hover region.
 */
export default function HoverTooltip({ source }: Props) {
  const { current: mapRef } = useMap()
  const build = useTileBuild()
  const hover = useMapHover(mapRef, source !== null)

  const cell = useMemo(
    () => (hover && build ? hm3CellAt(hover.lng, hover.lat, displayedTileZoom(build, hover.zoom, window.devicePixelRatio)) : null),
    [hover, build],
  )
  // The tile changes far less often than the cell: it is loaded once per tile × source ×
  // generation (primitive deps, so a mousemove inside one tile re-uses the sample) and held here
  // so a cache eviction cannot take it away from the readout.
  const tileZ = cell?.z, tileX = cell?.tx, tileY = cell?.ty
  const sample = useMemo(() => {
    if (tileZ === undefined || tileX === undefined || tileY === undefined || !build || source === null) return null
    return {
      key: `${buildKey(build, source)}|${tileZ}/${tileX}/${tileY}`,
      url: tileUrl(build, source, tileZ, tileX, tileY),
    }
  }, [tileZ, tileX, tileY, build, source])
  const [loaded, setLoaded] = useState<{ key: string; tile: SampledTile } | null>(null)
  useEffect(() => {
    if (!sample) return
    let cancelled = false
    void tileCells(sample.url).then((tile) => {
      if (!cancelled) setLoaded({ key: sample.key, tile })
    })
    return () => { cancelled = true }
  }, [sample])

  if (!hover || !cell || !sample) return null
  // '…' until this sample's tile has landed: a previous tile's cell would mislead.
  const readout = loaded?.key === sample.key ? heatmapCellReadoutText(readHeatmapCell(loaded.tile, cell)) : '…'
  return <MapHoverBox hover={hover} placement="above" testId="heatmap-hover">{readout}</MapHoverBox>
}

function heatmapCellReadoutText(readout: HeatmapCellReadout): string {
  switch (readout.kind) {
    case 'level': return `${fmtSone(readout.sone)} sone`
    case 'lden': return `${readout.db.toFixed(1)} dB`
    case 'silence': return 'silent'
    case 'no-level': return '—'
    case 'unavailable': return '— (unavailable)'
  }
}
