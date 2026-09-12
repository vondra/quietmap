// Right-column card for a clicked place on the validation QA map (`/#val=1`):
// what the outside world says here, what QuietMap says, and whether they agree.
// The card is about a PLACE, not a fixture — co-located anchors (an Lden/Ln
// pair) share one title, and a network station standing on an anchor arrives
// already folded into it as `also_measured` (see validation-view.ts). Clicking
// a dot also opens the ordinary noise DetailCard for the same spot, so this
// card deliberately repeats no live-model breakdown.
import { fmt as fmtSigned, fmtFloat } from '../utils/formatters'
import type {
  ValidationArtifactMeta, ValidationFixture, ValidationPayload, ValidationReading, ValidationSelection,
} from './ValidationLayer'
import StationCard from './ValidationStationCard'
import { Link, range, Row, runDay } from './validation-parts'

const FIXTURE_COLOR: Record<string, string> = {
  'OK': '#2e7d32', 'EXTERNAL-GAP': '#ef6c00', 'KNOWN-GAP': '#8e24aa',
  'PENDING': '#757575', 'DRIFT': '#c62828', 'ERROR': '#c62828', 'SKIPPED': '#bdbdbd',
}
/** What QuietMap reports at this point — the label on our own number. */
const METRIC_LABEL: Record<string, string> = { lden: 'Lden', ld: 'Ld', le: 'Le', ln: 'Ln' }
/** Measured fields carried by the committed network snapshots. */
const MEASURED_LABEL: Record<string, string> = {
  lden: 'Lden', laeq_tag_0622: 'day LAeq 06–22', laeq_aircraft_day_0622: 'aircraft day LAeq 06–22',
}
/** How our value sits against the source range — the sentence, not the enum. */
const SIDE_EN: Record<string, string> = {
  above: 'above that range', below: 'below that range',
  'below-unattributable': 'below that range — the source bounds total ambient, so a quieter model is not attributable',
}

const amount = (value: ValidationReading['value']) =>
  Array.isArray(value) ? range(value) : String(value)
const shortCohort = (id: string | null | undefined) => id ? id.slice(0, 12) : 'unavailable'

