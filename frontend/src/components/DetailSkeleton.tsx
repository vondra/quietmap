// Loading / error placeholder for the popup card. Renders the same
// header structure as `NoiseDetailContent` (coordinate + dB badge slot)
// so the skeleton → full upgrade reads as a value change, not a layout
// swap. Pure presentational — no fetch, no map state.

interface DetailSkeletonProps {
  position: { lat: number; lng: number }
  error?: string | null
}

export default function DetailSkeleton({ position, error }: DetailSkeletonProps) {
  return (
    <div className="px-2.5 pt-1 pb-2" data-testid="detail-popup-skeleton">
      <div className="flex items-center justify-between mb-1">
        <span className={`text-2xl font-bold leading-none shrink-0 ${error ? 'text-destructive/60' : 'text-muted-foreground/40 animate-pulse'}`}>
          {error ? '⚠' : '— dB'}
        </span>
        <div className="text-right pr-6">
          <div className="text-xs text-muted-foreground/60 font-mono leading-tight">
            {position.lat.toFixed(4)}, {position.lng.toFixed(4)}
          </div>
          <div className={`text-xs font-mono leading-tight max-w-[160px] truncate ${error ? 'text-destructive/80' : 'text-muted-foreground/40 animate-pulse'}`} title={error ?? undefined}>
            {error ?? 'computing…'}
          </div>
        </div>
      </div>
      {!error && (
        <div className="border-b border-border pb-0.5 mb-0.5">
          <span className="text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground/40 animate-pulse">
            Loading noise sources…
          </span>
        </div>
      )}
    </div>
  )
}
