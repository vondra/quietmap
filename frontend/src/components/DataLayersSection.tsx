// The layer panel's "Advanced" group: the data layers, folded away until opened (open while any is
// on).
import { useState } from 'react'
import { ChevronDown, Fence, Mountain, SquareDashed, Trees, Building } from 'lucide-react'
import { DATA_LAYERS, type DataLayerId } from '../lib/data-layers'
import { Switch } from './ui/switch'

const ICONS: Record<DataLayerId, React.ReactNode> = {
  elevation: <Mountain className="size-3.5" />,
  forest: <Trees className="size-3.5" />,
  hard: <SquareDashed className="size-3.5" />,
  buildings: <Building className="size-3.5" />,
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
        title="The data every click computes over: the ground's height, hard ground, buildings and noise barriers; and the forest, which takes nothing off"
        aria-expanded={open}
        className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-black/5 transition-colors cursor-pointer text-muted-foreground hover:text-foreground"
      >
        <span className="flex-1 text-left text-[11px] font-medium uppercase tracking-[0.08em]">Advanced</span>
        <ChevronDown className={`size-3.5 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="ml-1 space-y-0.5 pb-1">
          {DATA_LAYERS.map(layer => {
            const active = dataLayers.includes(layer.id)
            return (
              <button
                key={layer.id}
                onClick={() => toggle(layer.id)}
                title={`${layer.tooltip}\n\nFrom zoom ${layer.minzoom}.`}
                aria-pressed={active}
                data-testid={`data-layer-${layer.id}`}
                className="flex w-full items-center gap-2 py-1 px-1 rounded-lg hover:bg-black/5 transition-colors cursor-pointer"
              >
                <span className={active ? 'text-foreground' : 'text-muted-foreground'}>{ICONS[layer.id]}</span>
                <span className={`flex-1 text-left text-xs ${active ? 'text-foreground' : 'text-muted-foreground'}`}>{layer.label}</span>
                <Switch on={active} size="sm" />
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
