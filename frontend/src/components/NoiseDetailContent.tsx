import { useEffect, useRef, useState } from 'react'
import { ldenToColor } from '../utils/noise-colors'
import { DataPoint } from './noise/noise-tooltips'
import { HoverText } from './ui/info-tip'
import { txtTable } from '../utils/formatters'
import { SOURCE_LABELS } from './noise/shared'
import { unavailableLayersSentence } from '../lib/unavailable-layers'
import { SegmentList } from './noise/SegmentList'
import { TabStrip, type PopupTab } from './noise/TabStrip'
import { ContributorRow } from './noise/source/ContributorRow'
import type { NoiseComputeData } from '../types/noise'

// The ⏱ timing panel ships server-side compute breakdowns into the popup —
// useful for profiling, noise for end users. Show it only when the URL carries
// ?timings (a dev opt-in), not to everyone.
const SHOW_TIMINGS = typeof location !== 'undefined' && new URLSearchParams(location.search).has('timings')

export interface NoiseDetailContentProps {
  data: NoiseComputeData
  onHighlight?: (geometry: any | null) => void
  maxSources?: number
}

// The rich popup body (sources + segments tabs, diagrams, per-effect tooltips).
// It pulls in the whole components/noise/ tree (~3.8 kLoC), so it is a lazy
// chunk — DetailCard / MobileDetailSheet import it via React.lazy and show
// DetailSkeleton until both the ~1.5 s noise compute AND this chunk land.
export default function NoiseDetailContent({ data, onHighlight, maxSources }: NoiseDetailContentProps) {
  const [centerLat, centerLng] = data.center
  // The popup's 0 dB display floor, applied to this list the way the per-layer
  // rows already apply it.
  const audibleContributors = data.top_contributors.filter(c => c.received_lden > 0)
  // Hide silence-sentinel values (sources with no audible contribution at this point).
  // The Rust engine returns periods even for empty source classes; their Lden falls
  // to ~−113 dB (silence) which is meaningless to display in the breakdown.
  const totalLdenText = data.total_lden != null
    ? txtTable([
        ...data.sources
          .filter(s => s.lden != null && s.lden > 0)
          .map(s => [SOURCE_LABELS[s.source_type] ?? s.source_type, `${s.lden!.toFixed(1)} dB`] as [string, string]),
        { sep: true },
        ['Total Lden', `${data.total_lden.toFixed(1)} dB`],
      ], 14, 9)
    : ''

  const [tab, setTab] = useState<PopupTab>('sources')
  // Mount the (up to 8000-row) Segments list only after its tab is first
  // visited — opening the popup on Sources must not pay to build it. Once
  // mounted it stays (display toggle) so per-row expanded state survives.
  const [segmentsMounted, setSegmentsMounted] = useState(false)
  const [fullSegments, setFullSegments] = useState<{
    segments: NoiseComputeData['segments']
    meta: NoiseComputeData['segments_meta']
  } | null>(null)
  const [loadingFull, setLoadingFull] = useState(false)
  const [segmentsError, setSegmentsError] = useState<string | null>(null)
  const showAllController = useRef<AbortController | null>(null)
  // Identity of the point the visible segments belong to. Compared by ref,
  // not by closure values (a closure comparison can never see a new point).
  const livePoint = useRef(`${centerLat},${centerLng}`)
  // Reset augmented data whenever the user clicks a new point, aborting any
  // in-flight "show all" so it can neither overwrite nor leak into the new one.
  useEffect(() => {
    showAllController.current?.abort()
    showAllController.current = null
    livePoint.current = `${centerLat},${centerLng}`
    setFullSegments(null)
    setLoadingFull(false)
    setSegmentsError(null)
  }, [centerLat, centerLng])
  // Unmount aborts the in-flight "show all": the request must not outlive
  // the dialog (gg finding 6).
  useEffect(() => () => showAllController.current?.abort(), [])

  const displaySegments = fullSegments?.segments ?? data.segments ?? []
  const displayMeta = fullSegments?.meta ?? data.segments_meta ?? null
  const segmentsTotal = displayMeta?.total_count ?? displaySegments.length
  const hasSegmentsTab = segmentsTotal > 0
  const showSegments = tab === 'segments' && hasSegmentsTab

  // The click fetches the summary (no segment list). Opening the Segments
  // tab pulls `detail=segments` (served from the server's result cache);
  // "Show all" pulls `detail=all` (fresh unfiltered compute, uncached).
  // Both abort when the point changes so a late `all` can never overwrite
  // a newer point's segments.
  useEffect(() => {
    if (!showSegments || fullSegments || data.segments) return
    const controller = new AbortController()
    setLoadingFull(true)
    setSegmentsError(null)
    void (async () => {
      try {
        const r = await fetch(
          `/api/noise-onfly-v2?lat=${centerLat}&lng=${centerLng}&detail=segments`,
          { signal: controller.signal },
        )
        if (!r.ok) throw new Error(`fetch failed: ${r.status}`)
        const next = (await r.json()) as NoiseComputeData
        setFullSegments({
          segments: next.segments ?? [],
          meta: next.segments_meta ?? null,
        })
      } catch (err) {
        // Aborted by point change: silent. A real failure surfaces with a
        // retry (independent of truncation — Show all may not exist).
        if (!controller.signal.aborted) {
          setSegmentsError(err instanceof Error ? err.message : 'fetch failed')
        }
      } finally {
        if (!controller.signal.aborted) setLoadingFull(false)
      }
    })()
    return () => controller.abort()
  }, [showSegments, fullSegments, data.segments, centerLat, centerLng])

  const handleShowAll = async () => {
    if (loadingFull) return
    setLoadingFull(true)
    const controller = new AbortController()
    showAllController.current = controller
    const point = `${centerLat},${centerLng}`
    try {
      const r = await fetch(`/api/noise-onfly-v2?lat=${centerLat}&lng=${centerLng}&detail=all`, {
        signal: controller.signal,
      })
      if (!r.ok) throw new Error(`fetch failed: ${r.status}`)
      const next = (await r.json()) as NoiseComputeData
      // A late arrival for an older point must not overwrite the current one.
      if (point !== livePoint.current) return
      setFullSegments({
        segments: next.segments ?? [],
        meta: next.segments_meta ?? null,
      })
    } catch {
      // Aborted or failed — loading state resets below for retry.
    } finally {
      setLoadingFull(false)
    }
  }

  return (
    <div data-testid="detail-popup" role="dialog" className="px-2.5 pt-1 pb-2" onClick={(e) => e.stopPropagation()}>
      {data.total_lden != null ? (
        <>
          <div className="flex items-center justify-between mb-1">
            <span
              data-testid="noise-badge"
              className="text-2xl font-bold leading-none shrink-0 whitespace-nowrap"
              style={{ color: ldenToColor(data.total_lden) }}
            >
              <DataPoint title="Total Lden — energy sum across all sources" text={totalLdenText}>
                {data.total_lden.toFixed(1)} dB
              </DataPoint>
            </span>
            <div className="text-right pr-6">
              <div className="text-xs text-muted-foreground/60 font-mono leading-tight">
                {centerLat.toFixed(4)}, {centerLng.toFixed(4)}
              </div>
              {data.elevation_m > 0 && (
                <div className="text-xs text-muted-foreground/60 font-mono leading-tight">{Math.round(data.elevation_m)} m a.s.l.</div>
              )}
            </div>
          </div>
          <BuildingExposureNotice exposure={data.building_exposure} />
          <UnavailableLayersNotice layers={data.unavailable_layers} />
          {hasSegmentsTab ? (
            <TabStrip
              active={tab}
              sourceCount={audibleContributors.length}
              segmentCount={segmentsTotal}
              onChange={(t) => { setTab(t); if (t === 'segments') setSegmentsMounted(true) }}
            />
          ) : (
            <div className="border-b border-border pb-0.5 mb-0.5">
              <span className="text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
                Noise sources ({audibleContributors.length})
              </span>
            </div>
          )}
          <div className="overflow-y-auto overflow-x-clip" style={{ maxHeight: 'max(100dvh - 400px, 160px)' }}>
            {/* Sources is always mounted; Segments mounts lazily (below) on first
                visit. Once mounted, both toggle via display so expanded-row state
                survives tab switches. */}
            <div style={{ display: showSegments ? 'none' : 'block' }}>
              {(maxSources ? audibleContributors.slice(0, maxSources) : audibleContributors).map((c, i) => (
                <ContributorRow key={`${c.source_type}-${c.osm_id}-${i}`} c={c} onToggle={onHighlight} />
              ))}
              {/* Same display floor as the per-layer rows. */}
              {data.other_sources_lden !== null && data.other_sources_lden > 0 && (
                <div className="flex items-baseline gap-1.5 px-0 py-1.5 border-t border-border/40 text-xs italic text-muted-foreground/70">
                  <span className="truncate flex-1">Other sources</span>
                  {/* Distance-column placeholder — keeps dB aligned with contributor rows above. */}
                  <span className="shrink-0 w-14" aria-hidden="true" />
                  <span className="shrink-0 w-14 text-right tabular-nums">
                    {data.other_sources_lden.toFixed(1)} dB
                  </span>
                  {/* Chevron-sized spacer so the right edge matches contributor rows. */}
                  <span aria-hidden="true" className="text-[10px] shrink-0 invisible">▼</span>
                </div>
              )}
            </div>
            {hasSegmentsTab && segmentsMounted && (
              <div style={{ display: showSegments ? 'block' : 'none' }}>
                {segmentsError && displaySegments.length === 0 ? (
                  <div role="alert" className="text-xs text-destructive py-2">
                    Segments failed to load ({segmentsError}).{' '}
                    <button
                      type="button"
                      className="underline"
                      onClick={() => {
                        setSegmentsError(null)
                        setFullSegments(null)
                      }}
                    >
                      Retry
                    </button>
                  </div>
                ) : (
                  <SegmentList
                    segments={displaySegments}
                    meta={displayMeta}
                    onHighlight={onHighlight}
                    onShowAll={handleShowAll}
                    loadingFull={loadingFull}
                  />
                )}
              </div>
            )}
          </div>
          <TimingsOverlay timings={data.timings ?? null} />
        </>
      ) : (
        <>
          <div className="text-sm text-muted-foreground mt-1">
            {data.building_exposure
              ? 'Not assessed — this building has no exposed façade'
              : 'No noise data computed for this location.'}
          </div>
          <UnavailableLayersNotice layers={data.unavailable_layers} />
        </>
      )}
    </div>
  )
}

