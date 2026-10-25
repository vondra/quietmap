// The layer controls, shared by the desktop ControlCard and the phone LayersPanel: the noise
// layers, the overlays (quiet zones, places to stay), the data layers and the account.
import AccountSection, { type AccountSectionProps } from './AccountSection'
import DataLayersSection, { type DataLayersSectionProps } from './DataLayersSection'
import OverlayControls, { type OverlayControlsProps } from './OverlayControls'
import SourceToggles from './SourceToggles'

export interface LayerControlsBodyProps extends DataLayersSectionProps, OverlayControlsProps, AccountSectionProps {
  heatmapLayers: Record<string, boolean>
  onHeatmapLayersChange: (layers: Record<string, boolean>) => void
  dividerSpacing?: 'compact' | 'comfortable'
}

export default function LayerControlsBody({
  heatmapLayers, onHeatmapLayersChange,
  dataLayers, onDataLayersChange,
  account, onSignOut, onDeleteAccount,
  dividerSpacing = 'compact',
  ...overlays
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

      <OverlayControls {...overlays} />

      <div className={divClass} />

      <DataLayersSection dataLayers={dataLayers} onDataLayersChange={onDataLayersChange} />

      <AccountSection account={account} onSignOut={onSignOut} onDeleteAccount={onDeleteAccount} dividerClass={divClass} />
    </>
  )
}
