// One public state for controls and address, written at once: the address is the place's link.
import { useCallback, useEffect, useRef, useState } from 'react'
import { buildHash, parseHash, sameDetailPosition, type UrlState } from '../lib/url-state'
export type UrlUpdate = Partial<UrlState> | ((state: UrlState) => Partial<UrlState>)

export function useUrlState() {
  const [state, setState] = useState(() => parseHash())
  const latest = useRef(state)
  const updateUrl = useCallback((patch: UrlUpdate) => {
    const next = { ...latest.current, ...(typeof patch === 'function' ? patch(latest.current) : patch) }
    latest.current = next
    const hash = buildHash(next)
    if (window.location.hash !== hash) window.history.replaceState(null, '', hash)
    setState(next)
  }, [])
  useEffect(() => {
    const read = () => {
      const next = parseHash()
      if (buildHash(next) === buildHash(latest.current)) return
      if (sameDetailPosition(next.detailPosition, latest.current.detailPosition)) next.detailPosition = latest.current.detailPosition
      if (next.reach && latest.current.reach && sameDetailPosition(next.reach, latest.current.reach)) {
        next.reach = { ...next.reach, lat: latest.current.reach.lat, lng: latest.current.reach.lng }
      }
      for (const key of ['padding', 'staySearch', 'reach', 'heatmapLayers', 'dataLayers', 'popup', 'scroll'] as const) {
        if (JSON.stringify(next[key]) === JSON.stringify(latest.current[key])) Object.assign(next, { [key]: latest.current[key] })
      }
      latest.current = next
      setState(next)
    }
    window.addEventListener('hashchange', read)
    window.addEventListener('popstate', read)
    window.history.replaceState(null, '', buildHash(latest.current))
    return () => { window.removeEventListener('hashchange', read); window.removeEventListener('popstate', read) }
  }, [])
  return { state, updateUrl }
}
