// The recent places as tabs above the popup: each the place's name and its whole-day loudness, the
// open one marked; a tab reopens its place, its cross forgets it. Kept in this browser only.
import { X } from 'lucide-react'
import { samePlace, type RecentPlace } from '../lib/recent-places'
import { fmtSone } from '../utils/formatters'

/** The place's name up to its first comma, else its coordinates. */
export function placeName(place: RecentPlace): string {
  return place.place?.split(',')[0]?.trim() || `${place.lat.toFixed(3)}, ${place.lng.toFixed(3)}`
}

export default function RecentPlaces({ places, current, onOpen, onRemove }: {
  places: RecentPlace[]
  /** The open popup's point. */
  current: { lat: number, lng: number } | null
  onOpen: (place: RecentPlace) => void
  onRemove: (place: RecentPlace) => void
}) {
  // One place alone has nothing to be compared with.
  if (places.length < 2) return null
  return (
    <nav aria-label="Recent places" data-testid="recent-places" className="pointer-events-auto flex gap-1 overflow-x-auto pb-0.5">
      {places.map(place => {
        const isCurrent = current != null && samePlace(place, current)
        const name = placeName(place)
        return (
          <div
            key={`${place.lat},${place.lng}`}
            className={`flex shrink-0 items-center rounded-md border text-xs shadow-sm ${isCurrent
              ? 'border-foreground bg-foreground text-background'
              : 'border-black/10 bg-white/95 text-foreground'}`}
          >
            <button
              type="button"
              className="flex items-baseline gap-1 py-1 pl-2 pr-1"
              aria-current={isCurrent ? 'page' : undefined}
              title={place.place ?? name}
              onClick={() => onOpen(place)}
            >
              <span className="max-w-[8rem] truncate">{name}</span>
              <b className="tabular-nums">{place.sone != null && place.sone > 0 ? fmtSone(place.sone) : '–'}</b>
            </button>
            <button
              type="button"
              aria-label={`Forget ${name}`}
              className="px-1 py-1 opacity-60 hover:opacity-100"
              onClick={() => onRemove(place)}
            >
              <X className="size-3" />
            </button>
          </div>
        )
      })}
    </nav>
  )
}
