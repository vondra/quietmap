// The popup body: the total level, the loudest contributors and the aircraft layer, and whether the
// answer is still being refined; or, once the click is answered, how it was computed (segments).
// Redrawn on every streamed update of the click.
import { useState } from 'react'
import { ldenToColor } from '../utils/noise-colors'
import { DataPoint } from './noise/noise-tooltips'
import { fmtDb, txtTable } from '../utils/formatters'
import { PERIOD_LABELS_DETAIL, SOURCE_LABELS } from './noise/shared'
import { AircraftLayerRow, ContributorRow } from './noise/source/ContributorRow'
import { SegmentsSection } from './noise/segments/SegmentsSection'
import type { PopupLoudness, PopupPercentiles, PopupUpdate, SegmentFan } from '../types/noise'

// The read and compute statistics of the click are for profiling, not for visitors: shown only
// when the URL carries ?timings.
const SHOW_STATS = typeof location !== 'undefined' && new URLSearchParams(location.search).has('timings')

export interface NoiseDetailContentProps {
  data: PopupUpdate
  maxSources?: number
  /** Shows a loudest flight's track (by `topFlightKey`) or a contributor's pieces (by
   * `source:<id>`) on the map; null shows none. */
  onHighlight: (key: string | null) => void
  /** Draws the segments view's rays on the map; null clears them. */
  onFan?: (fan: SegmentFan | null) => void
}

