import { lazy, Suspense, useState, useEffect, useRef, useCallback } from 'react'
import DetailSkeleton from './DetailSkeleton'
import type { NoiseComputeData } from '../types/noise'
import type { SurfacePreview } from '../lib/fetch-noise-detail'
import { resolveSheetTouchEnd } from '../lib/sheet-drag'

// Lazy popup body — see DetailCard; off first paint, pre-warmed by App on click.
const NoiseDetailContent = lazy(() => import('./NoiseDetailContent'))

interface MobileDetailSheetProps {
  data: NoiseComputeData | null
  // Same MVP-0 contract as DetailCard — sheet springs open on click
  // (position) with skeleton; upgrades to full content when `data`
  // arrives. Dependency MUST include `position` so a previously
  // dismissed sheet resets `dragOffset`/`dismissing` on a fresh click
  // (gg/Codex 2026-05-24 WARNING).
  position?: { lat: number; lng: number } | null
  error?: string | null
  preview?: SurfacePreview | null
  onClose: () => void
  onHighlight?: (geometry: any | null) => void
}

export default function MobileDetailSheet({ data, position, error, preview, onClose, onHighlight }: MobileDetailSheetProps) {
  const [expanded, setExpanded] = useState(false)
  const [dismissing, setDismissing] = useState(false)
  const [dragOffset, setDragOffset] = useState(0)
  const dragRef = useRef({ startY: 0, isDragging: false })
  // Pending swipe-dismiss `setTimeout` handle — cancelled on a fresh
  // click so a 300 ms swipe animation doesn't clear the user's new
  // selection (Codex /gg #82 WARNING).
  const dismissTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    if (data || position) {
      setExpanded(true)
      setDismissing(false)
      setDragOffset(0)
      if (dismissTimeoutRef.current !== null) {
        clearTimeout(dismissTimeoutRef.current)
        dismissTimeoutRef.current = null
      }
    }
  }, [data, position])

  const onTouchStart = useCallback((e: React.TouchEvent) => {
    dragRef.current = { startY: e.touches[0].clientY, isDragging: true }
  }, [])

  const onTouchMove = useCallback((e: React.TouchEvent) => {
    if (!dragRef.current.isDragging) return
    const deltaY = e.touches[0].clientY - dragRef.current.startY
    if (deltaY > 0) setDragOffset(deltaY)
  }, [])

  const onTouchEnd = useCallback((e: React.TouchEvent) => {
    if (!dragRef.current.isDragging) return
    const { cancelClick, dismiss } = resolveSheetTouchEnd(e.changedTouches[0].clientY - dragRef.current.startY)
    dragRef.current.isDragging = false
    if (cancelClick) {
      e.preventDefault()
      e.stopPropagation()
    }
    if (dismiss) {
      setDismissing(true)
      setDragOffset(0)
      dismissTimeoutRef.current = setTimeout(() => {
        dismissTimeoutRef.current = null
        onClose()
      }, 300)
    } else {
      setDragOffset(0)
    }
  }, [onClose])

  // Require active position for ANY render (Codex /gg #82 CRITICAL —
  // a late fetch resolving after Close would otherwise reopen the
  // sheet with stale data).
  if (!position) return null
  const showSkeleton = !data

  const transform = dismissing
    ? 'translateY(100%)'
    : dragOffset > 0
      ? `translateY(${dragOffset}px)`
      : undefined
  const transition = dragRef.current.isDragging ? 'none' : 'transform 0.3s ease-out'

  return (
    <div className="fixed bottom-0 left-0 right-0 z-[2000] md:hidden">
      <div
        data-testid="mobile-detail-sheet"
        className="bg-background rounded-t-xl shadow-2xl"
        style={{ transform, transition }}
      >
        <div
          className="flex justify-center py-3 cursor-grab"
          onTouchStart={onTouchStart}
          onTouchMove={onTouchMove}
          onTouchEnd={onTouchEnd}
          onClick={() => setExpanded(!expanded)}
          style={{ touchAction: 'none' }}
          role="button"
          aria-label={expanded ? 'Collapse' : 'Expand'}
        >
          <div className="w-10 h-1 bg-muted-foreground/30 rounded-full" />
        </div>

        {/* Collapsed = a peek at the top of the detail (place + level) under
            a fixed cap; the tap now reaches onClick, so this state is real
            (review 2026-09-10: `auto` let a tall detail grow on Collapse). */}
        <div className={`pb-1 overflow-x-clip ${expanded ? 'overflow-y-auto max-h-[calc(50vh-16px)]' : 'overflow-hidden max-h-24'}`}>
          {showSkeleton
            ? <DetailSkeleton position={position} error={error} preview={preview} />
            : <Suspense fallback={<DetailSkeleton position={position} error={error} preview={preview} />}>
                <NoiseDetailContent data={data!} onHighlight={onHighlight} maxSources={9} />
              </Suspense>}
        </div>
      </div>
    </div>
  )
}
