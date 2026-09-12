// Detail for a clicked network station on the validation QA map (`/#val=1`) —
// a measurement from a committed snapshot next to the model value from that
// network's Δ table. Stations standing on a fixture anchor never reach this
// card: the API folds them into the anchor (see validation-view.ts), so the
// map draws and explains one monitor once.
import { fmt as fmtSigned, fmtFloat } from '../utils/formatters'
import { Link, Row, runDay } from './validation-parts'
import type { ValidationNetwork, ValidationStation } from './ValidationLayer'

const STATION_COLOR: Record<string, string> = {
  above: '#c62828', within_bound: '#2e7d32', below: '#ef6c00',
  unattributable: '#78909c', trend_only: '#5c6bc0',
  error: '#c62828', no_coverage: '#bdbdbd',
}
const FALLBACK_COLOR = '#8d6e63'

const LEVEL_METRICS = ['lden', 'ld', 'le', 'ln', 'laeq_24h', 'laeq_tag_0622', 'laeq_nacht_2206'] as const
const TRAFFIC_METRICS = ['trains_per_day', 'freight_trains_per_day', 'trains_night', 'mean_speed_kmh', 'mean_train_length_m'] as const

const VERDICT_EN: Record<string, string> = {
  'within_bound': 'Matches the measurement.',
  'above': 'Model louder than measured.',
  'below': 'Model quieter than measured.',
  'unattributable': 'Different conditions — not comparable.',
  'trend_only': 'Trend anchor, no verdict.',
  'error': 'Query failed.',
  'no_probe': 'Station not found in model data.',
  'no_model': 'No model value.',
  'no-delta': 'No model data — regenerate the delta table.',
}

export default function StationCard({ s, net }: { s: ValidationStation; net: ValidationNetwork }) {
  const verdict = s.verdict ?? 'no-delta'
  const color = STATION_COLOR[verdict] ?? FALLBACK_COLOR
  const note = (net.commensurability as Record<string, string | undefined>).note
  const day = runDay(net.delta_meta)
  return (
    <>
      <div className="font-semibold">{s.name}</div>
      <div className="mb-1 text-[13px]">
        Measured <b>{fmtFloat(s.measured_value)} dB</b> ({s.months_covered ?? '?'} mo, {net.year})
        {' · '}model <b>{fmtFloat(s.model_value)} dB</b>{day ? ` (${day})` : ''}
        {' → '}<span className="font-semibold" style={{ color }}>{verdict}</span>
      </div>
      <div className="mb-1 text-[11px] text-muted-foreground">{VERDICT_EN[verdict] ?? ''}</div>
      <details className="mt-1 text-[11px] text-muted-foreground">
        <summary className="cursor-pointer">evidence</summary>
        <table className="w-full border-collapse">
          <tbody>
            {s.delta_db != null ? (
              <Row label={`Δ ${s.model_metric_field}−${s.measured_metric_field}`}>
                <b>{fmtSigned(s.delta_db)}</b> dB → <span className="font-semibold" style={{ color }}>{verdict}</span>
              </Row>
            ) : (
              <Row label="comparison">
                <span className="font-semibold" style={{ color }}>{verdict}</span>
                {net.comparison_mode === 'trend_only' ? ' (explicit trend-only anchor)' : ''}
              </Row>
            )}
            <Row label="comparison mode">
              {net.comparison_mode}
              {net.comparison_tolerance_db != null
                ? net.comparison_mode === 'upper_bound'
                  ? ` · +${net.comparison_tolerance_db} dB upper allowance`
                  : ` · ±${net.comparison_tolerance_db} dB`
                : ''}
            </Row>
            {net.comparison_tolerance_basis && <Row label="tolerance basis">{net.comparison_tolerance_basis}</Row>}
            <Row label="explicit values">
              measured {s.measured_metric_field} <b>{fmtFloat(s.measured_value)}</b> · model {s.model_metric_field} <b>{fmtFloat(s.model_value)}</b>
            </Row>
            {s.dominant_source && <Row label="model dominant">{s.dominant_source}</Row>}
            {LEVEL_METRICS.filter((k) => s[k] != null).map((k) => (
              <Row key={k} label={k}>
                measured <b>{fmtFloat(s[k] as number)}</b>
                {s.model?.[k] != null ? <> · model {fmtFloat(s.model[k])}</> : null}
              </Row>
            ))}
            {TRAFFIC_METRICS.filter((k) => s[k] != null).map((k) => (
              <Row key={k} label={k.replaceAll('_', ' ')}>{String(s[k])}</Row>
            ))}
            {s.months_covered != null && <Row label="coverage">{s.months_covered} mo · {s.coverage_pct ?? '—'} %</Row>}
          </tbody>
        </table>
        {note && <div className="mt-1 whitespace-pre-wrap border-l-2 border-neutral-200 bg-neutral-50 p-1.5">{note}</div>}
        <div className="mt-1">{net.license}<Link url={net.source_url}>source</Link></div>
      </details>
    </>
  )
}
