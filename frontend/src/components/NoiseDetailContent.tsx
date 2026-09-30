// The popup body: the total level, the building notice, the loudest contributors and the aircraft
// layer, and whether the answer is still being refined. Redrawn on every streamed update of the click.
import { ldenToColor } from '../utils/noise-colors'
import { DataPoint } from './noise/noise-tooltips'
import { HoverText } from './ui/info-tip'
import { fmtDb, txtTable, type TableRow } from '../utils/formatters'
import { fieldText, PERIOD_LABELS_DETAIL, SOURCE_LABELS } from './noise/shared'
import { AircraftLayerRow, ContributorRow } from './noise/source/ContributorRow'
import type { BuildingAnswer, PopupUpdate } from '../types/noise'

// The read and compute statistics of the click are for profiling, not for visitors: shown only
// when the URL carries ?timings.
const SHOW_STATS = typeof location !== 'undefined' && new URLSearchParams(location.search).has('timings')

export interface NoiseDetailContentProps {
  data: PopupUpdate
  maxSources?: number
  /** Shows a loudest flight's track on the map, by `topFlightKey`; null shows none. */
  onHighlightFlight: (key: string | null) => void
}

export default function NoiseDetailContent({ data, maxSources, onHighlightFlight }: NoiseDetailContentProps) {
  const [centerLat, centerLng] = data.center
  // The popup's 0 dB display floor, applied to this list the way the per-layer rows apply it.
  const audibleContributors = data.top_contributors.filter(c => c.received_lden != null && c.received_lden > 0)
  const rows = audibleContributors.map(c => <ContributorRow key={`${c.source_type}-${c.id}`} c={c} />)
  // The aircraft layer lists no contributors: the layer is one row, at its rank by Lden.
  const aircraft = data.sources.find(s => s.source_type === 'aircraft')
  const aircraftLden = aircraft?.lden ?? 0
  if (aircraft && aircraftLden > 0) {
    const rank = audibleContributors.findIndex(c => (c.received_lden ?? 0) < aircraftLden)
    rows.splice(rank < 0 ? rows.length : rank, 0,
      <AircraftLayerRow key="aircraft" layer={aircraft} flights={data.top_flights} onHighlightFlight={onHighlightFlight} />)
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
      <BuildingNotice building={data.building} />
      {data.total_lden != null ? (
        <>
          <div className="border-b border-border pb-0.5 mb-0.5">
            <span className="text-[11px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
              Noise sources ({rows.length})
            </span>
          </div>
          <div className="overflow-y-auto overflow-x-clip" style={{ maxHeight: 'max(100dvh - 400px, 160px)' }}>
            {shown}
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

const COMPASS_POINTS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'] as const

function compassPoint(bearingDeg: number): string {
  return COMPASS_POINTS[Math.round((((bearingDeg % 360) + 360) % 360) / 45) % 8]
}

// One line above the source rows: every level in this popup is the building's loudest façade
// receiver, not the clicked point. Fields the popup has no words for are listed in the hover.
const BUILDING_FIELDS_IN_WORDS = new Set(['id', 'height_m', 'facade', 'facade_receivers'])

function BuildingNotice({ building }: { building: BuildingAnswer | null }) {
  if (!building) return null
  const { height_m, facade, facade_receivers } = building
  const others = Object.entries(building).filter(([name]) => !BUILDING_FIELDS_IN_WORDS.has(name))
  const faces = facade ? ` — faces ${compassPoint(facade.bearing_deg)}` : ''
  const points = typeof facade_receivers === 'number' && facade_receivers > 0
    ? `1 of ${facade_receivers} façade point${facade_receivers === 1 ? '' : 's'}`
    : ''
  const rows: TableRow[] = [
    ...(facade
      ? [
          `The level is computed at ${facade.receiver[0].toFixed(5)}, ${facade.receiver[1].toFixed(5)}:`,
          '0.1 m in front of this façade, 4 m above ground, as EU',
          'noise mapping does for building exposure. Of the',
          'building\'s façade points it is the loudest by Lden of',
          'all sources together.',
        ]
      : ['This building has no exposed façade.']),
    '',
    ['Building height', `${height_m.toFixed(1)} m`],
    ...others.map(([name, value]) => [name.replace(/_/g, ' '), fieldText(value)] as [string, string]),
  ]
  return (
    <div data-testid="building-exposure" className="mb-1 border-b border-border/50">
      <HoverText title={txtTable(rows, 16, 12)} className="block" focusable>
        <span className="flex items-baseline gap-1.5 px-0 py-1 text-xs font-medium">
          <span className="truncate flex-1">Noisiest façade of this building{faces}</span>
          <span className="shrink-0 text-right tabular-nums font-normal text-muted-foreground/70">{points}</span>
        </span>
      </HoverText>
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