const COMPASS_POINTS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'] as const

function compassPoint(bearingDeg: number): string {
  return COMPASS_POINTS[Math.round((((bearingDeg % 360) + 360) % 360) / 45) % 8]
}

// One line above the source rows: every level in this popup is the building's
// noisiest façade receiver, not the clicked point. Hover carries the receiver rule.
function BuildingExposureNotice({ exposure }: { exposure: NoiseComputeData['building_exposure'] }) {
  if (!exposure?.receiver) return null
  const [receiverLat, receiverLng] = exposure.receiver
  const faces = exposure.facade_bearing_deg == null ? '' : ` — faces ${compassPoint(exposure.facade_bearing_deg)}`
  const points = `1 of ${exposure.facade_points} façade point${exposure.facade_points === 1 ? '' : 's'}`
  const detail = `The level is computed at ${receiverLat.toFixed(5)}, ${receiverLng.toFixed(5)}: 0.1 m in front of this façade, 4 m above ground, as EU noise mapping does for building exposure; the façade's own reflection is excluded. Of the building's ${exposure.facade_points} façade points, this one is the loudest by Lden of all sources together.`
  return (
    <div data-testid="building-exposure" className="mb-1 border-b border-border/50">
      <HoverText title={detail} className="block" focusable>
        <span className="flex items-baseline gap-1.5 px-0 py-1 text-xs font-medium">
          <span className="truncate flex-1">Noisiest façade of this building{faces}</span>
          <span className="shrink-0 text-right tabular-nums font-normal text-muted-foreground/70">{points}</span>
        </span>
      </HoverText>
    </div>
  )
}

