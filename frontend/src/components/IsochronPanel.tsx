// The area reachable from the open place: in how many minutes, on foot and by car (any of the two),
// drawn on the map (IsochronLayer) by the server's router. Under the open place's row, opened by the
// button at the search field's end.
import type { ReachOptions, ReachRequest } from '../lib/url-state'
import { CheckChip } from './ui/check-chip'
import { PRIMARY_BUTTON } from './ui/button'

export default function IsochronPanel({ at, options, onChange, onShow, busy, error }: {
  at: { lat: number; lng: number }
  options: ReachOptions
  onChange: (options: ReachOptions) => void
  onShow: (request: ReachRequest) => void
  busy: boolean
  error: string | null
}) {
  const { minutes, modes } = options
  const toggle = (mode: 'walk' | 'car') => onChange({ ...options,
    modes: (['walk', 'car'] as const).filter(each => each === mode ? !modes.includes(each) : modes.includes(each)) })
  return (
    // Two lines, so that a narrow card never splits Walk from Car.
    <div data-testid="isochron-panel" className="flex flex-col gap-2 border-t border-border px-3 py-2 text-sm">
      <label className="flex items-center gap-1.5">
        Reachable in
        {/* 16 px on a touch screen: iOS zooms into a smaller field. */}
        <input
          type="number"
          aria-label="Minutes"
          value={minutes}
          min={1}
          max={120}
          onChange={event => onChange({ ...options, minutes: Math.max(0, Math.min(120, Math.round(Number(event.target.value) || 0))) })}
          className="h-8 w-14 rounded-md border border-input bg-background px-1 text-center text-base tabular-nums text-foreground focus:border-ring focus:outline-none pointer-fine:text-sm [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none"
        />
        <span className="text-muted-foreground">min</span>
      </label>
      <div className="flex items-center gap-2">
        <CheckChip checked={modes.includes('walk')} label="Walk" onToggle={() => toggle('walk')} />
        <CheckChip checked={modes.includes('car')} label="Car" onToggle={() => toggle('car')} />
        <button type="button" className={`${PRIMARY_BUTTON} ml-auto`} disabled={busy || modes.length === 0 || minutes < 1} onClick={() => onShow({ ...at, ...options })}>
          {busy ? 'Drawing…' : 'Show area'}
        </button>
      </div>
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    </div>
  )
}
