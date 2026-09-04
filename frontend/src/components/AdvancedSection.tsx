import { useState } from 'react'
import { ChevronDown, Mountain, Building, TreePine, Shield } from 'lucide-react'
import { Switch } from './ui/switch'

const OVERLAYS = [
  { id: 'dem', label: 'Elevation', tooltip: 'DEM terrain elevation — hills, valleys, ridges (30m)', icon: <Mountain className="size-3.5" /> },
  { id: 'building-height', label: 'Building heights', tooltip: 'Exact building footprints + heights the noise model screens with (vector obstacle store)', icon: <Building className="size-3.5" /> },
  { id: 'forest', label: 'Forest', tooltip: 'Forest cover from ESA WorldCover (30m)', icon: <TreePine className="size-3.5" /> },
  { id: 'barriers', label: 'Noise barriers', tooltip: 'Noise barriers from OSM (walls blocking sound propagation)', icon: <Shield className="size-3.5" /> },
]

interface AdvancedSectionProps {
  rasterOverlays: Record<string, boolean>
  onRasterOverlayChange: (overlays: Record<string, boolean>) => void
}

export default function AdvancedSection({ rasterOverlays, onRasterOverlayChange }: AdvancedSectionProps) {
  const [open, setOpen] = useState(() => OVERLAYS.some(o => rasterOverlays[o.id]))

  return (
    <div>
      <button
        onClick={() => setOpen(v => !v)}
        title="Toggle raster data overlays — see what feeds the noise computation"
        className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-black/5 transition-colors cursor-pointer text-muted-foreground hover:text-foreground"
      >
        <span className="flex-1 text-left text-[11px] font-medium uppercase tracking-[0.08em]">
          Advanced
        </span>
        <ChevronDown className={`size-3.5 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="ml-1 space-y-0.5 pb-1">
          {OVERLAYS.map(overlay => {
            const active = rasterOverlays[overlay.id] ?? false
            return (
              <button
                key={overlay.id}
                onClick={() => onRasterOverlayChange({ ...rasterOverlays, [overlay.id]: !active })}
                title={overlay.tooltip}
                className="flex w-full items-center gap-2 py-1 px-1 rounded-lg hover:bg-black/5 transition-colors cursor-pointer"
              >
                <span className={active ? 'text-foreground' : 'text-muted-foreground'}>{overlay.icon}</span>
                <span className={`flex-1 text-left text-xs ${active ? 'text-foreground' : 'text-muted-foreground'}`}>
                  {overlay.label}
                </span>
                <Switch on={active} size="sm" />
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
