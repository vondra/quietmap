// The layer panel's "Advanced" group: the data layers, folded away until opened (open while any is
// on), each on with its legend.
import { useState } from 'react'
import { Building, Car, ChevronDown, Fence, Mountain, SquareDashed, TrainFront, Trees, Volume2 } from 'lucide-react'
import { DATA_LAYERS, type DataLayerId } from '../lib/data-layers'
import LEGEND from '../lib/data-legend.json'
import { Switch } from './ui/switch'

/** A layer's legend under its switch: the renderer's colours (`qm-raster --legend`). */
function Legend({ id }: { id: DataLayerId }) {
  return (
    <div className="ml-6 mb-1 flex flex-wrap gap-x-2 gap-y-0.5" data-testid={`legend-${id}`}>
      {LEGEND[id].map(entry => (
        <span key={entry.label} className="flex items-center gap-1 text-xs leading-4 text-muted-foreground">
          <span
            className="inline-block size-2.5 shrink-0 rounded-sm"
            style={{ backgroundColor: entry.colour, opacity: entry.opacity }}
          />
          {entry.label}
        </span>
      ))}
    </div>
  )
}

const ICONS: Record<DataLayerId, React.ReactNode> = {
  elevation: <Mountain className="size-3.5" />,
  forest: <Trees className="size-3.5" />,
  hard: <SquareDashed className="size-3.5" />,
  buildings: <Building className="size-3.5" />,
  traffic: <Car className="size-3.5" />,
  trains: <TrainFront className="size-3.5" />,
  others: <Volume2 className="size-3.5" />,
  barriers: <Fence className="size-3.5" />,
}

export interface DataLayersSectionProps {
  dataLayers: DataLayerId[]
  onDataLayersChange: (layers: DataLayerId[]) => void
}

export default function DataLayersSection({ dataLayers, onDataLayersChange }: DataLayersSectionProps) {
  const [open, setOpen] = useState(dataLayers.length > 0)
  const toggle = (id: DataLayerId) => onDataLayersChange(
    dataLayers.includes(id)
      ? dataLayers.filter(each => each !== id)
      : DATA_LAYERS.map(layer => layer.id).filter(each => each === id || dataLayers.includes(each)),
  )
  return (
    <div>
      <button
        onClick={() => setOpen(value => !value)}
        title="The data every click computes over: the ground's height, the forest, hard ground, buildings, road traffic, trains, the other sources and noise barriers"
        aria-expanded={open}
        className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-accent transition-colors cursor-pointer text-muted-foreground hover:text-foreground"
      >
        <span className="flex-1 text-left text-sm font-medium">Advanced</span>
        <ChevronDown className={`size-3.5 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="ml-1 space-y-0.5 pb-1">
          {DATA_LAYERS.map(layer => {
            const active = dataLayers.includes(layer.id)
            return (
              <div key={layer.id}>
              <button
                onClick={() => toggle(layer.id)}
                title={`${layer.tooltip}\n\nFrom zoom ${layer.minzoom}.`}
                aria-pressed={active}
                data-testid={`data-layer-${layer.id}`}
                className="flex w-full items-center gap-2 py-1 px-1 rounded-lg hover:bg-accent transition-colors cursor-pointer"
              >
                <span className={active ? 'text-foreground' : 'text-muted-foreground'}>{ICONS[layer.id]}</span>
                <span className={`flex-1 text-left text-xs ${active ? 'text-foreground' : 'text-muted-foreground'}`}>{layer.label}</span>
                <Switch on={active} size="sm" />
              </button>
              {active && <Legend id={layer.id} />}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