export default function ValidationCard({ selection, payload, onClose }: {
  selection: ValidationSelection
  payload: ValidationPayload | null
  onClose: () => void
}) {
  return (
    <div className="rounded-lg bg-white p-3 text-[13px] shadow max-h-[52vh] overflow-y-auto" style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.06)' }}>
      <button onClick={onClose} className="float-right text-muted-foreground hover:text-foreground" aria-label="Close validation card">×</button>
      {selection.kind === 'place'
        ? <PlaceBody fixtures={selection.fixtures} day={runDay(payload?.lastrun)} />
        : <StationCard s={selection.station} net={selection.network} />}
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

/** One-line system state for the QA map; machine detail hides in <details>. */
export function ValidationStatusCard({ payload }: { payload: ValidationPayload | null }) {
  const deltas = payload?.networks.filter(network => network.delta_meta != null) ?? []
  const counts: Record<string, number> = {}
  for (const f of payload?.fixtures ?? []) counts[f.status ?? 'no-run'] = (counts[f.status ?? 'no-run'] ?? 0) + 1
  const drift = (counts['DRIFT'] ?? 0) + (counts['ERROR'] ?? 0)
  const ok = counts['OK'] ?? 0
  const total = payload?.fixtures.length ?? 0
  return (
    <div className="rounded-lg bg-white px-3 py-2 text-[12px] shadow" style={{ boxShadow: '0 0 0 2px rgba(0,0,0,.06)' }}>
      {!payload ? <span className="text-muted-foreground">Loading…</span>
        : payload.lastrun == null ? (
          <span>Validation: no model data — run <code>/check-world</code>.</span>
        ) : (
          <span>Validation: OK {ok}/{total}{drift > 0 ? <>, <b>{drift} drift</b></> : null} · stations {deltas.length}/{payload.networks.length} <Details payload={payload} deltas={deltas} /></span>
        )}
    </div>
  )
}

function Details({ payload, deltas }: {
  payload: ValidationPayload
  deltas: ValidationPayload['networks']
}) {
  return (
    <details className="text-[11px] text-muted-foreground">
      <summary className="inline cursor-pointer">detail</summary>
      <div>cohort {shortCohort(payload.model_cohort?.cohort_id)} · run {payload.lastrun ? <Meta value={payload.lastrun} /> : 'n/a'}</div>
      {deltas.map(network => (
        <div key={`${network.network}:${network.year}`}>
          {network.network} {network.year}: <Meta value={network.delta_meta!} />
        </div>
      ))}
      {payload.warnings.length > 0 && (
        <ul className="list-disc pl-4 text-amber-800">
          {payload.warnings.map((warning, index) => <li key={`${index}:${warning}`}>{warning}</li>)}
        </ul>
      )}
    </details>
  )
}

/** The place: one title, then one block per anchor standing on it. */
function PlaceBody({ fixtures, day }: { fixtures: ValidationFixture[]; day: string }) {
  return (
    <>
      <div className="font-semibold">{fixtures[0].name}</div>
      <div className="text-[11px] text-muted-foreground">{fixtures[0].regime.replace('_', ' ')}</div>
      {fixtures.map((f, index) => (
        <div key={f.id} className={index > 0 ? 'mt-2 border-t border-neutral-200 pt-2' : 'mt-1'}>
          <AnchorBody f={f} day={day} sameSourceAsAbove={index > 0 && f.external.source === fixtures[index - 1].external.source} />
        </div>
      ))}
    </>
  )
}

/** The sentence worth a line — silence when the two numbers already say it. */
function verdict(f: ValidationFixture): string | null {
  switch (f.status ?? 'no-run') {
    case 'OK': return null
    case 'DRIFT': return f.drift != null
      ? `Moved ${fmtSigned(f.drift)} dB out of its verified state — under investigation.`
      : 'Outside its verified state — under investigation.'
    case 'KNOWN-GAP': return `Outside its verified state, documented as ${f.known_gap}.`
    case 'EXTERNAL-GAP': return 'Holds its verified state, but does not match the source.'
    case 'PENDING': return 'New point — no verified state pinned yet.'
    case 'SKIPPED': return 'No coverage here.'
    case 'ERROR': return 'The query failed.'
    default: return 'No model data — run /check-world.'
  }
}

function AnchorBody({ f, day, sameSourceAsAbove }: {
  f: ValidationFixture; day: string; sameSourceAsAbove: boolean
}) {
  const metric = METRIC_LABEL[f.metric_field] ?? f.metric_field
  const readings = f.external.readings ?? []
  const band = f.external.band
  const hasBand = band != null && (band[0] != null || band[1] != null)
  const sourceNumbers = readings.length > 0 || hasBand
  const readingLabel = f.anchor_type === 'measurement' ? 'Measured' : 'Source'
  const say = verdict(f)
  return (
    <>
      <table className="w-full border-collapse">
        <tbody>
          {!sameSourceAsAbove && (
            <Row label={sourceNumbers ? 'Source' : 'Context'}>
              {f.external.source}
              {f.external.year != null ? `, ${f.external.year}` : ''}
              {f.external.months_covered != null ? ` (${f.external.months_covered} mo)` : ''}
              <Link url={f.external.url} />
            </Row>
          )}
          {readings.map((reading) => (
            <Row key={reading.label} label={readingLabel}>
              {reading.label} <b>{amount(reading.value)}</b> {reading.unit}
            </Row>
          ))}
          {hasBand && <Row label="Compared against"><b>{range(band)}</b> dB {metric}</Row>}
          {f.also_measured.map((extra) => (
            <Row key={`${extra.source}:${extra.label}`} label="Also measured">
              {MEASURED_LABEL[extra.label] ?? extra.label} <b>{fmtFloat(extra.value)}</b> dB
              {' · '}{extra.source}<Link url={extra.url} />
            </Row>
          ))}
          <Row label="QuietMap">
            <b>{fmtFloat(f.model_value)}</b> dB {metric}{day ? ` · ${day}` : ''}
          </Row>
          {hasBand && f.ext && (
            <Row label="Difference">
              {f.ext.side === 'within'
                ? 'within that range'
                : <><b>{fmtSigned(f.ext.delta)}</b> dB {SIDE_EN[f.ext.side] ?? f.ext.side}</>}
            </Row>
          )}
          <Row label="Verified range">
            {range(f.regression_band)} dB
            {f.drift != null ? <span className="text-muted-foreground"> · moved {fmtSigned(f.drift, 2)} dB since the baseline</span> : null}
          </Row>
        </tbody>
      </table>
      {say && <div className="mt-1 text-[12px]" style={{ color: FIXTURE_COLOR[f.status ?? ''] ?? '#8d6e63' }}>{say}</div>}
      {!sourceNumbers && (
        <div className="mt-1 text-[12px] text-muted-foreground">
          No comparable external number here — this anchor only watches that our own value does not move.
          {f.external.note ? ` ${f.external.note}` : ''}
        </div>
      )}
      <details className="mt-1 text-[11px] text-muted-foreground">
        <summary className="cursor-pointer">evidence</summary>
        {sourceNumbers && f.external.note && <div className="mt-1">source: {f.external.note}</div>}
        <div className="mt-1">
          {(f.tags ?? []).map((t) => (
            <span key={t} className="mr-1 inline-block rounded bg-neutral-100 px-1 text-[11px]">{t}</span>
          ))}
        </div>
        <div className="mt-1">
          {f.id} · {f.anchor_type} · {f.role} · {String(f.commensurability.metric_variant)} · {String(f.commensurability.dominance)}
        </div>
        {f.external.annualization_method && <div className="mt-1">annualization: {f.external.annualization_method}</div>}
        <div className="mt-1 whitespace-pre-wrap border-l-2 border-neutral-200 bg-neutral-50 p-1.5">band: {f.tolerance_note}</div>
        {f.caveats && <div className="mt-1 whitespace-pre-wrap border-l-2 border-neutral-200 bg-neutral-50 p-1.5">caveat: {f.caveats}</div>}
      </details>
    </>
  )
}
