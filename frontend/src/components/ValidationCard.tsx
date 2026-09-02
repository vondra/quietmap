// Right-column card for a clicked validation anchor on the single React QA
// map: model vs external truth, both bands, the external value's kind,
// provenance, tags. Clicking a dot also opens the
// ordinary noise DetailCard for the same spot, so this card deliberately
// repeats no live-model breakdown.
import type { ValidationArtifactMeta, ValidationPayload, ValidationSelection } from './ValidationLayer'

const FIXTURE_COLOR: Record<string, string> = {
  'OK': '#2e7d32', 'EXTERNAL-GAP': '#ef6c00', 'KNOWN-GAP': '#8e24aa',
  'PENDING': '#757575', 'DRIFT': '#c62828', 'ERROR': '#c62828', 'SKIPPED': '#bdbdbd',
}
const STATION_COLOR: Record<string, string> = {
  above: '#c62828', within_bound: '#2e7d32', below: '#ef6c00',
  unattributable: '#78909c', trend_only: '#5c6bc0',
  error: '#c62828', no_coverage: '#bdbdbd',
}

const fmt = (v: number | null | undefined, digits = 1) => (v == null ? '—' : v.toFixed(digits))
const band = (b: [number | null, number | null] | null | undefined) =>
  b ? `[${b[0] ?? '·'}, ${b[1] ?? '·'}]` : '—'
const safeUrl = (u: string | null | undefined) => (typeof u === 'string' && /^https?:\/\//i.test(u) ? u : null)
const shortCohort = (id: string | null | undefined) => id ? id.slice(0, 12) : 'unavailable'
const EXTERNAL_VALUE_LABEL: Record<string, string> = {
  measurement: 'they measure',
  official_map: 'official map/model',
}
const externalValueLabel = (anchorType: string) => EXTERNAL_VALUE_LABEL[anchorType] ?? 'external value'

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <tr>
      <td className="pr-2 py-0.5 align-top whitespace-nowrap text-muted-foreground">{label}</td>
      <td className="py-0.5 tabular-nums">{children}</td>
    </tr>
  )
}

export default function ValidationCard({ selection, onClose }: {
  selection: ValidationSelection
  onClose: () => void
}) {
  return (
    <div className="rounded-lg bg-white p-3 text-[13px] shadow max-h-[52vh] overflow-y-auto" style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.06)' }}>
      <button onClick={onClose} className="float-right text-muted-foreground hover:text-foreground" aria-label="Close validation card">×</button>
      {selection.kind === 'fixture' ? <FixtureBody f={selection.fixture} /> : <StationBody s={selection.station} net={selection.network} />}
    </div>
  )
}

function Meta({ value }: { value: ValidationArtifactMeta }) {
  const runner = value.runner_commit
    ? ` · runner ${value.runner_commit.slice(0, 12)}${value.runner_dirty ? '+dirty' : ''}${value.requested_data_year != null ? ` · requested data ${value.requested_data_year}` : ''}`
    : ''
  return (
    <span className="break-all">
      {value.generated_at ?? 'unknown time'} · queried {value.server ?? 'unknown server'} · cohort {shortCohort(value.model_cohort)}{runner}
    </span>
  )
}

/** Always-visible provenance/warning summary for the single React QA map. */
export function ValidationStatusCard({ payload }: { payload: ValidationPayload | null }) {
  const deltas = payload?.networks.filter(network => network.delta_meta != null) ?? []
  return (
    <div className="rounded-lg bg-white p-3 text-[11px] shadow" style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.06)' }}>
      <div className="font-semibold text-[13px]">Validation QA</div>
      {!payload ? <div className="text-muted-foreground">Loading catalogs…</div> : (
        <>
          <div className="mt-1"><b>current model cohort:</b> {shortCohort(payload.model_cohort?.cohort_id)}</div>
          <div><b>external truth:</b> {payload.networks.length} approved committed snapshots</div>
          <div><b>world model run:</b> {payload.lastrun ? <Meta value={payload.lastrun} /> : 'not available'}</div>
          <div><b>complete station-query artifacts:</b> {deltas.length}/{payload.networks.length}</div>
          {deltas.map(network => (
            <div key={`${network.network}:${network.year}`} className="pl-2 text-muted-foreground">
              {network.network} {network.year}: <Meta value={network.delta_meta!} />
            </div>
          ))}
          {payload.warnings.length > 0 && (
            <ul className="mt-1 list-disc pl-4 text-amber-800">
              {payload.warnings.map((warning, index) => <li key={`${index}:${warning}`}>{warning}</li>)}
            </ul>
          )}
        </>
      )}
    </div>
  )
}

