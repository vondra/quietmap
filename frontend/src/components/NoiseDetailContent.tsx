// The click's level, source list and detailed calculation, redrawn on every streamed update.
import { lazy, Suspense, useEffect, useRef, type ReactNode } from 'react'
import { paletteHex } from '../lib/heatmap-palette'
import { fmtSone, wholePercents } from '../utils/formatters'
import { HoverText } from './ui/info-tip'
import { restSources, shownRows, type HiddenRow } from './noise/rest'
import { Chevron } from './noise/shared'
import { AircraftLayerRow, ContributorRow, RestRow, rowRank } from './noise/source/ContributorRow'
import type { Metric } from '../lib/hm3-decoder'
import type { PopupUpdate, SegmentFan } from '../types/noise'

// Lazy: the calculation is a separate chunk, loaded when a visitor opens it.
const CalculationDetails = lazy(() => import('./calculation/CalculationDetails'))

// The read and compute statistics of the click are for profiling, not for visitors: shown only
// when the URL carries ?timings.
const SHOW_STATS = typeof location !== 'undefined' && new URLSearchParams(location.search).has('timings')

export interface NoiseDetailContentProps {
  /** The quantity it leads with and ranks the sources by: loudness (sone) or Lden (dB). */
  metric: Metric
  data: PopupUpdate
  /** Shows a loudest flight's track (by `topFlightKey`) or a contributor's pieces (by
   * `source:<id>`) on the map; null shows none. */
  onHighlight: (key: string | null) => void
  /** Whether the detailed calculation is open under the list, and how to open or close it. */
  calculationOpen?: boolean
  onCalculationToggle?: () => void
  /** Draws an opened row's segments and rays on the map; null takes that row's away. */
  onFan?: (source: string, fan: SegmentFan | null) => void
}

const NDEN_TOOLTIP = 'How loud the place sounds over the whole day: every moment by its loudness\n'
  + 'to the ear (ISO 532-1), the evening 5 dB and the night 10 dB louder, as in Lden.\n'
  + 'Twice the sone sounds twice as loud.'
const NDEN_SHARE_TOOLTIP = NDEN_TOOLTIP + ' Each row: its share of that loudness;\n'
  + 'opened, how loud it sounds alone.'

const LDEN_TOOLTIP = 'All sources together over the day (EU Directive 2002/49): the evening\n'
  + 'counts 5 dB and the night 10 dB louder. Each period in the detailed calculation.'
const LDEN_SHARE_TOOLTIP = LDEN_TOOLTIP + '\nEach row: its share of the sound energy.'