// The server answered without these emission layers, so the level is lower than the real one:
// the visitor must see that, not only a missing source row.
function UnavailableLayersNotice({ layers }: { layers: NoiseComputeData['unavailable_layers'] }) {
  const sentence = unavailableLayersSentence(layers)
  if (!sentence) return null
  return (
    <div data-testid="unavailable-layers" role="status" className="mb-1 border-b border-border/50 py-1 text-xs font-medium text-amber-800">
      {sentence}
    </div>
  )
}

function TimingsOverlay({ timings }: { timings: NoiseComputeData['timings'] }) {
  if (!timings || !SHOW_TIMINGS) return null
  // Sort components by cost so the dominant bucket is visible at a glance.
  const rows: Array<[string, number]> = [
    ['road', timings.road_ms],
    ['rail', timings.rail_ms],
    ['building', timings.building_ms],
    ['industrial', timings.industrial_ms],
    ['ships', timings.ship_ms],
    ['ac airborne', timings.aircraft_airborne_ms],
    ['ac cruise', timings.aircraft_cruise_ms],
    ['ac ground', timings.aircraft_ground_ms],
    ['load', timings.load_ms],
    ['collect', timings.collect_ms],
  ]
  // Road and rail kernels run concurrently, so their wall times overlap.
  const total = rows.reduce((s, [, ms]) => s + ms, 0) - Math.min(timings.road_ms, timings.rail_ms)
  const sorted = rows.sort((a, b) => b[1] - a[1])
  return (
    <div className="mt-2 pt-1.5 border-t border-border/30 text-[10px] font-mono text-muted-foreground/70 leading-tight">
      <div className="opacity-60 mb-0.5">⏱ popup-timing — Σ {total.toFixed(0)} ms (server, pre-JSON)</div>
      <div className="grid grid-cols-2 gap-x-3 gap-y-0">
        {sorted.map(([k, ms]) => (
          <div key={k} className="flex justify-between">
            <span className="opacity-80">{k}</span>
            <span className="tabular-nums">{ms.toFixed(0)} ms</span>
          </div>
        ))}
      </div>
    </div>
  )
}
