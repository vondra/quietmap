import { useMemo } from 'react'
import type { SegmentTrace } from '../../../types/noise'
import { HoverText } from '../../ui/info-tip'
import { isLineSourceKind } from '../shared'
import { CruiseLoudestFlights } from './CruiseLoudestFlights'
import { emissionInputRows } from './emission-rows'
import { Section, InlineTable } from './display'

export function Section1Source({ trace }: { trace: SegmentTrace }) {
  const emissionRowsList = useMemo(() => emissionInputRows(trace), [trace])
  const lwRow = useMemo(() => computeLwRow(trace), [trace])
  const sourceHeightRow = useMemo(() => computeSourceHeightRow(trace), [trace])
  const rows = useMemo(
    () => {
      const out: [React.ReactNode, React.ReactNode][] = [...emissionRowsList]
      if (lwRow) out.push(lwRow)
      if (sourceHeightRow) out.push(sourceHeightRow)
      return out
    },
    [emissionRowsList, lwRow, sourceHeightRow],
  )
  return (
    <Section>
      <InlineTable rows={rows} />
      {/* Separate block, not an InlineTable row: its `<table>` can't live
          inside InlineTable's `<span>` value cell. */}
      {trace.kind === 'aircraft' && trace.aircraft_subtype === 3 && trace.cruise_top_flights && trace.cruise_top_flights.length > 0 && (
        <CruiseLoudestFlights tops={trace.cruise_top_flights} />
      )}
    </Section>
  )
}

// Source height is the single row inherited from the removed §2 Baseline
// section — value is a per-source-type model constant (road 0.05 m wheel,
// rail 0.5 m wheel-rail, building mid-facade, industrial varies, aircraft
// ground 4 m), not a computed field. Tooltip explains the convention.
function computeSourceHeightRow(trace: SegmentTrace): [React.ReactNode, React.ReactNode] | null {
  // Aircraft ground-ops + airborne rows hide Source height: ground
  // is a fixed CNOSSOS 4 m model constant (no info per row); airborne
  // duplicates `Altitude at CPA` byte-for-byte (see `traces.rs:213`
  // — `baseline.source_height_m = altitude_m_at_cpa` for airborne
  // sub-segments). Other source types still show it because they
  // vary (rail 0.5 m, building mid-facade, industrial NACE-dependent).
  if (trace.kind === 'aircraft' && (trace.aircraft_subtype === 1 || trace.aircraft_subtype === 2)) {
    return null
  }
  if (trace.propagation.model !== 'cnossos') return null
  const h = trace.propagation.baseline.source_height_m
  const slantSame = Math.abs(trace.d_slant_m - trace.dist_m) < 0.5
  const slantNote = slantSame
    ? ''
    : `\n\nSlant distance: ${trace.d_slant_m.toFixed(0)} m vs horizontal ${trace.dist_m.toFixed(0)} m — the vertical offset from source height contributes.`
  const tooltip =
    'Source height above ground — model constant per source type:\n' +
    '  road       0.05 m   (wheel/tyre contact)\n' +
    '  railway    0.5 m    (wheel-rail contact)\n' +
    '  aircraft   4.0 m    (ground ops)\n' +
    '  building   height/2 (mid-facade)\n' +
    '  industrial varies   (5 m other / 8 m quarry / 10 m heavy NACE;\n' +
    '                       wind turbine = hub height)\n\n' +
    `Feeds geometric divergence (d_slant = √(d_horizontal² + Δh²)) and\n` +
    'diffraction S/R anchor points.' +
    slantNote
  return [
    <HoverText title={tooltip}>Source height</HoverText>,
    `${h.toFixed(1)} m`,
  ]
}

const LW_LINE_SOURCE = {
  unit: 'dB(A)/m',
  symbol: "L'w",
  label: "L'w (day)",
  desc: 'line-source power density',
} as const
const LW_POINT_SOURCE = {
  unit: 'dB(A)',
  symbol: 'Lw',
  label: 'Lw (day)',
  desc: 'point-source sound power',
} as const
const POINT_SOURCE_KINDS = new Set(['building', 'industrial'])

function computeLwRow(trace: SegmentTrace): [React.ReactNode, React.ReactNode] | null {
  if (trace.propagation.model !== 'cnossos') return null
  const lw = trace.propagation.lw_db_a
  if (!lw) return null
  // Aircraft ground-ops SegmentTraces emit silent (NEG_INFINITY →
  // JSON null) lw_db_a values per period because per-microsegment
  // Lw isn't computed today — the airport-aggregate emission_db
  // lives on the parent Contributor instead. Skip the row when any
  // period's Lw is missing rather than crashing on null.toFixed.
  if (!Number.isFinite(lw.day) || !Number.isFinite(lw.evening) || !Number.isFinite(lw.night)) {
    return null
  }
  const kind = trace.emission.kind
  const cfg = isLineSourceKind(kind)
    ? LW_LINE_SOURCE
    : POINT_SOURCE_KINDS.has(kind)
      ? LW_POINT_SOURCE
      : null
  if (!cfg) return null
  const labelConcept =
    `${cfg.symbol} — ${cfg.desc}, A-weighted. Sound power at the\n` +
    'source before any propagation is applied. CNOSSOS-EU Annex II\n' +
    '(road, railway) / project emission model (building, industrial).'
  const valueComputation =
    `Per-period ${cfg.symbol}:\n\n` +
    `  day      ${lw.day.toFixed(1).padStart(6)} ${cfg.unit}\n` +
    `  evening  ${lw.evening.toFixed(1).padStart(6)} ${cfg.unit}\n` +
    `  night    ${lw.night.toFixed(1).padStart(6)} ${cfg.unit}\n\n` +
    `Day shown is the representative period. Per-band L_w lives\n` +
    'in §5 under "Lw (A)" — hover each period cell there.'
  return [
    <HoverText title={labelConcept}>{cfg.label}</HoverText>,
    <HoverText title={valueComputation}>{`${lw.day.toFixed(1)} ${cfg.unit}`}</HoverText>,
  ]
}
