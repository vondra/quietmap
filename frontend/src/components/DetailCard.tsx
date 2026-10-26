// The desktop popup card: the places' tabs and the close at its top, then the place to stay whose pin
// opened it, the place's recordings and photos, and a skeleton from the click on, then the streamed
// answer, or the error.
import { lazy, Suspense, useEffect, useRef, type ReactNode } from 'react'
import { X } from 'lucide-react'
import FloatingCard from './FloatingCard'
import DetailSkeleton from './DetailSkeleton'
import StayHeader from './StayHeader'
import type { PopupUpdate, SegmentFan } from '../types/noise'
import type { Stay } from '../lib/stays'

// Lazy: the popup body is a separate chunk, off first paint. App pre-warms it on click (its
// detailPosition effect) so it downloads while the first answer is computed.
const NoiseDetailContent = lazy(() => import('./NoiseDetailContent'))

interface DetailCardProps {
  noiseData: PopupUpdate | null
  // The position arrives at click time, before the first streamed update: the card opens at
  // once with a skeleton and the coordinate.
  position?: { lat: number; lng: number } | null
  error?: string | null
  onNoiseClose: () => void
  onHighlight: (key: string | null) => void
  calculationOpen: boolean
  onCalculationToggle: () => void
  onFan: (source: string, fan: SegmentFan | null) => void
  /** The place to stay at the point, when its pin opened the popup. */
  stay: Stay | null
  /** The places' tabs, the open one's name among them. */
  tabs?: ReactNode
  /** The open place's recordings and photos, over the answer. */
  placeMedia?: ReactNode
}

export default function DetailCard({ noiseData, position, error, onNoiseClose, onHighlight, calculationOpen, onCalculationToggle, onFan, stay, tabs, placeMedia }: DetailCardProps) {
  const scrollRef = useRef<HTMLDivElement>(null)

  // A new point starts at the top; the streamed updates of one point keep the reader's scroll.
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0
  }, [position])

  // Require an active position for ANY render: a late update resolving after Close must never
  // reopen the card with stale content.
  if (!position) return null
  const showSkeleton = !noiseData

  // max-h-[50dvh] keeps half the viewport for the map; with the calculation open the card takes
  // the rest of its column. The column's flex shrink (min-h-0) squeezes the card further on
  // short viewports. overflow-x-clip: nothing inside may pan sideways — a child wider than
  // the card used to drag the whole popup left on vertical scroll.
  return (
    <FloatingCard className={`relative p-0 pointer-events-auto min-h-0 overflow-y-auto overflow-x-clip ${calculationOpen ? '' : 'max-h-[50dvh]'}`} ref={scrollRef}>
      {/* The tabs and the close stay at the top while the answer scrolls under them. */}
      <div className="sticky top-0 z-10 flex items-start gap-1 rounded-t-md bg-white pt-1.5 pl-1.5">
        <div className="min-w-0 flex-1">{tabs}</div>
        <button
          onClick={onNoiseClose}
          className="shrink-0 p-1.5 rounded-md hover:bg-black/5 text-muted-foreground hover:text-foreground"
          aria-label="Close"
        >
          <X className="size-3.5" />
        </button>
      </div>
      {stay && <StayHeader stay={stay} />}
      {placeMedia}
      {showSkeleton
        ? <DetailSkeleton position={position} error={error} />
        : <Suspense fallback={<DetailSkeleton position={position} error={error} />}>
            <NoiseDetailContent
              data={noiseData}
              onHighlight={onHighlight}
              calculationOpen={calculationOpen}
              onCalculationToggle={onCalculationToggle}
              onFan={onFan}
            />
          </Suspense>}
    </FloatingCard>
  )
}
