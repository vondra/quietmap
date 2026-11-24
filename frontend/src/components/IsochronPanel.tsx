// The area reachable from a place: in how many minutes, on foot or by car (one at a time), drawn on
// the map (IsochronLayer) by the server's router. While an area is shown, changing its minutes or mode
// redraws it at its own origin; another clicked point is offered explicitly as "From here".
import { useState } from 'react'
import type { ReachOptions, ReachRequest } from '../lib/url-state'
import { CheckChip } from './ui/check-chip'
import { PRIMARY_BUTTON } from './ui/button'

export default function IsochronPanel({ at, from, atOtherPlace, options, onChange, onShow, busy, error }: {
  at: { lat: number; lng: number }
  from: string | null
  atOtherPlace: boolean
  options: ReachOptions
  onChange: (options: ReachOptions) => void
  onShow: (request: ReachRequest) => void
  busy: boolean
  error: string | null
}) {
  const { minutes, mode } = options
  const [draft, setDraft] = useState<string | null>(null)
  return (
    // Two lines, so that a narrow card never splits Walk from Car.
    <div data-testid="isochron-panel" className="flex flex-col gap-2 border-t border-border px-3 py-2 text-sm">
      <div className="flex items-center gap-1.5">
        <label className="flex shrink-0 items-center gap-1.5">
          Reachable in
          {/* 16 px on a touch screen: iOS zooms into a smaller field. */}
          <input
            type="number"
            aria-label="Minutes"
            value={draft ?? minutes}
            min={1}
            max={120}
            onChange={event => {
              const raw = event.target.value
              if (raw.trim() === '') {
                setDraft('')
                return
              }
              const parsed = Math.round(Number(raw))
              if (!Number.isFinite(parsed) || parsed < 1) {
                setDraft(raw)
                return
              }
              setDraft(null)
              onChange({ ...options, minutes: Math.min(120, parsed) })
            }}
            onBlur={() => setDraft(null)}
            className="h-8 w-14 rounded-md border border-input bg-background px-1 text-center text-base tabular-nums text-foreground focus:border-ring focus:outline-none pointer-fine:text-sm [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none"
          />
          <span className="text-muted-foreground">min</span>
        </label>
        {from && <span data-testid="isochron-origin" className="ml-auto min-w-0 truncate text-xs text-muted-foreground">from {from}</span>}
      </div>
      <div className="flex items-center gap-2">
        <CheckChip checked={mode === 'walk'} label="Walk" onToggle={() => onChange({ ...options, mode: 'walk' })} />
        <CheckChip checked={mode === 'car'} label="Car" onToggle={() => onChange({ ...options, mode: 'car' })} />
        <button type="button" className={`${PRIMARY_BUTTON} ml-auto`} disabled={busy || minutes < 1} onClick={() => onShow({ ...at, ...options })}>
          {busy ? 'Drawing…' : atOtherPlace ? 'From here' : 'Show area'}
        </button>
      </div>
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    </div>
  )
}