export default function NoiseDetailContent({ metric, data, onHighlight, calculationOpen = false, onCalculationToggle, onFan }: NoiseDetailContentProps) {
  const [centerLat, centerLng] = data.center
  const answered = data.total_lden != null && !data.partial
  // The popup sends the ground contributors above its 0 dB display floor; the list ranks what is
  // heard by its share of the place's loudness, the number each row shows (before the final
  // update by its Lden). The aircraft layer is one row, at its rank, from 0 dB up. What the list
  // does not show is one last row.
  // In Lden the list ranks by each row's Lden and shows its share of the place's energy, once
  // answered. The energy is the sum of what the answer is made of (the rows, the aircraft layer, each
  // ground layer's unlisted sources), so the shares of the rounded levels add up to 100 %.
  const lden = metric === 'lden'
  const rank = (share: number | null | undefined, db: number | null | undefined) =>
    lden ? (db ?? -Infinity) : rowRank(share, db)
  const aircraft = data.sources.find(s => s.source_type === 'aircraft')
  const energy = (db: number | null | undefined) => (db == null ? 0 : 10 ** (db / 10))
  const unlistedEnergy = data.sources.reduce(
    (sum, s) => sum + (s.source_type === 'aircraft' ? 0 : energy(s.unlisted?.lden)), 0)
  const placeEnergy = unlistedEnergy + energy(aircraft?.lden)
    + data.top_contributors.reduce((sum, c) => sum + energy(c.received_lden), 0)
  const energyShare = (part: number) => (answered && placeEnergy > 0 ? part / placeEnergy : undefined)
  const aircraftEntry = aircraft && aircraft.lden != null
    ? [{ rank: rank(aircraft.share, aircraft.lden), layer: aircraft }]
    : []
  const entries: ListEntry[] = [
    ...data.top_contributors.map(c => ({ rank: rank(c.share, c.received_lden), contributor: c })),
    ...aircraftEntry.filter(e => (e.layer.lden ?? 0) > 0),
  ].sort((a, b) => b.rank - a.rank)
  const quietAircraft = aircraftEntry.filter(e => (e.layer.lden ?? 0) <= 0)
  const shareOf = (row: HiddenRow) => lden
    ? energyShare(energy(row.contributor ? row.contributor.received_lden : row.layer?.lden))
    : (row.contributor ? row.contributor.share : row.layer?.share)
  const restShare = lden
    ? quietAircraft.reduce((sum, e) => sum + (shareOf(e) ?? 0), energyShare(unlistedEnergy) ?? 0)
    : quietAircraft.reduce((sum, e) => sum + (shareOf(e) ?? 0), data.rest_share ?? 0)
  // Every row's share and the rest's in whole percents adding up to 100, once the final update
  // carries them; the list shows the rows `shownRows` picks, the others fold into the last row.
  const shares = [...entries.map(shareOf), restShare]
  const percents = shares.every(share => share != null) ? wholePercents(shares as number[]) : []
  const shownMask = shownRows(shares.slice(0, entries.length))
  const shownEntries = entries.filter((_, row) => shownMask[row])
  const folded = entries.filter((_, row) => !shownMask[row])
  const hidden = [...folded, ...quietAircraft]
  const rest = restSources(data.sources, hidden)
  const percentOf = (entry: ListEntry) => (percents.length ? percents[entries.indexOf(entry)] : undefined)
  const restPercent = percents.length
    ? folded.reduce((sum, e) => sum + (percentOf(e) ?? 0), percents[entries.length])
    : undefined
  // An opened row's segments are the same click computed again: only once it is answered.
  const click = answered && onFan
    ? { at: data.center, receiver: data.building?.facade?.receiver ?? data.center, onFan }
    : undefined
  const listRow = (e: ListEntry) => e.contributor
    ? (
      <ContributorRow
        key={`${e.contributor.source_type}-${e.contributor.id}`}
        c={e.contributor}
        percent={percentOf(e)}
        onHighlight={id => onHighlight(id === null ? null : `source:${id}`)}
        click={click}
      />
    )
    : <AircraftLayerRow key="aircraft" layer={e.layer!} percent={percentOf(e)} flights={data.top_flights} onHighlightFlight={onHighlight} />
  const shown = shownEntries.map(listRow)
  if (rest != null) {
    shown.push(
      <RestRow key="rest" sources={rest} percent={restPercent}>
        {folded.length > 0 ? folded.map(listRow) : undefined}
      </RestRow>,
    )
  }
  const sone = data.loudness?.nden_sone ?? null
  // The level in the chosen quantity leads, the other small beneath it; Lden from the first update.
  const headline = lden
    ? { value: data.total_lden, text: (db: number) => db.toFixed(1), unit: ' dB', name: ' Lden', tooltip: LDEN_SHARE_TOOLTIP }
    : { value: sone, text: fmtSone, unit: ' sone', name: ' Nden', tooltip: NDEN_SHARE_TOOLTIP }

  return (
    <div data-testid="detail-popup" role="dialog" className="px-2.5 pt-1 pb-2" onClick={(e) => e.stopPropagation()}>
      <div className="flex items-start justify-between mb-1.5">
        {data.total_lden != null ? (
          <div className="shrink-0">
            <span data-testid="noise-badge" className="flex items-baseline gap-1.5 leading-none whitespace-nowrap text-foreground">
              <span className="inline-block size-2.5 rounded-full self-center" style={{ background: headline.value != null ? paletteHex(metric, headline.value) : 'transparent' }} aria-hidden="true" />
              {headline.value != null
                ? (
                  <HoverText title={headline.tooltip}>
                    <span className="text-2xl font-bold">{headline.text(headline.value)}</span>
                    <span className="text-sm font-medium">{headline.unit}</span>
                    <span className="text-xs font-medium text-muted-foreground">{headline.name}</span>
                  </HoverText>
                )
                : <span className={`text-2xl font-bold text-muted-foreground/40${data.partial ? ' animate-pulse' : ''}`}>{data.partial ? `…${headline.unit}` : '—'}</span>}
            </span>
            {lden
              ? sone != null && (
                <div data-testid="nden" className="mt-1 pl-4 text-xs text-muted-foreground/60 tabular-nums leading-tight">
                  <HoverText title={NDEN_TOOLTIP}>{fmtSone(sone)} sone Nden</HoverText>
                </div>
              )
              : (
                <div data-testid="lden" className="mt-1 pl-4 text-xs text-muted-foreground/60 tabular-nums leading-tight">
                  <HoverText title={LDEN_TOOLTIP}>{data.total_lden.toFixed(1)} dB Lden</HoverText>
                </div>
              )}
          </div>
        ) : <span />}
        <div className="text-right pr-6 text-xs text-muted-foreground/60 tabular-nums leading-tight">
          <div>{centerLat.toFixed(4)}, {centerLng.toFixed(4)}</div>
          {data.elevation_m > 0 && <div>{Math.round(data.elevation_m)} m a.s.l.</div>}
          {data.partial && <div data-testid="popup-refining" className="animate-pulse">refining…</div>}
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
                <Chevron open={calculationOpen} />
              </button>
              {calculationOpen && (
                <Suspense fallback={<div className="mt-2 text-[11px] text-muted-foreground animate-pulse">…</div>}>
                  <IntoView>
                    <CalculationDetails data={data} />
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
        ⏱ update {data.seq}{data.partial ? ' (partial)' : ''} · {stats.rings} ring{stats.rings === 1 ? '' : 's'} · {stats.reads} reads · {(stats.bytes / 1e6).toFixed(1)} MB
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

/** A row of the list: a contributor, or the aircraft layer as a whole; ranked by its share. */
interface ListEntry extends HiddenRow {
  rank: number
}
