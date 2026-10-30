// The layer controls, shared by the desktop ControlCard and the phone LayersPanel: the quantity
// shown, the noise layers, the overlays (quiet zones, places to stay) and the data layers.
import type { Metric } from '../lib/hm3-decoder'
import DataLayersSection, { type DataLayersSectionProps } from './DataLayersSection'
import MetricSwitch from './MetricSwitch'
import OverlayControls, { type OverlayControlsProps } from './OverlayControls'
import SourceToggles from './SourceToggles'

export interface LayerControlsBodyProps extends DataLayersSectionProps, OverlayControlsProps {
  onMetricChange: (metric: Metric) => void
  heatmapLayers: Record<string, boolean>
  onHeatmapLayersChange: (layers: Record<string, boolean>) => void
  dividerSpacing?: 'compact' | 'comfortable'
}

export default function LayerControlsBody({
  onMetricChange, heatmapLayers, onHeatmapLayersChange,
  dataLayers, onDataLayersChange,
  dividerSpacing = 'compact',
  ...overlays
}: LayerControlsBodyProps) {
  const divClass = dividerSpacing === 'compact'
    ? 'my-1.5 border-t border-border'
    : 'my-2 border-t border-border'

  return (
    <>
      <MetricSwitch metric={overlays.metric} onChange={onMetricChange} />
      <SourceToggles
        heatmapLayers={heatmapLayers}
        onHeatmapLayersChange={onHeatmapLayersChange}
      />

      <div className={divClass} />

      <OverlayControls {...overlays} />

      <div className={divClass} />

      <DataLayersSection dataLayers={dataLayers} onDataLayersChange={onDataLayersChange} />
    </>
  )
}
