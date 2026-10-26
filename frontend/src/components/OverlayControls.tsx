// The overlay switches: the quiet zones with their threshold slider, and the places to stay with
// the stay they are searched for.
import { useState, useEffect, useRef } from 'react'
import { BedDouble, TreePine } from 'lucide-react'
import { QUIET_THRESHOLDS } from '../hooks/useUrlState'
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
      className="flex w-full items-center gap-2.5 py-1.5 px-1 rounded-lg hover:bg-accent transition-colors cursor-pointer"
    >
      <span className={active ? 'text-foreground' : 'text-muted-foreground'}>{icon}</span>
      <span className={`flex-1 text-left text-sm ${active ? 'text-foreground' : 'text-muted-foreground'}`}>{label}</span>
      <Switch on={active} />
    </button>
  )
}

/** A slider over the quiet thresholds (sone). */
function LoudnessSlider({ value, onChange, testId }: {
  value: number; onChange: (v: number) => void; testId: string
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
  const at = Math.max(0, QUIET_THRESHOLDS.findIndex((t) => t === local))
  return (
    <div className="flex items-center gap-2 ml-7 mt-0.5 mb-1">
      <span className="text-xs text-muted-foreground shrink-0">below</span>
      <input type="range" data-testid={testId} value={at}
        onChange={(e) => setLocal(QUIET_THRESHOLDS[parseInt(e.target.value, 10)])} min={0} max={QUIET_THRESHOLDS.length - 1} step={1}
        className="flex-1 h-1 accent-primary cursor-pointer" />
      <span className="text-xs text-muted-foreground tabular-nums w-14 text-right">{local} sone</span>
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
        tooltip="Highlight areas where the total loudness (all sources) stays below a threshold"
        onClick={() => onQuietClustersChange(!quietClustersEnabled)}
      />
      {quietClustersEnabled && (
        <LoudnessSlider value={quietThreshold} onChange={onQuietThresholdChange} testId="quiet-threshold" />
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
