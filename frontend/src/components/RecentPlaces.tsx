// The recent places as one row of tabs above the popup, newest first: each the place's name over its
// whole-day loudness, the open one marked and forgotten by its cross; a tab reopens its place. Kept
// in this browser only.
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
    <nav aria-label="Recent places" data-testid="recent-places" className="pointer-events-auto flex shrink-0 gap-1">
      {places.map(place => {
        const isCurrent = current != null && samePlace(place, current)
        const name = placeName(place)
        return (
          <div
            key={`${place.lat},${place.lng}`}
            className={`relative flex min-w-0 max-w-[9rem] flex-1 basis-0 rounded-md border shadow-sm ${isCurrent
              ? 'border-foreground bg-foreground text-background'
              : 'border-black/10 bg-white/95 text-foreground'}`}
          >
            <button
              type="button"
              className={`flex min-w-0 flex-1 flex-col items-start py-0.5 pl-1.5 text-left ${isCurrent ? 'pr-5' : 'pr-1.5'}`}
              aria-current={isCurrent ? 'page' : undefined}
              title={place.place ?? name}
              onClick={() => onOpen(place)}
            >
              <span className="w-full truncate text-[10px] leading-tight opacity-80">{name}</span>
              <b className="text-xs leading-tight tabular-nums">{place.sone != null && place.sone > 0 ? fmtSone(place.sone) : '–'}</b>
            </button>
            {isCurrent && (
              <button
                type="button"
                aria-label={`Forget ${name}`}
                className="absolute right-0 top-0 p-1 opacity-60 hover:opacity-100"
                onClick={() => onRemove(place)}
              >
                <X className="size-3" />
              </button>
            )}
          </div>
        )
      })}
    </nav>
  )
}
