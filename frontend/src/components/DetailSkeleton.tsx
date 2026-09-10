import type { SurfacePreview } from '../lib/fetch-noise-detail'

interface DetailSkeletonProps {
  position: { lat: number; lng: number }
  error?: string | null
  preview?: SurfacePreview | null
}

const LAYER_LABELS = { road: 'Roads', rail: 'Railways', industry: 'Industry', building: 'Buildings', ground_ops: 'Airport ground' }

export default function DetailSkeleton({ position, error, preview }: DetailSkeletonProps) {
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
      {preview && (
        <div className="mt-2 text-xs" data-testid="noise-surface-preview" aria-live="polite">
          <div className="font-medium">Approximate outdoor levels · Lden</div>
          <dl className="my-1 space-y-0.5">
            {preview.layers.map(layer => (
              <div key={layer.layer} className="flex justify-between gap-2">
                <dt>{LAYER_LABELS[layer.layer]}</dt>
                <dd className="font-mono tabular-nums">{layer.lden_db === null ? '—' : `≈ ${Math.round(layer.lden_db)} dB`}</dd>
              </div>
            ))}
          </dl>
          <p className="text-muted-foreground">Aircraft in flight are not yet included.
            {!error && ' Calculating full results for this location…'}</p>
        </div>
      )}
      {!error && !preview && (
        <div className="border-b border-border pb-0.5 mb-0.5">
          <span className="text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground/40 animate-pulse">
            Loading noise sources…
          </span>
        </div>
      )}
    </div>
  )
}
