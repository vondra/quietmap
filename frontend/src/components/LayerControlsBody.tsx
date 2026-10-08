// The layer controls, shared by the desktop ControlCard and the phone LayersPanel: the noise
// layers, the quiet-zone overlay and the data layers.
import DataLayersSection, { type DataLayersSectionProps } from './DataLayersSection'
import OverlayControls from './OverlayControls'
import SourceToggles from './SourceToggles'

export interface LayerControlsBodyProps extends DataLayersSectionProps {
  quietClustersEnabled: boolean
  onQuietClustersChange: (enabled: boolean) => void
  quietThreshold: number
  onQuietThresholdChange: (threshold: number) => void
  heatmapLayers: Record<string, boolean>
  onHeatmapLayersChange: (layers: Record<string, boolean>) => void
  dividerSpacing?: 'compact' | 'comfortable'
}

export default function LayerControlsBody({
  quietClustersEnabled, onQuietClustersChange,
  quietThreshold, onQuietThresholdChange,
  heatmapLayers, onHeatmapLayersChange,
  dataLayers, onDataLayersChange,
  dividerSpacing = 'compact',
}: LayerControlsBodyProps) {
  const divClass = dividerSpacing === 'compact'
    ? 'my-1.5 border-t border-border'
    : 'my-2 border-t border-border'

  return (
    <>
      <SourceToggles
        heatmapLayers={heatmapLayers}
        onHeatmapLayersChange={onHeatmapLayersChange}
      />

      <div className={divClass} />

      <OverlayControls
        quietClustersEnabled={quietClustersEnabled}
        onQuietClustersChange={onQuietClustersChange}
        quietThreshold={quietThreshold}
        onQuietThresholdChange={onQuietThresholdChange}
      />

      <div className={divClass} />

      <DataLayersSection dataLayers={dataLayers} onDataLayersChange={onDataLayersChange} />
    </>
  )
}
