/** Concurrent provisional and exact point queries, guarded against stale responses. */
import type { NoiseComputeData } from '../types/noise'

export interface SurfacePreview {
  status: 'provisional'
  receiver: 'outdoor'
  accuracy: 'unmeasured'
  center: [number, number]
  layers: {
    layer: 'road' | 'rail' | 'industry' | 'building' | 'ground_ops'
    period_power: [number, number, number]
    lden_db: number | null
  }[]
}

type Position = { lat: number; lng: number }
type Callbacks = {
  isCurrent: () => boolean
  onData: (data: NoiseComputeData | null) => void
  onPreview: (preview: SurfacePreview | null) => void
  onError: (message: string) => void
}

export async function fetchNoiseDetail(position: Position, signal: AbortSignal, callbacks: Callbacks): Promise<void> {
  const params = new URLSearchParams({ lat: String(position.lat), lng: String(position.lng) })
  let exactHasData = false
  const exact = fetch(`/api/noise-onfly-v2?${params}`, { signal })
    .then(response => {
      if (!response.ok) throw new Error(`API ${response.status}`)
      return response.json() as Promise<NoiseComputeData | null>
    })
    .then(data => {
      if (signal.aborted || !callbacks.isCurrent()) return
      exactHasData = data !== null
      callbacks.onData(data)
    })
    .catch(error => {
      if (!signal.aborted && callbacks.isCurrent()) callbacks.onError(error instanceof Error ? error.message : String(error))
    })
  const preview = fetch(`/api/noise-preview?${params}`, { signal })
    .then(response => response.ok ? response.json() as Promise<SurfacePreview | null> : null)
    .then(data => {
      if (signal.aborted || !callbacks.isCurrent() || exactHasData || !data) return
      if (data.status !== 'provisional' || data.receiver !== 'outdoor'
          || data.center?.[0] !== position.lat || data.center?.[1] !== position.lng) return
      callbacks.onPreview(data)
    })
    .catch(() => { /* The exact request reports availability independently. */ })
  await Promise.all([exact, preview])
}
