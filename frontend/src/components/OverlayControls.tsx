// The overlay switches: the quiet zones with their threshold slider, and the places to stay with
// the stay they are searched for.
import { BedDouble, TreePine } from 'lucide-react'
import { QUIET_THRESHOLDS } from '../lib/url-state'
import type { Metric } from '../lib/hm3-decoder'
import type { StaySearch } from '../lib/stays'
import StaySearchFields from './StaySearchFields'
import { BUTTON } from './ui/button'
import { Switch } from './ui/switch'

export interface OverlayControlsProps {
  /** The quantity shown: the quiet threshold is in its unit. */
  metric: Metric
  quietClustersEnabled: boolean
  onQuietClustersChange: (enabled: boolean) => void
  quietThreshold: number
  onQuietThresholdChange: (threshold: number) => void
  staysOn: boolean
  onStaysChange: (on: boolean) => void
  stayMaxPrice: number | null
  onStayMaxPriceChange: (price: number | null) => void
  reachLabel: string | null
  onClearReach: () => void
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

/** A slider over the quiet thresholds of `metric` (sone or dB). */
function LoudnessSlider({ metric, value, onChange, testId }: {
  metric: Metric; value: number; onChange: (v: number) => void; testId: string
}) {
  const thresholds = QUIET_THRESHOLDS[metric]
  const at = Math.max(0, thresholds.findIndex(t => t === value))
  return (
    <div className="flex items-center gap-2 ml-7 mt-0.5 mb-1">
      <span className="text-xs text-muted-foreground shrink-0">below</span>
      <input type="range" data-testid={testId} value={at}
        onChange={(e) => onChange(thresholds[parseInt(e.target.value, 10)])} min={0} max={thresholds.length - 1} step={1}
        className="flex-1 h-1 accent-primary cursor-pointer" />
      <span className="text-xs text-muted-foreground tabular-nums w-14 text-right">{value} {metric === 'lden' ? 'dB' : 'sone'}</span>
    </div>
  )
}

export default function OverlayControls({
  metric, quietClustersEnabled, onQuietClustersChange,
  quietThreshold, onQuietThresholdChange,
  staysOn, onStaysChange, staySearch, onStaySearchChange, stayMaxPrice, onStayMaxPriceChange, reachLabel, onClearReach,
}: OverlayControlsProps) {
  return (
    <div>
      <ToggleRow
        active={quietClustersEnabled}
        icon={<TreePine className="size-4" />}
        label="Quiet zones"
        tooltip="Highlight areas below the total-noise threshold; places to stay are filtered by the same limit"
        onClick={() => onQuietClustersChange(!quietClustersEnabled)}
      />
      {quietClustersEnabled && (
        // Keyed by the quantity: a value dragged in sone must not land after the switch to dB.
        <LoudnessSlider key={metric} metric={metric} value={quietThreshold} onChange={onQuietThresholdChange} testId="quiet-threshold" />
      )}
      <ToggleRow
        active={staysOn}
        icon={<BedDouble className="size-4" />}
        label="Places to stay"
        tooltip="Hotels and apartments with a room for your dates and their prices; click one for its noise"
        onClick={() => onStaysChange(!staysOn)}
      />
      {staysOn && <StaySearchFields search={staySearch} onChange={onStaySearchChange} maxPrice={stayMaxPrice} onMaxPriceChange={onStayMaxPriceChange} />}
      {reachLabel && (
        <div className="ml-7 flex items-center gap-2 py-1 text-xs" data-testid="stay-reach">
          <span className="min-w-0 flex-1 text-muted-foreground">{reachLabel}</span>
          <button type="button" aria-label="Clear reachable area" onClick={onClearReach}
            className={BUTTON}>Clear</button>
        </div>
      )}
    </div>
  )
}
