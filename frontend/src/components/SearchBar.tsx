import { useState, useRef, useEffect, useCallback } from 'react'

interface SearchResult {
  display_name: string
  secondary: string
  lat: number
  lon: number
}

interface SearchBarProps {
  onSelect: (result: SearchResult) => void
  onIsochronToggle?: () => void
  isochronActive?: boolean
  onSearchInput?: () => void
  mapCenter?: { lat: number; lng: number }
}

export default function SearchBar({ onSelect, onIsochronToggle, isochronActive, onSearchInput, mapCenter }: SearchBarProps) {
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<SearchResult[]>([])
  const [open, setOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [highlighted, setHighlighted] = useState(-1)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  const containerRef = useRef<HTMLDivElement>(null)

  const search = useCallback(async (q: string) => {
    if (q.length < 2) {
      setResults([])
      setOpen(false)
      return
    }

    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller

    setLoading(true)
    try {
      const params = new URLSearchParams({ q })
      if (mapCenter) {
        params.set('lat', mapCenter.lat.toFixed(4))
        params.set('lon', mapCenter.lng.toFixed(4))
      }
      const res = await fetch(`/api/search?${params}`, { signal: controller.signal })
      if (!res.ok) return
      const data: SearchResult[] = await res.json()
      setResults(data)
      setOpen(data.length > 0)
      setHighlighted(-1)
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return
    } finally {
      setLoading(false)
    }
  }, [mapCenter])

  const handleInput = (value: string) => {
    setQuery(value)
    onSearchInput?.()
    if (timerRef.current) clearTimeout(timerRef.current)
    timerRef.current = setTimeout(() => search(value), 150)
  }

  const handleSelect = (result: SearchResult) => {
    const label = result.secondary
      ? `${result.display_name}, ${result.secondary}`
      : result.display_name
    setQuery(label)
    setOpen(false)
    setResults([])
    onSelect(result)
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (!open || results.length === 0) return
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setHighlighted(prev => (prev + 1) % results.length)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setHighlighted(prev => (prev <= 0 ? results.length - 1 : prev - 1))
    } else if (e.key === 'Enter' && highlighted >= 0) {
      e.preventDefault()
      handleSelect(results[highlighted])
    } else if (e.key === 'Escape') {
      setOpen(false)
    }
  }

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [])

  // z-1003 keeps the bar above the z-1002 card column (at z-1000 the layers
  // card once swallowed clicks on the isochron toggle); `.map-top-bar`
  // (index.css) keeps it clear of the column horizontally on md+.
  return (
    <div ref={containerRef} className="map-top-bar top-[calc(env(safe-area-inset-top,0px)+0.75rem)] z-[1003]">
      <div className="relative">
        <input
          id="search"
          name="search"
          type="text"
          role="searchbox"
          autoComplete="off"
          value={query}
          onChange={(e) => handleInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Search address..."
          className="w-full rounded-lg border border-input bg-background px-4 py-2.5 pr-10 text-foreground shadow-lg placeholder:text-muted-foreground focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring/30"
        />
        {loading && (
          <div className="absolute right-10 top-3 text-muted-foreground text-sm">...</div>
        )}
        {onIsochronToggle && (
          <button
            aria-label="Toggle isochron"
            onClick={onIsochronToggle}
            className={`absolute right-2 top-1/2 -translate-y-1/2 p-1.5 rounded-md hover:bg-muted active:bg-accent ${
              isochronActive ? 'text-primary bg-primary/10' : 'text-muted-foreground'
            }`}
            title="Reachability"
          >
            <svg xmlns="http://www.w3.org/2000/svg" className="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="3" />
              <path d="M12 12m-7 0a7 7 0 1 0 14 0a7 7 0 1 0-14 0" strokeDasharray="4 3" />
              <path d="M12 12m-11 0a11 11 0 1 0 22 0a11 11 0 1 0-22 0" strokeDasharray="4 5" />
            </svg>
          </button>
        )}
      </div>
      {open && results.length > 0 && (
        <ul
          role="listbox"
          className="mt-1 max-h-60 overflow-auto rounded-lg border border-border bg-popover shadow-lg"
        >
          {results.map((r, i) => (
            <li
              key={`${r.lat}:${r.lon}:${r.display_name}`}
              role="option"
              aria-selected={i === highlighted}
              onClick={() => handleSelect(r)}
              className={`cursor-pointer px-4 py-2 border-b border-border last:border-b-0 ${
                i === highlighted ? 'bg-accent' : 'hover:bg-accent'
              }`}
            >
              <div className="text-sm font-medium text-popover-foreground truncate">{r.display_name}</div>
              {r.secondary && (
                <div className="text-xs text-muted-foreground truncate">{r.secondary}</div>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
