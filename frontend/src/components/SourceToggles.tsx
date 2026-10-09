// The layer panel's noise-layer switches, all on by default.
import { Car, TrainFront, Plane, Building2, Factory, Ship } from 'lucide-react'
import type { ReactNode } from 'react'
import type { HeatmapLayer } from '../lib/tile-urls'
import { Switch } from './ui/switch'

interface LayerRow {
  id: HeatmapLayer
  label: string
  tooltip: string
  icon: ReactNode
}

// The layer panel: the noise layers, all on by default. There is no
// `total` toggle — when every layer is on the overlay fetches the precomputed
// `total` tile automatically (see MapView); turning any off sums the rest.
const LAYER_ROWS: LayerRow[] = [
  { id: 'road', label: 'Roads', tooltip: 'Cars, lorries, buses and motorcycles', icon: <Car className="size-4" /> },
  { id: 'railway', label: 'Railways', tooltip: 'Trains and trams', icon: <TrainFront className="size-4" /> },
  { id: 'industrial', label: 'Industrial', tooltip: 'Industry, wind turbines, quarries and other sites', icon: <Factory className="size-4" /> },
  { id: 'building', label: 'Buildings', tooltip: 'Plant on buildings, bells and calls, people outside bars, sport and parking', icon: <Building2 className="size-4" /> },
  { id: 'ship', label: 'Ships', tooltip: 'Ships and boats', icon: <Ship className="size-4" /> },
  { id: 'aircraft', label: 'Aircraft', tooltip: 'Flights and the airports\' ground operations', icon: <Plane className="size-4" /> },
]

interface SourceTogglesProps {
  heatmapLayers: Record<string, boolean>
  onHeatmapLayersChange: (layers: Record<string, boolean>) => void
}

export default function SourceToggles({
  heatmapLayers, onHeatmapLayersChange,
}: SourceTogglesProps) {
  return (
    <div data-testid="layers-panel">
      {LAYER_ROWS.map(row => {
        const active = !!heatmapLayers[row.id]
        return (
          <button
            key={row.id}
            onClick={() => onHeatmapLayersChange({ ...heatmapLayers, [row.id]: !active })}
            title={row.tooltip}
            aria-pressed={active}
            data-testid={`layer-${row.id}`}
            className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-black/5 transition-colors cursor-pointer"
          >
            <span className={active ? 'text-foreground' : 'text-muted-foreground'}>{row.icon}</span>
            <span className={`flex-1 text-left text-sm ${active ? 'text-foreground' : 'text-muted-foreground'}`}>{row.label}</span>
            <Switch on={active} />
          </button>
        )
      })}
    </div>
  )
}
