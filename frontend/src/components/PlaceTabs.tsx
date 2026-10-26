// The open place and the recent places as a browser's tabs at the top of the popup, the newest first,
// each the place's name and its whole-day loudness. A tap on another tab opens its place; a tap on the
// open one renames it there (Enter or a tap elsewhere keeps the name, Escape leaves it as it was, an
// emptied name gives the geocoded one back); the open tab's cross forgets its place. Signed out, a
// person at the end offers the sign-in that keeps them on every device; signed in, a change the server
// did not take shows under them. Kept in this browser or with the account (`hooks/usePlaces.ts`).
import { useRef, useState } from 'react'
import { UserRound, X } from 'lucide-react'
import { placeLabel, samePlace, titleAt, TITLE_MAX, type PlaceTitle, type RecentPlace } from '../lib/place-rules'
import { fmtSone } from '../utils/formatters'

/** The name the visitor gave the place, else its geocoded name up to its first comma, else its
 *  coordinates. */
function placeName(place: RecentPlace, titles: PlaceTitle[]): string {
  return titleAt(titles, place) ?? placeLabel(place.place) ?? `${place.lat.toFixed(3)}, ${place.lng.toFixed(3)}`
}

const keyOf = (place: RecentPlace) => `${place.lat},${place.lng}`

export default function PlaceTabs({ places, titles, current, notSaved, onOpen, onForget, onRename, onSignIn }: {
  /** The recent places with the open one among them, newest first. */
  places: RecentPlace[]
  /** The names the visitor gave places. */
  titles: PlaceTitle[]
  /** The open popup's point. */
  current: { lat: number, lng: number }
  /** The server did not take the last change of a signed-in visitor's places. */
  notSaved: boolean
  onOpen: (place: RecentPlace) => void
  onForget: (place: RecentPlace) => void
  /** A place's new name, or null to give the geocoded one back. */
  onRename: (place: RecentPlace, title: string | null) => void
  /** Opens the sign-in; none while signed in or without the sign-in. */
  onSignIn?: () => void
}) {
  // The name being written: its tab, and the name shown when the writing began, which a geocoded
  // name coming meanwhile does not make a change. Escape leaves the field without keeping it.
  const [draft, setDraft] = useState<{ key: string, from: string, text: string } | null>(null)
  const escaping = useRef(false)
  // One open tab, the first within the place (`withPlace` leaves no second).
  const open = places.findIndex(place => samePlace(place, current))
  return (
    <div>
      <nav aria-label="Places" data-testid="place-tabs" className="flex min-w-0 items-end gap-0.5 border-b border-border">
        {places.map((place, k) => {
          const isOpen = k === open
          const name = placeName(place, titles)
          const writing = isOpen && draft?.key === keyOf(place) ? draft : null
          const keep = () => {
            if (!writing) return
            const written = writing.text.trim()
            setDraft(null)
            if (escaping.current) escaping.current = false
            else if (written && written !== writing.from) onRename(place, written)
            else if (!written && titleAt(titles, place) != null) onRename(place, null)
          }
          return (
            <div
              key={keyOf(place)}
              data-testid="place-tab"
              // Each tab sizes the others' share of the row: a narrow one drops its loudness first.
              className={`@container group relative flex min-w-0 basis-0 items-center rounded-t-md text-xs ${isOpen
                ? '-mb-px h-8 max-w-[14rem] flex-[2] border border-border border-b-white bg-white text-foreground'
                : 'h-7 max-w-[9rem] flex-1 bg-muted text-muted-foreground hover:text-foreground'}`}
            >
              {writing ? (
                // 16 px on a touch screen: iOS zooms the page into a smaller field and keeps it zoomed.
                <input
                  data-testid="place-title"
                  aria-label="Place name"
                  autoFocus
                  className="h-full min-w-0 flex-1 bg-transparent px-2 text-base font-semibold text-foreground outline-none pointer-fine:text-xs"
                  placeholder={placeLabel(place.place) ?? 'Name this place'}
                  value={writing.text}
                  maxLength={TITLE_MAX}
                  enterKeyHint="done"
                  onFocus={event => event.currentTarget.select()}
                  onChange={event => setDraft({ ...writing, text: event.target.value })}
                  onBlur={keep}
                  onKeyDown={event => {
                    if (event.key !== 'Enter' && event.key !== 'Escape') return
                    escaping.current = event.key === 'Escape'
                    event.currentTarget.blur()
                  }}
                />
              ) : (
                <button
                  type="button"
                  aria-current={isOpen ? 'page' : undefined}
                  title={isOpen ? 'Rename this place' : (place.place ?? name)}
                  className={`flex h-full min-w-0 flex-1 items-center gap-1.5 text-left ${isOpen ? 'pl-2 pr-7 font-semibold' : 'px-1.5'}`}
                  onClick={() => (isOpen ? setDraft({ key: keyOf(place), from: name, text: name }) : onOpen(place))}
                >
                  <span className="min-w-0 truncate">{name}</span>
                  <b className={`shrink-0 tabular-nums ${isOpen ? '' : '@max-[4rem]:hidden'}`}>
                    {place.sone != null && place.sone > 0 ? fmtSone(place.sone) : '–'}
                  </b>
                </button>
              )}
              {isOpen && !writing && (
                <button
                  type="button"
                  aria-label={`Forget ${name}`}
                  className="absolute right-0 rounded p-1.5 opacity-60 hover:bg-black/5 hover:opacity-100"
                  onClick={() => onForget(place)}
                >
                  <X className="size-3" />
                </button>
              )}
            </div>
          )
        })}
        {onSignIn && (
          <button
            type="button"
            aria-label="Sign in to keep your places on all your devices"
            title="Sign in to keep your places on all your devices"
            className="mb-0.5 ml-auto grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-black/5 hover:text-foreground"
            onClick={onSignIn}
          >
            <UserRound className="size-4" />
          </button>
        )}
      </nav>
      {notSaved && <p role="alert" className="px-2.5 pt-1 text-xs text-destructive">Not saved: the map could not be reached.</p>}
    </div>
  )
}
