// The search field and what drops down under it, as on a web map: with nothing typed, the history
// of places opened (clicked or searched, the one opened last first); typed, the places of the
// history it names, then the geocoder's suggestions leaning towards the map's view. The field keeps
// the focus while the arrows move through the list: Enter opens, Shift+Delete forgets, Escape, Tab
// or a press elsewhere closes it (a press on the map does nothing more). While it is closed the
// open place stands under the field, as the list's selected item, its name the field that renames
// it, with its action and what that opens (App: the reachable area).
import { useState, useRef, useEffect, useCallback, type ReactNode } from 'react'
import type { Metric } from '../lib/hm3-decoder'
import { History, Search, UserRound } from 'lucide-react'
import PlaceRow from './PlaceRow'
import { FLOATING } from './ui/button'
import { placeLabel, samePlace, titleAt, type PlaceTitle, type RecentPlace } from '../lib/place-rules'

interface SearchResult {
  display_name: string
  secondary: string
  lat: number
  lon: number
}

type Point = { lat: number; lng: number }

/** The places of the history a query names, at most. */
const NAMED_MAX = 3
/** About six rows of the list in sight (px), the rest scrolled within it. */
const LIST_MAX_PX = 336

interface SearchBarProps {
  onSelect: (result: SearchResult) => void
  /** The map's view when the visitor types (read then, not at the last render). */
  getMapView?: () => { lat: number; lng: number; zoom: number }
  /** The places opened, in the history's order, and the names the visitor gave places. */
  history: RecentPlace[]
  titles: PlaceTitle[]
  /** The quantity the places' rows show. */
  metric: Metric
  /** The open place: its point, its name as known and its loudness and Lden, once known. */
  open: { at: Point; place: string | null; sone: number | null; lden: number | null } | null
  /** The server did not take the last change of a signed-in visitor's places. */
  notSaved: boolean
  onOpenPlace: (place: RecentPlace) => void
  onRename: (at: Point, title: string | null) => void
  onForget: (place: RecentPlace) => void
  /** Offers the sign-in under the history; none while signed in or without the sign-in. */
  onSignIn?: () => void
  /** The open place's actions, at its row's end. */
  openAction?: ReactNode
  /** The open place's action at the field's end, where Google's directions stand (the reachable area). */
  fieldAction?: ReactNode
  /** Under the open place's row: what its actions opened. */
  children?: ReactNode
}

/** Text compared without case and accents. */
const folded = (text: string) => text.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLowerCase()
const keyOf = (at: Point) => `${at.lat},${at.lng}`