function FixtureBody({ f }: { f: Extract<ValidationSelection, { kind: 'fixture' }>['fixture'] }) {
  const status = f.status ?? 'no-run'
  const url = safeUrl(f.external?.url)
  const c = f.commensurability as Record<string, string | number | undefined>
  return (
    <>
      <div className="font-semibold">{f.id}</div>
      <div className="mb-1 text-[11px] text-muted-foreground">
        validation fixture · {f.regime} · anchor {f.anchor_type} · role {f.role} · metric {f.metric_field}
      </div>
      <table className="w-full border-collapse">
        <tbody>
          <Row label="model (last run)">
            <b>{fmt(f.model_value, 2)}</b> dB → <span className="font-semibold" style={{ color: FIXTURE_COLOR[status] ?? '#8d6e63' }}>{status}</span>
          </Row>
          <Row label="regression band">{band(f.regression_band)} (drift {fmt(f.drift, 2)})</Row>
          <Row label="external band">
            {band(f.external?.band)}{f.ext ? <> → Δ {f.ext.delta > 0 ? '+' : ''}{fmt(f.ext.delta)} ({f.ext.side})</> : null}
          </Row>
          <Row label={externalValueLabel(f.anchor_type)}>{f.external?.value ?? '—'}</Row>
          <Row label="source">
            {f.external?.metric ?? ''} ({f.external?.year ?? '—'}, {f.external?.months_covered ?? '?'} mo)
            {url && <> · <a href={url} target="_blank" rel="noopener noreferrer" className="text-blue-700 underline">link</a></>}
          </Row>
          <Row label="convention">
            {c.metric_variant} · {c.dominance}
            {c.receiver_convention ? ` · ${c.receiver_convention}` : ''}
            {c.coord_uncertainty_m ? ` · ±${c.coord_uncertainty_m} m` : ''}
          </Row>
          {f.known_gap && <Row label="known gap"><b>{f.known_gap}</b></Row>}
          {f.pair_id && <Row label="pair/ladder">{f.pair_id}</Row>}
        </tbody>
      </table>
      <div className="mt-1">
        {(f.tags ?? []).map((t) => (
          <span key={t} className="mr-1 inline-block rounded bg-neutral-100 px-1 text-[11px]">{t}</span>
        ))}
      </div>
      <div className="mt-1 whitespace-pre-wrap border-l-2 border-neutral-200 bg-neutral-50 p-1.5 text-[11px]">{f.tolerance_note}</div>
      {f.caveats && <div className="mt-1 whitespace-pre-wrap border-l-2 border-neutral-200 bg-neutral-50 p-1.5 text-[11px]">{f.caveats}</div>}
    </>
  )
}

const LEVEL_METRICS = ['lden', 'ld', 'le', 'ln', 'laeq_24h', 'laeq_tag_0622', 'laeq_nacht_2206'] as const
const TRAFFIC_METRICS = ['trains_per_day', 'freight_trains_per_day', 'trains_night', 'mean_speed_kmh', 'mean_train_length_m'] as const

function StationBody({ s, net }: {
  s: Extract<ValidationSelection, { kind: 'station' }>['station']
  net: Extract<ValidationSelection, { kind: 'station' }>['network']
}) {
  const verdict = s.verdict ?? 'no-delta'
  const srcUrl = safeUrl(net.source?.[0])
  const note = (net.commensurability as Record<string, string | undefined>).note
  return (
    <>
      <div className="font-semibold">{s.name}</div>
      <div className="mb-1 text-[11px] text-muted-foreground">
        {net.network} {net.year} · station {s.station_id}{s.font ? ` · dominant on site: ${s.font}` : ''}
      </div>
      <table className="w-full border-collapse">
        <tbody>
          {s.delta_db != null ? (
            <Row label={`Δ ${s.model_metric_field}−${s.measured_metric_field}`}>
              <b>{s.delta_db > 0 ? '+' : ''}{fmt(s.delta_db)}</b> dB → <span className="font-semibold" style={{ color: STATION_COLOR[verdict] ?? '#8d6e63' }}>{verdict}</span>
            </Row>
          ) : (
            <Row label="comparison">
              <span className="font-semibold" style={{ color: STATION_COLOR[verdict] ?? '#8d6e63' }}>{verdict}</span>
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
            measured {s.measured_metric_field} <b>{fmt(s.measured_value)}</b> · model {s.model_metric_field} <b>{fmt(s.model_value)}</b>
          </Row>
          {s.dominant_source && <Row label="model dominant">{s.dominant_source}</Row>}
          {LEVEL_METRICS.filter((k) => s[k] != null).map((k) => (
            <Row key={k} label={k}>
              measured <b>{fmt(s[k] as number)}</b>
              {s.model?.[k] != null ? <> · model {fmt(s.model[k])}</> : null}
            </Row>
          ))}
          {TRAFFIC_METRICS.filter((k) => s[k] != null).map((k) => (
            <Row key={k} label={k.replaceAll('_', ' ')}>{String(s[k])}</Row>
          ))}
          {s.months_covered != null && <Row label="coverage">{s.months_covered} mo · {s.coverage_pct ?? '—'} %</Row>}
        </tbody>
      </table>
      {note && <div className="mt-1 whitespace-pre-wrap border-l-2 border-neutral-200 bg-neutral-50 p-1.5 text-[11px]">{note}</div>}
      <div className="mt-1 text-[11px] text-muted-foreground">
        {net.license}
        {srcUrl && <> · <a href={srcUrl} target="_blank" rel="noopener noreferrer" className="text-blue-700 underline">source</a></>}
      </div>
    </>
  )
}
