import { Car, TrainFront, Plane, Building2, Factory } from 'lucide-react'
import type { ReactNode } from 'react'
import type { HeatmapLayer } from './HeatmapOverlay'
import { Switch } from './ui/switch'

interface LayerRow {
  id: HeatmapLayer
  label: string
  tooltip: string
  icon: ReactNode
}

// The layer panel: the seven noise layers, all on by default. There is no
// `total` toggle — when every layer is on the overlay fetches the precomputed
// `total` tile automatically (see MapView); turning any off sums the rest.
const LAYER_ROWS: LayerRow[] = [
  { id: 'road', label: 'Roads', tooltip: 'Car, truck, and motorcycle traffic noise', icon: <Car className="size-4" /> },
  { id: 'rail', label: 'Railways', tooltip: 'Trains, trams, and freight lines', icon: <TrainFront className="size-4" /> },
  { id: 'industrial', label: 'Industrial', tooltip: 'Factories, power plants, quarries', icon: <Factory className="size-4" /> },
  { id: 'building', label: 'Buildings', tooltip: 'Everyday building noise — HVAC, activity, deliveries', icon: <Building2 className="size-4" /> },
  { id: 'aircraft-ground', label: 'Aircraft — ground ops', tooltip: 'Taxi + runway roll + apron movements', icon: <Plane className="size-4" /> },
  { id: 'aircraft-airborne', label: 'Aircraft — airborne', tooltip: 'Climb / approach / departure within ~3000 m AGL', icon: <Plane className="size-4" /> },
  { id: 'aircraft-cruise', label: 'Aircraft — cruise', tooltip: 'High-altitude overflight (FL100+)', icon: <Plane className="size-4" /> },
]

interface SourceTogglesProps {
  rasterOverlays: Record<string, boolean>
  onRasterOverlayChange: (overlays: Record<string, boolean>) => void
}

export default function SourceToggles({
  rasterOverlays, onRasterOverlayChange,
}: SourceTogglesProps) {
  return (
    <div data-testid="layers-panel">
      {LAYER_ROWS.map(row => {
        const active = !!rasterOverlays[row.id]
        return (
          <button
            key={row.id}
            onClick={() => onRasterOverlayChange({ ...rasterOverlays, [row.id]: !active })}
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
