// The overlay switches: the quiet zones with their threshold slider, and the places to stay with
// the stay they are searched for.
import { useState, useEffect, useRef } from 'react'
import { BedDouble, TreePine } from 'lucide-react'
import { QUIET_THRESHOLD_MIN, QUIET_THRESHOLD_MAX, QUIET_THRESHOLD_STEP } from '../hooks/useUrlState'
import type { StaySearch } from '../lib/stays'
import StaySearchFields from './StaySearchFields'
import { Switch } from './ui/switch'

export interface OverlayControlsProps {
  quietClustersEnabled: boolean
  onQuietClustersChange: (enabled: boolean) => void
  quietThreshold: number
  onQuietThresholdChange: (threshold: number) => void
  staysOn: boolean
  onStaysChange: (on: boolean) => void
  staySearch: StaySearch
  onStaySearchChange: (search: StaySearch) => void
}

function ToggleRow({ active, icon, label, tooltip, onClick }: {
  active: boolean; icon: React.ReactNode; label: string; tooltip: string; onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      title={tooltip}
      aria-pressed={active}
      className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-black/5 transition-colors cursor-pointer"
    >
      <span className={active ? 'text-foreground' : 'text-muted-foreground'}>{icon}</span>
      <span className={`flex-1 text-left text-sm ${active ? 'text-foreground' : 'text-muted-foreground'}`}>{label}</span>
      <Switch on={active} />
    </button>
  )
}

function NoiseSlider({ value, onChange, min, max, step = 1, testId }: {
  value: number; onChange: (v: number) => void; min: number; max: number; step?: number; testId: string
}) {
  const [local, setLocal] = useState(value)
  const onChangeRef = useRef(onChange)
  onChangeRef.current = onChange
  useEffect(() => { setLocal(value) }, [value])
  useEffect(() => {
    if (local === value) return
    const t = setTimeout(() => onChangeRef.current(local), 300)
    return () => clearTimeout(t)
  }, [local, value])
  return (
    <div className="flex items-center gap-2 ml-7 mt-0.5 mb-1">
      <span className="text-[11px] text-muted-foreground shrink-0">below</span>
      <input type="range" data-testid={testId} value={local}
        onChange={(e) => setLocal(parseFloat(e.target.value))} min={min} max={max} step={step}
        className="flex-1 h-1 accent-primary cursor-pointer" />
      <span className="text-[11px] text-muted-foreground tabular-nums w-12 text-right">{local} dB</span>
    </div>
  )
}

export default function OverlayControls({
  quietClustersEnabled, onQuietClustersChange,
  quietThreshold, onQuietThresholdChange,
  staysOn, onStaysChange, staySearch, onStaySearchChange,
}: OverlayControlsProps) {
  return (
    <div>
      <ToggleRow
        active={quietClustersEnabled}
        icon={<TreePine className="size-4" />}
        label="Quiet zones"
        tooltip="Highlight areas where total noise (all sources) stays below a threshold"
        onClick={() => onQuietClustersChange(!quietClustersEnabled)}
      />
      {quietClustersEnabled && (
        <NoiseSlider value={quietThreshold} onChange={onQuietThresholdChange} min={QUIET_THRESHOLD_MIN} max={QUIET_THRESHOLD_MAX} step={QUIET_THRESHOLD_STEP} testId="quiet-threshold" />
      )}
      <ToggleRow
        active={staysOn}
        icon={<BedDouble className="size-4" />}
        label="Places to stay"
        tooltip="Hotels and apartments with a room for your dates and their prices (Stay22); click one for its noise"
        onClick={() => onStaysChange(!staysOn)}
      />
      {staysOn && <StaySearchFields search={staySearch} onChange={onStaySearchChange} />}
    </div>
  )
}
