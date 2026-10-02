// The popup body: how loud the place is over the whole day (loudness in sone, Lden under it), and
// what is heard there and from what: the loudest contributors and the aircraft layer, each with how
// it is heard and its share of the noise. Under the list the whole calculation opens in place.
// Redrawn on every streamed update of the click.
import { lazy, Suspense } from 'react'
import { ldenToColor } from '../utils/noise-colors'
import { DataPoint } from './noise/noise-tooltips'
import { fmtDb, fmtSone, txtTable } from '../utils/formatters'
import { PERIOD_LABELS_DETAIL, SOURCE_LABELS } from './noise/shared'
import { AircraftLayerRow, ContributorRow } from './noise/source/ContributorRow'
import type { PopupUpdate, SegmentFan } from '../types/noise'

// Lazy: the calculation is a separate chunk, loaded when a visitor opens it.
const CalculationDetails = lazy(() => import('./calculation/CalculationDetails'))

// The read and compute statistics of the click are for profiling, not for visitors: shown only
// when the URL carries ?timings.
const SHOW_STATS = typeof location !== 'undefined' && new URLSearchParams(location.search).has('timings')

export interface NoiseDetailContentProps {
  data: PopupUpdate
  maxSources?: number
  /** Shows a loudest flight's track (by `topFlightKey`) or a contributor's pieces (by
   * `source:<id>`) on the map; null shows none. */
  onHighlight: (key: string | null) => void
  /** Whether the detailed calculation is open under the list, and how to open or close it. */
  calculationOpen?: boolean
  onCalculationToggle?: () => void
  /** Draws the calculation's pieces and rays on the map; null clears them. */
  onFan?: (fan: SegmentFan | null) => void
}

const LOUDNESS_TEXT = [
  'Loudness over the whole day: how loud the sound',
  'heard 5 % of the time is to the ear (Zwicker,',
  'ISO 532-1, N5), the evening counted 5 dB and',
  'the night 10 dB louder, as Lden counts them.',
  'Twice the number sounds twice as loud.',
].join('\n')