export default function NoiseDetailContent({ data, maxSources, onHighlight, onFan }: NoiseDetailContentProps) {
  const [centerLat, centerLng] = data.center
  const [segments, setSegments] = useState(false)
  const answered = data.total_lden != null && !data.partial
  // The popup's 0 dB display floor, applied to this list the way the per-layer rows apply it.
  const audibleContributors = data.top_contributors.filter(c => c.received_lden != null && c.received_lden > 0)
  const rows = audibleContributors.map(c => (
    <ContributorRow key={`${c.source_type}-${c.id}`} c={c} onHighlight={id => onHighlight(id === null ? null : `source:${id}`)} />
  ))
  // The aircraft layer lists no contributors: the layer is one row, at its rank by Lden.
  const aircraft = data.sources.find(s => s.source_type === 'aircraft')
  const aircraftLden = aircraft?.lden ?? 0
  if (aircraft && aircraftLden > 0) {
    const rank = audibleContributors.findIndex(c => (c.received_lden ?? 0) < aircraftLden)
    rows.splice(rank < 0 ? rows.length : rank, 0,
      <AircraftLayerRow key="aircraft" layer={aircraft} flights={data.top_flights} onHighlightFlight={onHighlight} />)
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

  return (
    <div data-testid="detail-popup" role="dialog" className="px-2.5 pt-1 pb-2" onClick={(e) => e.stopPropagation()}>
      <div className="flex items-center justify-between mb-1">
        {data.total_lden != null ? (
          <span
            data-testid="noise-badge"
            className="text-2xl font-bold leading-none shrink-0 whitespace-nowrap"
            style={{ color: ldenToColor(data.total_lden) }}
          >
            <DataPoint title="Total Lden — energy sum across all sources" text={totalLdenText}>
              {data.total_lden.toFixed(1)} dB
            </DataPoint>
          </span>
        ) : <span />}
        <div className="text-right pr-6">
          <div className="text-xs text-muted-foreground/60 font-mono leading-tight">
            {centerLat.toFixed(4)}, {centerLng.toFixed(4)}
          </div>
          {data.elevation_m > 0 && (
            <div className="text-xs text-muted-foreground/60 font-mono leading-tight">{Math.round(data.elevation_m)} m a.s.l.</div>
          )}
          {data.partial && (
            <div
              data-testid="popup-refining"
              className="text-xs text-muted-foreground/60 font-mono leading-tight animate-pulse"
              title="The levels shown are valid; farther sources are still being added."
            >
              refining…
            </div>
          )}
        </div>
      </div>
      {data.loudness && <LoudnessLine loudness={data.loudness} />}
      {data.percentiles?.audible_percent && <HeardLine heard={data.percentiles.audible_percent} />}
      {data.total_lden != null ? (
        <>
          <div className="flex items-baseline justify-between border-b border-border pb-0.5 mb-0.5">
            <span className="text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
              {segments && answered ? 'How it is computed' : `Noise sources (${rows.length})`}
            </span>
            {answered && (
              <button
                type="button"
                data-testid="segments-toggle"
                className="text-[11px] text-muted-foreground hover:text-foreground"
                aria-expanded={segments}
                onClick={() => setSegments(!segments)}
              >
                {segments ? '◂ Sources' : 'Segments ▸'}
              </button>
            )}
          </div>
          <div className="overflow-y-auto overflow-x-clip" style={{ maxHeight: 'max(100dvh - 400px, 160px)' }}>
            {segments && answered
              ? (
                <SegmentsSection
                  lat={centerLat}
                  lng={centerLng}
                  building={data.building}
                  elevationM={data.elevation_m}
                  reflectionDb={data.reflection_db ?? 0}
                  layers={data.sources}
                  contributors={data.top_contributors}
                  onFan={onFan}
                />
              )
              : shown}
          </div>
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

// One line under the level: how loud the place sounds (N5 in sone), by day and at night, with the
// evening and the scale in the hover.
function LoudnessLine({ loudness }: { loudness: PopupLoudness }) {
  const { day, evening, night } = loudness.n5_sone
  if (day == null) return null
  const sone = (value: number | null) => (value == null ? '–' : `${value} sone`)
  const text = txtTable([
    ['Day 07–19', sone(day)],
    ['Evening 19–23', sone(evening)],
    ['Night 23–07', sone(night)],
    '',
    'Loudness N5: how loud the sound heard 5 % of',
    'the time is to the ear (Zwicker, ISO 532-1), the',
    'measure of psychoacoustic and soundscape research.',
    'Twice the sone sounds twice as loud: about 5 in',
    'a quiet park or suburb, 35 in a busy city street,',
    '55 beside a city motorway.',
  ], 22, 9)
  return (
    <div data-testid="loudness" className="text-sm mb-1">
      <DataPoint title="Loudness" text={text}>
        <span className="font-semibold">{sone(day)}</span>
        <span className="text-muted-foreground"> by day{night != null ? `, ${sone(night)} at night` : ''}</span>
      </DataPoint>
    </div>
  )
}

// One line under the loudness: how much of the day and of the night human noise stands above a
// quiet natural background, with the evening and the method in the hover.
function HeardLine({ heard }: { heard: NonNullable<PopupPercentiles['audible_percent']> }) {
  const percent = (value: number) => {
    if (value > 0 && value < 0.5) return 'under 1 %'
    if (value >= 99.5 && value < 100) return 'over 99 %'
    return `${Math.round(value)} %`
  }
  const text = txtTable([
    ['Day 07–19', percent(heard.day)],
    ['Evening 19–23', percent(heard.evening)],
    ['Night 23–07', percent(heard.night)],
    '',
    'The share of the time the noise of roads,',
    'railways, aircraft, industry, buildings and',
    'ships together stands above a quiet natural',
    'background (leaves, birds, a distant stream):',
    '30 dB(A) by day and evening, 25 at night. It',
    'follows how often vehicles and flights pass',
    'and how often the weather bends the sound',
    'down here (percent time audible, as the US',
    'national parks measure their soundscapes).',
  ], 22, 9)
  return (
    <div data-testid="heard" className="text-sm mb-1">
      <DataPoint title="Human noise heard" text={text}>
        <span className="text-muted-foreground">Human noise heard: day </span>
        <span className="font-semibold">{percent(heard.day)}</span>
        <span className="text-muted-foreground">, night </span>
        <span className="font-semibold">{percent(heard.night)}</span>
      </DataPoint>
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
