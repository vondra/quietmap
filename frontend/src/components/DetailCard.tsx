// The desktop popup card: its head the place's loudness, the close beside it, then the place to stay
// whose pin opened it, and a skeleton from the click on, then the streamed answer, or the error.
import { lazy, Suspense } from 'react'
import { useViewScroll } from '../hooks/useViewScroll'
import { useIsDesktop } from '../hooks/useIsDesktop'
import type { Metric } from '../lib/hm3-decoder'
import { X } from 'lucide-react'
import FloatingCard from './FloatingCard'
import DetailSkeleton from './DetailSkeleton'
import StayHeader from './StayHeader'
import SharePlace from './SharePlace'
import { ICON_BUTTON } from './ui/button'
import type { PopupUpdate, SegmentFan } from '../types/noise'
import type { Stay } from '../lib/stays'

// Lazy: the popup body is a separate chunk, off first paint. App pre-warms it on click (its
// detailPosition effect) so it downloads while the first answer is computed.
const NoiseDetailContent = lazy(() => import('./NoiseDetailContent'))

interface DetailCardProps {
  /** The quantity the popup leads with. */
  metric: Metric
  noiseData: PopupUpdate | null
  // The position arrives at click time, before the first streamed update: the card opens at
  // once with a skeleton and the coordinate.
  position?: { lat: number; lng: number } | null
  error?: string | null
  onNoiseClose: () => void
  getShareUrl: () => string
  onHighlight: (key: string | null) => void
  calculationOpen: boolean
  onCalculationToggle: () => void
  onFan: (source: string, fan: SegmentFan | null) => void
  /** The place to stay at the point, when its pin opened the popup. */
  stay: Stay | null
  stayMessage: string | null
}

export default function DetailCard({ metric, noiseData, position, error, onNoiseClose, getShareUrl, onHighlight, calculationOpen, onCalculationToggle, onFan, stay, stayMessage }: DetailCardProps) {
  const desktop = useIsDesktop()
  const scrollRef = useViewScroll('detail', desktop)

  // Require an active position for ANY render: a late update resolving after Close must never
  // reopen the card with stale content.
  if (!position) return null
  const showSkeleton = !noiseData

  // max-h-[50dvh] keeps half the viewport for the map; with the calculation open the card takes
  // the rest of its column. The column's flex shrink (min-h-0) squeezes the card further on
  // short viewports. overflow-x-clip: nothing inside may pan sideways — a child wider than
  // the card used to drag the whole popup left on vertical scroll.
  return (
    <FloatingCard key={`${position.lat},${position.lng}`} className={`relative p-0 pointer-events-auto min-h-0 overflow-y-auto overflow-x-clip ${calculationOpen ? '' : 'max-h-[50dvh]'}`} ref={scrollRef}>
      {/* The actions float at the card's head, the loudness beside them, and stay in sight while the
          answer scrolls under it. */}
      <div className="sticky top-0 z-10 float-right m-0.5 flex bg-card/90">
        <SharePlace key={`${position.lat},${position.lng}`} getUrl={getShareUrl} />
        <button onClick={onNoiseClose} className={`${ICON_BUTTON} pointer-coarse:size-11`} aria-label="Close">
          <X className="size-4 pointer-coarse:size-5" />
        </button>
      </div>
      {stay && <StayHeader stay={stay} />}
      {stayMessage && <p role="status" data-testid="stay-status" className="border-b border-border px-2.5 py-2 text-xs text-muted-foreground">{stayMessage}</p>}
      {desktop && (showSkeleton
        ? <DetailSkeleton position={position} error={error} />
        : <Suspense fallback={<DetailSkeleton position={position} error={error} />}>
            <NoiseDetailContent
              metric={metric}
              data={noiseData}
              onHighlight={onHighlight}
              calculationOpen={calculationOpen}
              onCalculationToggle={onCalculationToggle}
              onFan={onFan}
            />
          </Suspense>)}
    </FloatingCard>
  )
}
