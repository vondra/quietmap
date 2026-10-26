// The layer panel's noise-layer switches, each its own: any layers together.
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

// The layer panel: the noise layers, all on by default (the map shows the total); each switch
// turns its layer on or off, the map showing the layers on together (see MapView).
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

/** The switches after tapping `id`: its layer turned over, the others kept. */
function toggled(layers: Record<string, boolean>, id: HeatmapLayer): Record<string, boolean> {
  return Object.fromEntries(LAYER_ROWS.map(row => [row.id, row.id === id ? !layers[row.id] : !!layers[row.id]]))
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
            onClick={() => onHeatmapLayersChange(toggled(heatmapLayers, row.id))}
            title={row.tooltip}
            aria-pressed={active}
            data-testid={`layer-${row.id}`}
            className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-accent transition-colors cursor-pointer"
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