export default function SearchBar({ onSelect, getMapView, history, titles, metric, open, notSaved, onOpenPlace, onRename, onForget, onSignIn, openAction, fieldAction, children }: SearchBarProps) {
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<SearchResult[]>([])
  // The visitor is at the field (it took the focus or the keys): the list shows what it has.
  const [listing, setListing] = useState(false)
  const [loading, setLoading] = useState(false)
  const [highlighted, setHighlighted] = useState(-1)
  // The list's height above a phone's keyboard (the visual viewport shrinks under it, the layout's not).
  const [room, setRoom] = useState(LIST_MAX_PX)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLUListElement>(null)

  /** A place's name, and where it is: the rest of its geocoded name, or all of it under a given name. */
  const described = (at: Point, place: string | null) => {
    const given = titleAt(titles, at)?.title ?? null
    const label = placeLabel(place)
    return {
      at,
      name: given ?? label ?? `${at.lat.toFixed(4)}, ${at.lng.toFixed(4)}`,
      given: given != null,
      proposal: label,
      detail: given != null ? place : place?.split(',').slice(1).join(',').trim() || null,
    }
  }
  const asked = query.trim()
  const places = asked
    ? history.filter(place => {
      const { name, detail } = described(place, place.place)
      return folded(`${name} ${detail ?? ''}`).includes(folded(asked))
    }).slice(0, NAMED_MAX)
    : history
  const shownResults = asked.length >= 2 ? results : []
  const count = places.length + shownResults.length
  const listed = listing && count > 0

  const search = useCallback(async (q: string) => {
    if (q.length < 2) {
      setResults([])
      return
    }

    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller

    setLoading(true)
    try {
      const params = new URLSearchParams({ q })
      const view = getMapView?.()
      if (view) {
        params.set('lat', view.lat.toFixed(4))
        params.set('lon', view.lng.toFixed(4))
        params.set('zoom', view.zoom.toFixed(1))
      }
      const res = await fetch(`/api/search?${params}`, { signal: controller.signal })
      if (!res.ok) return
      setResults(await res.json() as SearchResult[])
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return
    } finally {
      setLoading(false)
    }
  }, [getMapView])

  const handleInput = (value: string) => {
    setQuery(value)
    setListing(true)
    setHighlighted(-1)
    if (timerRef.current) clearTimeout(timerRef.current)
    timerRef.current = setTimeout(() => search(value.trim()), 150)
  }

  // A choice closes the list and leaves the field, which a phone's keyboard leaves with it; a search
  // still on its way is dropped.
  const done = () => {
    if (timerRef.current) clearTimeout(timerRef.current)
    abortRef.current?.abort()
    setQuery('')
    setResults([])
    setListing(false)
    setHighlighted(-1)
    inputRef.current?.blur()
  }
  const choose = (k: number) => {
    if (k < places.length) onOpenPlace(places[k])
    else onSelect(shownResults[k - places.length])
    done()
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      setListing(false)
      return
    }
    if (!listed && e.key !== 'ArrowDown') return
    const place = highlighted >= 0 && highlighted < places.length ? places[highlighted] : null
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setListing(true)
      setHighlighted(prev => (count === 0 ? -1 : (prev + 1) % count))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setHighlighted(prev => (prev <= 0 ? count - 1 : prev - 1))
    } else if (e.key === 'Enter' && highlighted >= 0) {
      e.preventDefault()
      choose(highlighted)
    } else if (e.key === 'Delete' && e.shiftKey && place) {
      e.preventDefault()
      onForget(place)
    }
  }

  // The row the keys reach stays in the list's sight.
  useEffect(() => {
    if (highlighted >= 0) listRef.current?.children[highlighted]?.scrollIntoView({ block: 'nearest' })
  }, [highlighted])

  // How low the bar reaches with what stands under the field: a phone's map credits stand under it
  // (index.css).
  useEffect(() => {
    const bar = containerRef.current
    if (!bar) return
    const observer = new ResizeObserver(() =>
      document.documentElement.style.setProperty('--map-top-bar-bottom', `${bar.getBoundingClientRect().bottom}px`))
    observer.observe(bar)
    return () => {
      observer.disconnect()
      document.documentElement.style.removeProperty('--map-top-bar-bottom')
    }
  }, [])

  // The list keeps above a phone's keyboard, the sign-in line under it too.
  useEffect(() => {
    const viewport = window.visualViewport
    if (!listed || !viewport) return
    const fit = () => setRoom(Math.max(96, Math.min(LIST_MAX_PX,
      viewport.offsetTop + viewport.height - (listRef.current?.getBoundingClientRect().top ?? 0) - 48)))
    fit()
    viewport.addEventListener('resize', fit)
    return () => viewport.removeEventListener('resize', fit)
  }, [listed])

  return (
    <>
      {/* A press anywhere else while the list is open only closes it (the field's blur, or the click):
          the map under it takes nothing, a phone's click after the touch included. */}
      {listed && <div className="fixed inset-0 z-[2001]" onClick={() => setListing(false)} />}
      {/* Above the card column and the phone's sheets, under the account's form at the top right;
          `.map-top-bar` (index.css) keeps it clear of the column horizontally on md+. */}
      <div ref={containerRef} className="map-top-bar top-[calc(env(safe-area-inset-top,0px)+0.75rem)] z-[2003]">
        {/* On a phone the field leaves the account's button its room; the rows under it take the width. */}
        <div className="relative max-md:mr-[calc(2.75rem+var(--map-gutter))]">
          <input
            ref={inputRef}
            id="search"
            name="search"
            type="text"
            role="combobox"
            aria-label="Search address"
            aria-autocomplete="list"
            aria-expanded={listed}
            aria-controls="search-list"
            aria-activedescendant={listed && highlighted >= 0 ? `search-option-${highlighted}` : undefined}
            aria-describedby="search-keys"
            autoComplete="off"
            value={query}
            onChange={(e) => handleInput(e.target.value)}
            onFocus={() => setListing(true)}
            onClick={() => setListing(true)}
            // Leaving the field by the keys closes the list; a press inside the list keeps the focus here.
            onBlur={(e) => {
              if (!containerRef.current?.contains(e.relatedTarget)) setListing(false)
            }}
            onKeyDown={handleKeyDown}
            placeholder="Search address..."
            className={`${FLOATING} w-full px-4 py-2.5 pr-10 placeholder:text-muted-foreground focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring/30`}
          />
          <span id="search-keys" hidden>Up and down choose a place, Enter opens it, Shift+Delete forgets it.</span>
          {loading && (
            <div className="absolute right-10 top-3 text-muted-foreground text-sm">...</div>
          )}
          {open && !listed && <div className="absolute right-1.5 top-1/2 -translate-y-1/2">{fieldAction}</div>}
        </div>
        {listed ? (
          <div
            className={`${FLOATING} mt-1 overflow-hidden bg-popover`}
            onMouseDown={(e) => {
              if (!(e.target instanceof HTMLInputElement)) e.preventDefault()
            }}
          >
            <ul ref={listRef} id="search-list" role="listbox" aria-label="Places" className="overflow-y-auto overscroll-contain" style={{ maxHeight: room }}>
              {places.map((place, k) => (
                <li key={keyOf(place)} id={`search-option-${k}`} role="option" aria-selected={k === highlighted} onMouseEnter={() => setHighlighted(k)}>
                  <PlaceRow
                    listed
                    {...described(place, place.place)}
                    metric={metric}
                    sone={place.sone}
                    lden={place.lden}
                    highlighted={k === highlighted}
                    onOpen={() => choose(k)}
                    onForget={() => onForget(place)}
                  >
                    {/* The open place by its pin, the others as the history. */}
                    {open != null && samePlace(place, open.at) ? undefined : <History className="size-4 shrink-0 text-muted-foreground" />}
                  </PlaceRow>
                </li>
              ))}
              {shownResults.map((r, i) => {
                const k = places.length + i
                return (
                  <li
                    key={`${r.lat}:${r.lon}:${r.display_name}`}
                    id={`search-option-${k}`}
                    role="option"
                    aria-selected={k === highlighted}
                    onMouseEnter={() => setHighlighted(k)}
                    // The geocoder's answers under the history's places, a line between them.
                    className={i === 0 && places.length > 0 ? 'border-t border-border' : undefined}
                  >
                    <PlaceRow
                      listed
                      at={{ lat: r.lat, lng: r.lon }}
                      name={r.display_name}
                      given={false}
                      proposal={null}
                      detail={r.secondary}
                      metric={metric}
                      sone={null}
                      lden={null}
                      highlighted={k === highlighted}
                      onOpen={() => choose(k)}
                    >
                      <Search className="size-4 shrink-0 text-muted-foreground" />
                    </PlaceRow>
                  </li>
                )
              })}
            </ul>
            {onSignIn && !asked && (
              <button
                type="button"
                tabIndex={-1}
                // A row's look: the same icon column and the same size as the places above it.
                className="flex min-h-11 w-full items-center gap-1 border-t border-border pr-1 text-left text-sm text-muted-foreground hover:bg-accent hover:text-foreground"
                onClick={() => {
                  setListing(false)
                  onSignIn()
                }}
              >
                <span className="grid place-items-center pl-3 pr-2"><UserRound className="size-4 shrink-0" /></span>
                Sign in to keep these places on all your devices
              </button>
            )}
          </div>
        ) : open && (
          <>
            {/* Made anew for each place: a name being written belongs to the place it was begun for. Its
                key names the row alone: siblings under one key once left a row behind at every place. */}
            <div key={`open:${keyOf(open.at)}`} data-testid="open-place" className={`${FLOATING} mt-1 overflow-hidden bg-popover`}>
              <PlaceRow
                {...described(open.at, open.place)}
                metric={metric}
                sone={open.sone}
                lden={open.lden}
                highlighted
                onOpen={() => inputRef.current?.focus()}
                onRename={onRename}
                action={openAction}
              />
              {/* Scrolled within what is left above a phone's sheet (MobileDetailSheet's sheetTop) under a
                  notch too, the map's credits under it, and never shut on a short screen; a desktop has no
                  sheet. */}
              <div className="max-h-[max(6rem,calc(50dvh-11rem-env(safe-area-inset-top,0px)))] overflow-y-auto overscroll-contain md:max-h-[calc(100dvh-10rem)]">{children}</div>
            </div>
          </>
        )}
        {notSaved && <p role="alert" className="mt-1 rounded-md bg-popover px-3 py-1 text-xs text-destructive shadow-lg">Not saved: the map could not be reached.</p>}
      </div>
    </>
  )
}
