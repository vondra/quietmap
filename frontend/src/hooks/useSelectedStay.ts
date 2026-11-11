// Restore a selected stay by its id at the selected point, fetching fresh offers without storing them in links.
import { useEffect, useState } from 'react'
import { firstCheckin, stayRequests, type Stay, type StaySearch } from '../lib/stays'
import type { Position } from '../lib/url-state'
export function useSelectedStay(id: string | null, at: Position | null, search: StaySearch, seed: { stay: Stay; search: StaySearch } | null) {
  const expiredDates = search.checkin < firstCheckin()
  const key = JSON.stringify([id, at, search])
  const [result, setResult] = useState<{ key: string; stay: Stay | null; message: string | null } | null>(null)
  useEffect(() => {
    if (!id || !at || expiredDates) return
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    const load = async () => {
      setResult({ key, stay: null, message: 'Finding this place to stay…' })
      let retrySeconds = 0
      try {
        // A small box around the selected point also works when it is outside the recipient's view.
        const { lat, lng } = at
        const urls = stayRequests({ south: lat - 0.001, north: lat + 0.001, west: lng - 0.001, east: lng + 0.001 }, search)
        const answers = await Promise.all(urls.map(async url => {
          const response = await fetch(url, { signal: controller.signal })
          const seconds = Number(response.headers.get('Retry-After'))
          if (Number.isFinite(seconds) && seconds > 0 && seconds <= 60) retrySeconds = Math.max(retrySeconds, seconds)
          const body = await response.json()
          if (!response.ok || !Array.isArray(body?.listings)) throw new Error('This place could not be loaded. Reopen the link to try again.')
          const listing = body.listings.find((stay: Stay) => stay.id === id) as Stay | undefined
          return listing ? { ...listing, nights: body.nights, currency: body.currency, expires: Date.now() + body.expiresIn * 1000 } as Stay : null
        }))
        if (controller.signal.aborted) return
        const stay = answers.find(stay => stay != null) ?? null
        setResult({ key, stay, message: stay ? null : retrySeconds ? `Accommodation search is busy; retrying in ${retrySeconds} s.` : 'No current offer was found for this stay and these dates.' })
        if (stay || retrySeconds) timer = setTimeout(() => void load(), stay ? Math.max(1000, stay.expires - Date.now()) : retrySeconds * 1000)
      } catch {
        if (controller.signal.aborted) return
        setResult({ key, stay: null, message: retrySeconds
          ? `Accommodation search is busy; retrying in ${retrySeconds} s.`
          : 'This place could not be loaded. Reopen the link to try again.' })
        if (retrySeconds) timer = setTimeout(() => void load(), retrySeconds * 1000)
      }
    }
    if (seed?.stay.id === id && seed.stay.expires > Date.now() && JSON.stringify(seed.search) === JSON.stringify(search)) {
      setResult({ key, stay: seed.stay, message: null })
      timer = setTimeout(() => void load(), seed.stay.expires - Date.now())
    } else void load()
    return () => { controller.abort(); clearTimeout(timer) }
  }, [id, at, search, seed, key, expiredDates])
  if (!id || !at) return { stay: null, message: null }
  if (expiredDates) return { stay: null, message: 'These stay dates have passed. Choose new dates.' }
  return result?.key === key ? result : { stay: null, message: 'Finding this place to stay…' }
}
