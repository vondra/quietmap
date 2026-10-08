// The layer controls, shared by the desktop ControlCard and the phone LayersPanel: the noise
// layers and the quiet-zone overlay.
import OverlayControls from './OverlayControls'
import SourceToggles from './SourceToggles'

export interface LayerControlsBodyProps {
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
    </>
  )
}
