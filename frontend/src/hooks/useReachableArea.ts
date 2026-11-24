// Recompute a shared reachable area at its own origin; only making a new area fits the camera.
import { useCallback, useEffect, useRef, useState } from 'react'
import type { ReachRequest } from '../lib/url-state'
export function useReachableArea(request: ReachRequest | null) {
  const fitNext = useRef(false)
  const [attempt, setAttempt] = useState(0)
  const retry = useCallback(() => setAttempt(n => n + 1), [])
  const lat = request?.lat, lng = request?.lng, minutes = request?.minutes, mode = request?.mode
  const origin = request && `${lat?.toFixed(6)},${lng?.toFixed(6)}`
  const key = origin && `${origin},${minutes},${mode}`
  const [result, setResult] = useState<{ key: string; attempt: number; origin: string; area: GeoJSON.FeatureCollection | null; fit: boolean; error: string | null } | null>(null)
  useEffect(() => {
    setResult(prev => prev?.fit ? { ...prev, fit: false } : prev)
    if (!key || !origin || lat == null || lng == null || minutes == null || mode == null) return
    const controller = new AbortController()
    void (async () => {
      try {
        const response = await fetch(`/api/isochron?lat=${lat}&lng=${lng}&minutes=${minutes}&modes=${mode}`, { signal: controller.signal }).catch(() => { throw new Error('The map could not be reached. Try again in a minute.') })
        const answer = await response.json().catch(() => null)
        if (response.status === 429) throw new Error('Too many areas asked from here. Try again in a minute.')
        if (!response.ok || !Array.isArray(answer?.features) || !answer.features.some((feature: GeoJSON.Feature) => feature.properties?.mode === mode)) {
          throw new Error(answer?.error ?? 'The area could not be drawn. Try again in a minute.')
        }
        if (!controller.signal.aborted) {
          const fit = fitNext.current
          fitNext.current = false
          setResult({ key, attempt, origin, area: answer, fit, error: null })
        }
      } catch (failure) {
        if (!controller.signal.aborted) {
          fitNext.current = false
          const message = failure instanceof Error ? failure.message : 'The area could not be drawn.'
          setResult(prev => ({ key, attempt, origin, area: prev?.origin === origin ? prev.area : null, fit: false, error: message }))
        }
      }
    })()
    return () => controller.abort()
  }, [key, attempt, origin, lat, lng, minutes, mode])
  const current = key && result?.key === key && result.attempt === attempt ? result : null
  const sameOriginArea = origin && result?.origin === origin ? result.area : null
  return { area: request ? (current ? current.area : sameOriginArea) : null, fit: current?.fit ?? false, error: current?.error ?? null, busy: !!key && !current, fitNext, retry }
}
