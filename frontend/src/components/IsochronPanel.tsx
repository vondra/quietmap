import { useState } from 'react'
import { Radar } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { CheckChip } from '@/components/ui/check-chip'

interface IsochronRequest {
  lat: number
  lng: number
  time: number
  modes: string[]
}

interface IsochronPanelProps {
  location: { lat: number; lng: number } | null
  onGo: (request: IsochronRequest) => Promise<void>
  active: boolean
}

export default function IsochronPanel({ location, onGo, active }: IsochronPanelProps) {
  const [walk, setWalk] = useState(true)
  const [car, setCar] = useState(true)
  const [time, setTime] = useState(60)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const handleGo = async () => {
    if (!location || busy) return
    const modes = []
    if (walk) modes.push('walk')
    if (car) modes.push('car')
    if (modes.length === 0) return
    setError(null)
    setBusy(true)
    try {
      await onGo({ lat: location.lat, lng: location.lng, time, modes })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not draw the area.')
    } finally {
      setBusy(false)
    }
  }

  if (!active) return null

  return (
    <div className="
      fixed z-[1003] bg-background rounded-lg shadow-xl border border-border px-3 py-2.5
      left-[calc(50%-50vw+0.75rem)] right-[calc(50%-50vw+0.75rem)] top-[58px]
      md:left-1/2 md:-translate-x-1/2 md:right-auto md:top-[58px] md:w-[calc(100%-1.5rem)] md:max-w-md
    ">
      <div className="flex flex-wrap min-[360px]:flex-nowrap items-center gap-1 sm:gap-1.5 text-sm">
        <div className="flex items-center gap-1 sm:gap-1.5 shrink-0">
          <span className="text-foreground whitespace-nowrap">Reachable in</span>
          <input
            type="number"
            value={time}
            onChange={(e) => setTime(parseInt(e.target.value, 10) || 0)}
            min={1}
            max={120}
            className="w-14 shrink-0 [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none rounded-md border border-input bg-background px-2 py-1 text-sm text-foreground text-center tabular-nums focus:border-ring focus:outline-none focus:ring-0"
          />
          <span className="text-muted-foreground">min</span>
        </div>
        <div className="flex items-center gap-1 sm:gap-1.5 shrink-0">
          <CheckChip checked={walk} label="Walk" onToggle={() => setWalk(!walk)} testId="isochron-walk" />
          <CheckChip checked={car} label="Car" onToggle={() => setCar(!car)} testId="isochron-car" />
          <Button variant="default" size="sm" onClick={() => void handleGo()} disabled={!location || (!walk && !car) || busy || time < 1}>
            <Radar />
            <span className="max-sm:sr-only">{busy ? 'Drawing…' : 'Show area'}</span>
          </Button>
        </div>
      </div>
      {!location && <div className="text-xs text-muted-foreground mt-1">Search for a location first</div>}
      {error && <div role="alert" className="text-xs text-destructive mt-1">{error}</div>}
    </div>
  )
}
