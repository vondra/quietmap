// The popup body: how loud the place is over the whole day (Nden, the mean loudness in sone, Lden
// under it), and what is heard there and from what: the loudest contributors, the aircraft layer
// and everything else, each with why it is loud and how loud it is alone. Under the list the whole
// calculation opens in place.
// Redrawn on every streamed update of the click.
import { lazy, Suspense, useEffect, useRef, type ReactNode } from 'react'
import { ldenToColor } from '../utils/noise-colors'
import { DataPoint } from './noise/noise-tooltips'
import { fmtDb, fmtSone, txtTable } from '../utils/formatters'
import { SOURCE_LABELS } from './noise/labels'
import { PERIOD_LABELS_DETAIL } from './noise/shared'
import { AircraftLayerRow, ContributorRow, RestRow, rowRank } from './noise/source/ContributorRow'
import type { Contributor, LayerLevels, PeriodLevels, PopupUpdate, SegmentFan } from '../types/noise'

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
  'Nden: how loud the place sounds on average over',
  'the whole day, every moment by its loudness to',
  'the ear (Zwicker, ISO 532-1), so a sound counts',
  'by how loud it is and how long it lasts; the',
  'evening counted 5 dB and the night 10 dB louder,',
  'as Lden counts them. Twice the number sounds',
  'twice as loud.',
].join('\n')

export default function NoiseDetailContent({ data, maxSources, onHighlight, calculationOpen = false, onCalculationToggle, onFan }: NoiseDetailContentProps) {
  const [centerLat, centerLng] = data.center
  const answered = data.total_lden != null && !data.partial
  // The popup sends the ground contributors above its 0 dB display floor; the list ranks what is
  // heard by how loud each source is alone over the day (its Nden). The aircraft layer is one
  // row, at its rank, from 0 dB up. What the list does not show is one last row.
  const aircraft = data.sources.find(s => s.source_type === 'aircraft')
  const aircraftEntry = aircraft && aircraft.lden != null
    ? [{ rank: rowRank(aircraft.nden_sone, aircraft.lden), layer: aircraft }]
    : []
  const entries: ListEntry[] = [
    ...data.top_contributors.map(c => ({ rank: rowRank(c.nden_sone, c.received_lden), contributor: c })),
    ...aircraftEntry.filter(e => (e.layer.lden ?? 0) > 0),
  ].sort((a, b) => b.rank - a.rank)
  const shownEntries = maxSources ? entries.slice(0, maxSources) : entries
  const hidden = [
    ...entries.slice(shownEntries.length),
    ...aircraftEntry.filter(e => (e.layer.lden ?? 0) <= 0),
  ]
  const rest = restOf(data.sources, hidden)
  // The popup's Nden of what it left out knows nothing of rows hidden here (a phone shows fewer):
  // with them the last row has no Nden.
  const restNden = hidden.length > 0 && data.rest_nden_sone != null ? null : data.rest_nden_sone
  const shown = shownEntries.map(e => e.contributor
    ? (
      <ContributorRow
        key={`${e.contributor.source_type}-${e.contributor.id}`}
        c={e.contributor}
        onHighlight={id => onHighlight(id === null ? null : `source:${id}`)}
      />
    )
    : <AircraftLayerRow key="aircraft" layer={e.layer!} flights={data.top_flights} onHighlightFlight={onHighlight} />)
  if (rest) shown.push(<RestRow key="rest" levels={rest} nden={restNden} />)
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
  const sone = data.loudness?.nden_sone ?? null

  return (
    <div data-testid="detail-popup" role="dialog" className="px-2.5 pt-1 pb-2" onClick={(e) => e.stopPropagation()}>
      <div className="flex items-start justify-between mb-1.5">
        {data.total_lden != null ? (
          <div className="shrink-0">
            <span data-testid="noise-badge" className="flex items-baseline gap-1.5 leading-none whitespace-nowrap text-foreground">
              <span className="inline-block size-2.5 rounded-full self-center" style={{ background: ldenToColor(data.total_lden) }} aria-hidden="true" />
              {sone != null
                ? (
                  <DataPoint title="Nden" text={LOUDNESS_TEXT}>
                    <span className="text-xs font-medium text-muted-foreground">Nden </span>
                    <span className="text-2xl font-bold">{fmtSone(sone)}</span>
                    <span className="text-sm font-medium"> sone</span>
                  </DataPoint>
                )
                : <span className={`text-2xl font-bold text-muted-foreground/40${data.partial ? ' animate-pulse' : ''}`}>{data.partial ? '… sone' : '—'}</span>}
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
                className="mt-2 flex w-full items-center justify-between rounded-md border border-border px-2.5 py-1.5 text-xs font-medium text-foreground hover:bg-muted/40"
                onClick={onCalculationToggle}
              >
                <span>Detailed calculation</span>
                <span className="text-muted-foreground">{calculationOpen ? '▾' : '▸'}</span>
              </button>
              {calculationOpen && (
                <Suspense fallback={<div className="mt-2 text-[11px] text-muted-foreground animate-pulse">…</div>}>
                  <IntoView>
                    <CalculationDetails data={data} onFan={onFan} />
                  </IntoView>
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

/** Brings its content into view once it is shown: the calculation opens below the list, on a phone
 *  far below the fold. Inside the Suspense, so it scrolls when the lazy chunk has arrived. */
function IntoView({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => ref.current?.scrollIntoView({ block: 'start', behavior: 'smooth' }), [])
  return <div ref={ref} className="scroll-mt-2">{children}</div>
}

/** A row of the list: a contributor, or the aircraft layer as a whole; ranked by its Nden alone. */
interface ListEntry {
  rank: number
  contributor?: Contributor
  layer?: LayerLevels
}

const PERIOD_KEYS = ['ld', 'le', 'ln', 'lden'] as const

/** Everything the list does not show: the rows past its length (the aircraft layer among them when
 *  under 0 dB) and what the popup leaves out of each ground layer (its contributors cut from the
 *  thirty or under 0 dB): their levels. Null when it has no energy. */
function restOf(layers: LayerLevels[], hidden: ListEntry[]): PeriodLevels | null {
  const energy = (level: number | null | undefined) => (level == null ? 0 : 10 ** (level / 10))
  const ground = layers.filter(l => l.source_type !== 'aircraft')
  const levels = Object.fromEntries(PERIOD_KEYS.map(key => {
    const total = ground.reduce((sum, l) => sum + energy(l.unlisted?.[key]), 0)
      + hidden.reduce((sum, e) => sum + energy(e.contributor ? e.contributor.received[key] : e.layer?.[key]), 0)
    return [key, total > 0 ? 10 * Math.log10(total) : null]
  })) as unknown as PeriodLevels
  return levels.lden == null ? null : levels
}
