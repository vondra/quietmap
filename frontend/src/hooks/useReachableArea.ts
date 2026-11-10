// Recompute a shared reachable area at its own origin; only a new Show area action fits the camera.
import { useEffect, useRef, useState } from 'react'
import type { ReachRequest } from '../lib/url-state'
export function useReachableArea(request: ReachRequest | null) {
  const fitNext = useRef(false)
  const [result, setResult] = useState<{ request: ReachRequest; area: GeoJSON.FeatureCollection | null; fit: boolean; error: string | null } | null>(null)
  useEffect(() => {
    if (!request) return
    const controller = new AbortController(), fit = fitNext.current
    fitNext.current = false
    const { lat, lng, minutes, modes } = request
    void (async () => {
      try {
        const response = await fetch(`/api/isochron?lat=${lat}&lng=${lng}&minutes=${minutes}&modes=${modes.join(',')}`, { signal: controller.signal }).catch(() => { throw new Error('The map could not be reached. Try again in a minute.') })
        const answer = await response.json().catch(() => null)
        if (response.status === 429) throw new Error('Too many areas asked from here. Try again in a minute.')
        if (!response.ok || !Array.isArray(answer?.features)) throw new Error(answer?.error ?? 'The area could not be drawn. Try again in a minute.')
        const missing = modes.some(mode => !answer.features.some((feature: GeoJSON.Feature) => feature.properties?.mode === mode))
        if (!controller.signal.aborted) setResult({ request, area: answer, fit,
          error: missing ? 'Part of the reachable area could not be drawn. Try again in a minute.' : null })
      } catch (failure) {
        if (!controller.signal.aborted) setResult({ request, area: null, fit: false,
          error: failure instanceof Error ? failure.message : 'The area could not be drawn.' })
      }
    })()
    return () => controller.abort()
  }, [request])
  const current = request && result?.request === request ? result : null
  return { area: current?.area ?? null, fit: current?.fit ?? false, error: current?.error ?? null, busy: !!request && !current, fitNext }
}
