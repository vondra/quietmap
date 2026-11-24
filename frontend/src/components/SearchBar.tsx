// The search field and what drops down under it, as on a web map: with nothing typed, the history
// of places opened (clicked or searched, the one opened last first); typed, the places of the
// history it names, then the geocoder's suggestions leaning towards the map's view. The field keeps
// the focus while the arrows move through the list: Enter opens, Shift+Delete forgets, Escape, Tab
// or a press elsewhere closes it (a press on the map does nothing more). While it is closed the
// open place stands under the field, as the list's selected item, its name the field that renames
// it, with its action and what that opens (App: the reachable area).
import { useState, useRef, useEffect, type ReactNode } from 'react'
import type { Metric } from '../lib/hm3-decoder'
import { History, Search, UserRound } from 'lucide-react'
import PlaceRow from './PlaceRow'
import { FLOATING } from './ui/button'
import { placeLabel, samePlace, titleAt, type PlaceTitle, type RecentPlace } from '../lib/place-rules'
import { coordinateSuggestions, MAX_LATITUDE, type SearchResult } from '../lib/coordinates'

type Point = { lat: number; lng: number }

/** The places of the history a query names, at most. */
const NAMED_MAX = 3
/** About six rows of the list in sight (px), the rest scrolled within it. */
const LIST_MAX_PX = 336

interface SearchBarProps {
  onSelect: (at: Point, name: string | null) => void
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
  /** The reachable-area action at the field's end, where Google's directions stand. */
  fieldAction?: ReactNode
  /** The phone's account, at the field's end whatever is open (Google Maps' place for it). */
  accountAction?: ReactNode
  /** Under the open place's row (or under the search field when a reachable area is active): what its actions opened. */
  children?: ReactNode
}

/** Text compared without case and accents. */
const folded = (text: string) => text.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLowerCase()
const keyOf = (at: Point) => `${at.lat},${at.lng}`