export default function NoiseDetailContent({ data, maxSources, onHighlight, calculationOpen = false, onCalculationToggle, onFan }: NoiseDetailContentProps) {
  const [centerLat, centerLng] = data.center
  const answered = data.total_lden != null && !data.partial
  const total = data.total_lden ?? 0
  // The popup's 0 dB display floor, applied to this list the way the per-layer rows apply it.
  const audibleContributors = data.top_contributors.filter(c => c.received_lden != null && c.received_lden > 0)
  const rows = audibleContributors.map(c => (
    <ContributorRow
      key={`${c.source_type}-${c.id}`}
      c={c}
      totalLden={total}
      onHighlight={id => onHighlight(id === null ? null : `source:${id}`)}
    />
  ))
  // The aircraft layer lists no contributors: the layer is one row, at its rank by Lden.
  const aircraft = data.sources.find(s => s.source_type === 'aircraft')
  const aircraftLden = aircraft?.lden ?? 0
  if (aircraft && aircraftLden > 0) {
    const rank = audibleContributors.findIndex(c => (c.received_lden ?? 0) < aircraftLden)
    rows.splice(rank < 0 ? rows.length : rank, 0,
      <AircraftLayerRow key="aircraft" layer={aircraft} totalLden={total} flights={data.top_flights} onHighlightFlight={onHighlight} />)
  }
  const totalLdenText = txtTable([
    ...data.sources
      .filter(s => s.lden != null && s.lden > 0)
      .map(s => [SOURCE_LABELS[s.source_type] ?? s.source_type, fmtDb(s.lden)] as [string, string]),
    { sep: true },
    [PERIOD_LABELS_DETAIL[0], fmtDb(data.total.ld)],
    [PERIOD_LABELS_DETAIL[1], fmtDb(data.total.le)],
    [PERIOD_LABELS_DETAIL[2], fmtDb(data.total.ln)],
    { sep: true },
    ['Total Lden', fmtDb(data.total_lden)],
  ], 16, 9)
  const shown = maxSources ? rows.slice(0, maxSources) : rows
  const sone = data.loudness?.n5_den_sone ?? null

  return (
    <div data-testid="detail-popup" role="dialog" className="px-2.5 pt-1 pb-2" onClick={(e) => e.stopPropagation()}>
      <div className="flex items-start justify-between mb-1.5">
        {data.total_lden != null ? (
          <div className="shrink-0">
            <span data-testid="noise-badge" className="flex items-baseline gap-1.5 leading-none whitespace-nowrap text-foreground">
              <span className="inline-block size-2.5 rounded-full self-center" style={{ background: ldenToColor(data.total_lden) }} aria-hidden="true" />
              {sone != null && sone > 0
                ? (
                  <DataPoint title="Loudness" text={LOUDNESS_TEXT}>
                    <span className="text-2xl font-bold">{fmtSone(sone)}</span>
                    <span className="text-sm font-medium"> sone</span>
                  </DataPoint>
                )
                : <span className="text-2xl font-bold text-muted-foreground/40 animate-pulse">… sone</span>}
            </span>
            <div data-testid="lden" className="mt-1 pl-4 text-xs text-muted-foreground/60 font-mono leading-tight">
              <DataPoint title="Total Lden — energy sum across all sources (EU noise mapping)" text={totalLdenText}>
                {data.total_lden.toFixed(1)} dB Lden
              </DataPoint>
            </div>
          </div>
        ) : <span />}
        <div className="text-right pr-6 text-xs text-muted-foreground/60 font-mono leading-tight">
          <div>{centerLat.toFixed(4)}, {centerLng.toFixed(4)}</div>
          {data.elevation_m > 0 && <div>{Math.round(data.elevation_m)} m a.s.l.</div>}
          {data.partial && (
            <div
              data-testid="popup-refining"
              className="animate-pulse"
              title="The levels shown are valid; farther sources are still being added."
            >
              refining…
            </div>
          )}
        </div>
      </div>
      {data.total_lden != null ? (
        <>
          <div className="border-t border-border">{shown}</div>
          {answered && onCalculationToggle && (
            <>
              <button
                type="button"
                data-testid="calculation-toggle"
                aria-expanded={calculationOpen}
                className="mt-1 w-full border-t border-border pt-1 text-left text-[11px] text-muted-foreground hover:text-foreground"
                onClick={onCalculationToggle}
              >
                Detailed calc {calculationOpen ? '▾' : '▸'}
              </button>
              {calculationOpen && (
                <Suspense fallback={<div className="mt-2 text-[11px] text-muted-foreground animate-pulse">…</div>}>
                  <CalculationDetails data={data} onFan={onFan} />
                </Suspense>
              )}
            </>
          )}
        </>
      ) : (
        <div className="text-sm text-muted-foreground mt-1">
          {data.building?.facade === null
            ? 'Not assessed — this building has no exposed façade'
            : 'No modelled noise source reaches this point.'}
        </div>
      )}
      {SHOW_STATS && <StatsPanel data={data} />}
    </div>
  )
}

function StatsPanel({ data }: { data: PopupUpdate }) {
  const { stats } = data
  return (
    <div data-testid="popup-stats" className="mt-2 pt-1.5 border-t border-border/30 text-[10px] font-mono text-muted-foreground/70 leading-tight">
      <div className="opacity-60 mb-0.5">
        ⏱ update {data.seq}{data.partial ? ' (partial)' : ''} · {stats.rings} ring{stats.rings === 1 ? '' : 's'} · {stats.files} files · {(stats.bytes / 1e6).toFixed(1)} MB
      </div>
      <div className="grid grid-cols-2 gap-x-3 gap-y-0">
        {([['read', stats.read_ms], ['candidates', stats.candidate_ms], ['evaluate', stats.evaluate_ms], ['elapsed', stats.elapsed_ms]] as const).map(([name, ms]) => (
          <div key={name} className="flex justify-between">
            <span className="opacity-80">{name}</span>
            <span className="tabular-nums">{ms.toFixed(0)} ms</span>
          </div>
        ))}
        {data.sources.filter(s => s.candidates > 0).map(s => (
          <div key={s.source_type} className="flex justify-between">
            <span className="opacity-80">{s.source_type}</span>
            <span className="tabular-nums">{s.evaluated}/{s.candidates}</span>
          </div>
        ))}
      </div>
    </div>
  )
}
