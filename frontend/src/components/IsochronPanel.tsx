// The area reachable from the open place: in how many minutes, on foot and by car (any of the two),
// drawn on the map (IsochronLayer) by the server's router. Under the open place's row, opened by the
// button at the search field's end.
import { useState } from 'react'
import { CheckChip } from './ui/check-chip'
import { PRIMARY_BUTTON } from './ui/button'

export interface IsochronRequest {
  lat: number
  lng: number
  minutes: number
  modes: ('walk' | 'car')[]
}

export default function IsochronPanel({ at, onShow }: {
  at: { lat: number; lng: number }
  /** Draws the area; rejects with what to tell the visitor. */
  onShow: (request: IsochronRequest) => Promise<void>
}) {
  const [walk, setWalk] = useState(true)
  const [car, setCar] = useState(true)
  const [minutes, setMinutes] = useState(60)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const modes = [...(walk ? ['walk' as const] : []), ...(car ? ['car' as const] : [])]
  const show = async () => {
    setError(null)
    setBusy(true)
    try {
      await onShow({ ...at, minutes, modes })
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'Could not draw the area.')
    } finally {
      setBusy(false)
    }
  }
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
          onChange={event => setMinutes(Math.max(0, Math.min(120, Math.round(Number(event.target.value) || 0))))}
          className="h-8 w-14 rounded-md border border-input bg-background px-1 text-center text-base tabular-nums text-foreground focus:border-ring focus:outline-none pointer-fine:text-sm [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none"
        />
        <span className="text-muted-foreground">min</span>
      </label>
      <div className="flex items-center gap-2">
        <CheckChip checked={walk} label="Walk" onToggle={() => setWalk(!walk)} />
        <CheckChip checked={car} label="Car" onToggle={() => setCar(!car)} />
        <button type="button" className={`${PRIMARY_BUTTON} ml-auto`} disabled={busy || modes.length === 0 || minutes < 1} onClick={() => void show()}>
          {busy ? 'Drawing…' : 'Show area'}
        </button>
      </div>
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    </div>
  )
}