export default function SearchBar({ onSelect, getMapView, history, titles, metric, open, notSaved, onOpenPlace, onRename, onForget, onSignIn, openAction, fieldAction, accountAction, children }: SearchBarProps) {
  const [query, setQuery] = useState('')
  const [suggestions, setSuggestions] = useState<{ query: string, results: SearchResult[] } | null>(null)
  // The visitor is at the field (it took the focus or the keys): the list shows what it has.
  const [listing, setListing] = useState(false)
  const [rowIndex, setRowIndex] = useState(-1)
  // The list's height above a phone's keyboard (the visual viewport shrinks under it, the layout's not).
  const [room, setRoom] = useState(LIST_MAX_PX)
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
  const coordinates = coordinateSuggestions(asked)
  const places = coordinates ? [] : asked
    ? history.filter(place => {
      const { name, detail } = described(place, place.place)
      return folded(`${name} ${detail ?? ''}`).includes(folded(asked))
    }).slice(0, NAMED_MAX)
    : history
  const searchNames = asked.length >= 2 && coordinates === null
  const shownResults = coordinates ?? (searchNames && suggestions?.query === asked ? suggestions.results : [])
  const count = places.length + shownResults.length
  const highlighted = coordinates?.length ? 0 : rowIndex
  const listed = listing && count > 0
  const showingSearch = listed || (listing && asked !== '')

  useEffect(() => {
    if (!searchNames) return
    const controller = new AbortController()
    const timer = setTimeout(async () => {
      const params = new URLSearchParams({ q: asked })
      const view = getMapView?.()
      if (view) {
        params.set('lat', view.lat.toFixed(4))
        params.set('lon', view.lng.toFixed(4))
        params.set('zoom', view.zoom.toFixed(1))
      }
      let results: SearchResult[] = []
      try {
        const res = await fetch(`/api/search?${params}`, { signal: controller.signal })
        if (res.ok) results = await res.json() as SearchResult[]
      } catch { /* The geocoder could not answer this query. */ }
      if (!controller.signal.aborted) setSuggestions({ query: asked, results })
    }, 150)
    return () => {
      clearTimeout(timer)
      controller.abort()
    }
  }, [asked, searchNames, getMapView])

  const handleInput = (value: string) => {
    setQuery(value)
    setListing(true)
    setRowIndex(-1)
  }

  // A choice closes the list and leaves the field, which a phone's keyboard leaves with it; a search
  // still on its way is dropped.
  const done = () => {
    setQuery('')
    setSuggestions(null)
    setListing(false)
    setRowIndex(-1)
    inputRef.current?.blur()
  }
  const choose = (k: number) => {
    if (k < places.length) onOpenPlace(places[k])
    else {
      const result = shownResults[k - places.length]
      // A coordinate gets its name from the same reverse lookup as a map click.
      const name = coordinates ? null : [result.display_name, result.secondary].filter(Boolean).join(', ')
      onSelect({ lat: result.lat, lng: result.lon }, name)
    }
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
      setRowIndex(prev => (count === 0 ? -1 : (prev + 1) % count))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setRowIndex(prev => (prev <= 0 ? count - 1 : prev - 1))
    } else if (e.key === 'Enter' && highlighted >= 0 && highlighted < count) {
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
      <div ref={containerRef} className={`${FLOATING} map-top-bar top-[calc(env(safe-area-inset-top,0px)+0.75rem)] z-[2003]`}>
        <div className="relative">
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
            placeholder={open ? 'Search another place…' : 'Search address or coordinates…'}
            className={`w-full rounded-lg bg-background px-4 py-2.5 pr-10 ${fieldAction ? 'max-md:pr-[4.75rem]' : ''} text-base text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring/30 pointer-fine:text-sm`}
          />
          <span id="search-keys" hidden>Enter coordinates as latitude, longitude. Up and down choose a place, Enter opens it, Shift+Delete forgets it.</span>
          <div className="absolute right-1.5 top-1/2 flex -translate-y-1/2 items-center gap-0.5">
            {listing && searchNames && suggestions?.query !== asked && <span className="px-1 text-sm text-muted-foreground">...</span>}
            {!showingSearch && fieldAction}
            {!showingSearch && accountAction}
          </div>
        </div>
        {listed ? (
          <div
            className="overflow-hidden rounded-b-lg border-t border-border bg-popover"
            onMouseDown={(e) => {
              if (!(e.target instanceof HTMLInputElement)) e.preventDefault()
            }}
          >
            <ul ref={listRef} id="search-list" role="listbox" aria-label="Places" className="overflow-y-auto overscroll-contain" style={{ maxHeight: room }}>
              {places.map((place, k) => (
                <li key={keyOf(place)} id={`search-option-${k}`} role="option" aria-selected={k === highlighted} onMouseEnter={() => setRowIndex(k)}>
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
                    onMouseEnter={() => setRowIndex(k)}
                    // The geocoder's answers under the history's places, a line between them.
                    className={i === 0 && places.length > 0 ? 'border-t border-border' : undefined}
                  >
                    <PlaceRow
                      listed
                      at={{ lat: r.lat, lng: r.lon }}
                      name={r.display_name}
                      given={false}
                      proposal={null}
                      detail={coordinates ? 'Latitude, longitude' : r.secondary}
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
        ) : listing && coordinates?.length === 0 ? (
          <p role="status" className="rounded-b-lg border border-border bg-popover px-3 py-2 text-xs text-muted-foreground">
            Use latitude −{MAX_LATITUDE} to {MAX_LATITUDE} and longitude −180 to 180.
          </p>
        ) : open && !showingSearch ? (
          <>
            {/* Made anew for each place: a name being written belongs to the place it was begun for. Its
                key names the row alone: siblings under one key once left a row behind at every place. */}
            <div key={`open:${keyOf(open.at)}`} data-testid="open-place" className="overflow-hidden rounded-b-lg border-t border-border bg-popover">
              <PlaceRow
                {...described(open.at, open.place)}
                metric={metric}
                sone={open.sone}
                lden={open.lden}
                highlighted
                onRename={onRename}
                action={openAction}
              />
              {/* Scrolled within what is left above a phone's sheet (MobileDetailSheet's sheetTop) under a
                  notch too, and never shut on a short screen; a desktop has no
                  sheet. */}
              <div className="max-h-[max(6rem,calc(60dvh-11rem-env(safe-area-inset-top,0px)))] overflow-y-auto overscroll-contain md:max-h-[calc(100dvh-10rem)]">{children}</div>
            </div>
          </>
        ) : !showingSearch && children && (
          <div className="overflow-hidden rounded-b-lg bg-popover">{children}</div>
        )}
        {notSaved && <p role="alert" className="mt-1 rounded-md bg-popover px-3 py-1 text-xs text-destructive shadow-lg">Not saved: the map could not be reached.</p>}
      </div>
    </>
  )
}
