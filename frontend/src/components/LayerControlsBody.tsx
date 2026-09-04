import OverlayControls from './OverlayControls'
import AdvancedSection from './AdvancedSection'
import type { RealEstateFilters } from './RealEstateLayer'
import type { StayFilters } from './StayLayer'

export interface LayerControlsBodyProps {
  quietClustersEnabled: boolean
  onQuietClustersChange: (enabled: boolean) => void
  quietThreshold: number
  onQuietThresholdChange: (threshold: number) => void
  realEstateFilters: RealEstateFilters
  onRealEstateChange: (filters: RealEstateFilters) => void
  stayFilters: StayFilters
  onStayChange: (filters: StayFilters) => void
  rasterOverlays: Record<string, boolean>
  onRasterOverlayChange: (overlays: Record<string, boolean>) => void
  dividerSpacing?: 'compact' | 'comfortable'
}

/**
 * Shared body for the layer controls panel — rendered inside both the desktop
 * floating ControlCard and the mobile bottom-sheet LayersPanel. Only the shell
 * (header, dismiss affordance, animation, visibility media query) differs.
 */
export default function LayerControlsBody({
  quietClustersEnabled, onQuietClustersChange,
  quietThreshold, onQuietThresholdChange,
  realEstateFilters, onRealEstateChange,
  stayFilters, onStayChange,
  rasterOverlays, onRasterOverlayChange,
  dividerSpacing = 'compact',
}: LayerControlsBodyProps) {
  const divClass = dividerSpacing === 'compact'
    ? 'my-1.5 border-t border-border'
    : 'my-2 border-t border-border'

  return (
    <>
      <OverlayControls
        quietClustersEnabled={quietClustersEnabled}
        onQuietClustersChange={onQuietClustersChange}
        quietThreshold={quietThreshold}
        onQuietThresholdChange={onQuietThresholdChange}
        realEstateFilters={realEstateFilters}
        onRealEstateChange={onRealEstateChange}
        stayFilters={stayFilters}
        onStayChange={onStayChange}
      />

      <div className={divClass} />

      <AdvancedSection
        rasterOverlays={rasterOverlays}
        onRasterOverlayChange={onRasterOverlayChange}
      />
    </>
  )
}
