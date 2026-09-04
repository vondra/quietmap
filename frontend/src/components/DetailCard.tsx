import { lazy, Suspense, useEffect, useRef } from 'react'
import { X } from 'lucide-react'
import FloatingCard from './FloatingCard'
import DetailSkeleton from './DetailSkeleton'
import type { NoiseComputeData } from '../types/noise'

// Lazy: the popup body (+ the noise/ tree, ~3.8 kLoC) is a separate chunk, off
// first paint. App pre-warms it on click (its detailPosition effect) so it
// overlaps the ~1.5 s compute and the open isn't delayed.
const NoiseDetailContent = lazy(() => import('./NoiseDetailContent'))

interface DetailCardProps {
  noiseData: NoiseComputeData | null
  // Position arrives at click time, well before `noiseData` (which waits
  // for the ~1.5 s NAPI compute). Opening the card immediately on click
  // with a skeleton + coordinate is the MVP-0 perceived-latency win
  // (gg/Gemini/Codex 2026-05-24).
  position?: { lat: number; lng: number } | null
  error?: string | null
  onNoiseClose: () => void
  onHighlight: (geometry: any | null) => void
}

export default function DetailCard({ noiseData, position, error, onNoiseClose, onHighlight }: DetailCardProps) {
  const scrollRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (noiseData && scrollRef.current) {
      scrollRef.current.scrollTop = 0
    }
  }, [noiseData])

  // Require an active position for ANY render. Guards against a late
  // fetch resolving after the user clicked Close — without this, the
  // resolved data would reopen the card with stale content (Codex /gg
  // 2026-05-24 CRITICAL). App clears both on close + AbortController
  // catches most races, but this is the defense-in-depth.
  if (!position) return null
  const showSkeleton = !noiseData

  return (
    <FloatingCard className="relative p-0 max-h-[50vh] overflow-y-auto" ref={scrollRef}>
      <button
        onClick={onNoiseClose}
        className="absolute top-1 right-1.5 z-10 p-1 rounded-md hover:bg-black/5 text-muted-foreground hover:text-foreground"
        aria-label="Close"
      >
        <X className="size-3.5" />
      </button>
      {showSkeleton
        ? <DetailSkeleton position={position} error={error} />
        : <Suspense fallback={<DetailSkeleton position={position} error={error} />}>
            <NoiseDetailContent data={noiseData!} onHighlight={onHighlight} />
          </Suspense>}
    </FloatingCard>
  )
}
