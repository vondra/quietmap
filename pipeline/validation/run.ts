/**
 * Validation runner: query the popup at every catalogue station (at the microphone's height when
 * the server supports it), score native indicators and guards, and write one run directory.
 *
 * Run: node --import tsx validation/run.ts --server http://127.0.0.1:8600 \
 *   --catalogue <dir of *.jsonl | file> --out <new run dir> [--identity <ops identity json>] \
 *   [--label <name>] [--receiver-height microphone|engine-default] [--concurrency 6] [--position-samples]
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { loadCatalogue, parseIndicator, stationDefaultLayer, type CatalogueStation } from './catalogue.ts'
import { compareIndicator, evaluateGuard } from './comparison.ts'
import { readPopupAnswer, type PopupAnswer } from './popup.ts'
import { latency, renderReport, type StationRow } from './report.ts'

/** The engine's receiver height (`DEFAULT_RECEIVER_HEIGHT`) and floor (`RECEIVER_HEIGHT_FLOOR_M`). */
const ENGINE_DEFAULT_RECEIVER_HEIGHT_M = 4
const ENGINE_RECEIVER_HEIGHT_FLOOR_M = 0.5
const INSTANCE_HEADER = 'x-0db-instance'
const REQUEST_TIMEOUT_MS = 120_000

type ModelCohort = { cohort_id: string; cache_ttl_ms: number; runtime_sha256: string; prepared_sha256: string; cohort_unstable?: true }

const { values: args } = parseArgs({
  options: {
    server: { type: 'string' },
    catalogue: { type: 'string' },
    out: { type: 'string' },
    identity: { type: 'string' },
    label: { type: 'string', default: '' },
    'receiver-height': { type: 'string', default: 'microphone' },
    concurrency: { type: 'string', default: '6' },
    'position-samples': { type: 'boolean', default: false },
  },
})
/** Criteria u_pos: the model on this many points around the circle of the documented position uncertainty. */
const POSITION_SAMPLE_BEARINGS_DEG = [0, 45, 90, 135, 180, 225, 270, 315]
const METRES_PER_DEGREE_LATITUDE = 111_320
if (!args.server || !args.catalogue || !args.out) {
  console.error('usage: run.ts --server URL --catalogue PATH --out NEW_DIR [--identity JSON] [--label NAME]')
  process.exit(2)
}
if (args['receiver-height'] !== 'microphone' && args['receiver-height'] !== 'engine-default') {
  console.error('--receiver-height must be microphone or engine-default')
  process.exit(2)
}
const server = args.server.replace(/\/$/, '')
const concurrency = Number(args.concurrency)
const outDir = resolve(args.out)
if (existsSync(outDir)) throw new Error(`${outDir} exists: a run directory is written once`)

const catalogue = loadCatalogue(resolve(args.catalogue))
const health = await fetch(`${server}/api/health`, { signal: AbortSignal.timeout(5000) })
const instance = health.headers.get(INSTANCE_HEADER)
if (!health.ok || !instance) throw new Error(`${server}: unhealthy or without ${INSTANCE_HEADER}`)

async function cohort(): Promise<ModelCohort> {
  const response = await fetch(`${server}/api/validation/cohort`, { signal: AbortSignal.timeout(120_000) })
  if (!response.ok || response.headers.get(INSTANCE_HEADER) !== instance) throw new Error('cohort unavailable or server restarted')
  const value = await response.json() as ModelCohort
  if (value.cohort_unstable) throw new Error('server files changed under the running process; restart it before validating')
  return value
}

async function popup(lat: number, lng: number, receiverHeightM: number | null): Promise<PopupAnswer> {
  const query = `lat=${lat}&lng=${lng}${receiverHeightM == null ? '' : `&receiver_height_m=${receiverHeightM}`}`
  const response = await fetch(`${server}/api/noise-onfly-v2?${query}`, { signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) })
  if (response.headers.get(INSTANCE_HEADER) !== instance) throw new Error('server instance changed')
  if (!response.ok) throw new Error(`HTTP ${response.status}`)
  return await response.json() as PopupAnswer
}

async function positionSamples(station: CatalogueStation, radiusM: number, receiverHeightM: number | null): Promise<StationRow['position_samples']> {
  const samples: NonNullable<StationRow['position_samples']> = []
  for (const bearing of POSITION_SAMPLE_BEARINGS_DEG) {
    const lat = station.lat + radiusM * Math.cos(bearing * Math.PI / 180) / METRES_PER_DEGREE_LATITUDE
    const lng = station.lng + radiusM * Math.sin(bearing * Math.PI / 180) / (METRES_PER_DEGREE_LATITUDE * Math.cos(station.lat * Math.PI / 180))
    const model = readPopupAnswer(await popup(lat, lng, receiverHeightM), { lat, lng })
    samples.push({ bearing_deg: bearing, lden: model.total.lden, inside_footprint: model.inside_footprint })
  }
  return samples
}

function requestedHeight(station: CatalogueStation): number | null {
  if (args['receiver-height'] === 'engine-default' || station.mic_height_m == null) return null
  return Math.max(station.mic_height_m, ENGINE_RECEIVER_HEIGHT_FLOOR_M)
}

async function scoreStation(station: CatalogueStation): Promise<StationRow> {
  const requested = requestedHeight(station)
  const parsed = Object.entries(station.indicators)
    .map(([key, value]) => ({ key, result: parseIndicator(key, value, station.native_periods, stationDefaultLayer(station)) }))
  const scoring: StationRow['scoring'] = station.diagnostic_only ? 'diagnostic_only' : 'accuracy'
  const number = (value: unknown) => typeof value === 'number' ? value : null
  const base: StationRow = {
    key: station.station_id, set: station.set, station_id: station.station_id, name: station.name,
    lat: station.lat, lng: station.lng, expected_source: station.expected_source, truth_kind: station.truth_kind,
    measurand: station.measurand ?? 'sound_level', year: number(station.year), months_covered: number(station.months_covered),
    holdout: station.holdout, holdout_square: station.z9_holdout_square === true, scoring,
    scoring_reason: scoring === 'diagnostic_only' ? String(station.diagnostic_reason ?? '') : null,
    mount: typeof station.mount === 'string' ? station.mount : null, facade_distance_m: number(station.facade_distance_m),
    publisher_facade_correction_db: number(station.publisher_facade_correction_db), position_uncertainty_m: number(station.position_uncertainty_m),
    mic_height_m: station.mic_height_m, requested_receiver_height_m: requested,
    receiver_height_used_m: null, height_matches_microphone: false, request_ms: 0, model: null, comparisons: [], guard: null, position_samples: null,
    unsupported_indicators: parsed.flatMap(entry => 'unsupported' in entry.result ? [`${entry.key}: ${entry.result.unsupported}`] : []),
    error: null,
  }
  const started = performance.now()
  try {
    const answer = await popup(station.lat, station.lng, requested)
    base.request_ms = Math.round(performance.now() - started)
    const model = readPopupAnswer(answer, station)
    // A server without the height parameter ignores it and answers at the engine default.
    const used = model.receiver.height_m ?? ENGINE_DEFAULT_RECEIVER_HEIGHT_M
    if (requested != null && model.receiver.height_m != null && Math.abs(used - requested) > 1e-9) {
      throw new Error(`server computed ${used} m for a requested ${requested} m`)
    }
    return {
      ...base, model, receiver_height_used_m: used,
      height_matches_microphone: station.mic_height_m != null && Math.abs(used - station.mic_height_m) < 1e-9,
      comparisons: parsed.flatMap(entry => 'indicator' in entry.result ? [compareIndicator(entry.result.indicator, model)] : []),
      guard: station.guard ? evaluateGuard(station, model) : null,
      position_samples: args['position-samples'] && scoring === 'accuracy' && base.position_uncertainty_m
        ? await positionSamples(station, base.position_uncertainty_m, requested) : null,
    }
  } catch (error) {
    return { ...base, request_ms: Math.round(performance.now() - started), error: error instanceof Error ? error.message : String(error) }
  }
}

function productIdentity(): Record<string, unknown> {
  const repo = resolve(import.meta.dirname, '../..')
  const git = (...command: string[]) => execFileSync('git', ['-C', repo, ...command], { encoding: 'utf8' }).trim()
  return { runner_commit: git('rev-parse', 'HEAD'), runner_dirty_files: git('status', '--porcelain', '--', 'pipeline/validation').split('\n').filter(Boolean) }
}

const startedAt = new Date()
const initialCohort = await cohort()
const rows: StationRow[] = new Array(catalogue.stations.length)
let next = 0
await Promise.all(Array.from({ length: concurrency }, async () => {
  while (next < catalogue.stations.length) {
    const index = next++
    rows[index] = await scoreStation(catalogue.stations[index])
    if ((index + 1) % 25 === 0) console.error(`  ${index + 1}/${catalogue.stations.length}`)
  }
}))
await new Promise(wait => setTimeout(wait, initialCohort.cache_ttl_ms + 25))
const finalCohort = await cohort()
if (finalCohort.cohort_id !== initialCohort.cohort_id) throw new Error('model/data cohort changed during the run; discarding it')
const runSeconds = (Date.now() - startedAt.getTime()) / 1000

const identity = {
  label: args.label,
  started_utc: startedAt.toISOString(),
  run_seconds: runSeconds,
  server,
  server_instance: instance,
  server_cohort: initialCohort,
  receiver_height_mode: args['receiver-height'],
  position_samples: args['position-samples'],
  concurrency,
  latency: latency(rows),
  catalogue: catalogue.files,
  ...productIdentity(),
  ...(args.identity ? { ops: JSON.parse(readFileSync(resolve(args.identity), 'utf8')) as unknown } : {}),
}
mkdirSync(outDir, { recursive: true })
writeFileSync(resolve(outDir, 'identity.json'), JSON.stringify(identity, null, 2) + '\n', { flag: 'wx' })
writeFileSync(resolve(outDir, 'stations.jsonl'), rows.map(row => JSON.stringify(row)).join('\n') + '\n', { flag: 'wx' })
writeFileSync(resolve(outDir, 'report.md'), renderReport(rows, identity, runSeconds), { flag: 'wx' })
console.error(`[validation] ${rows.length} stations → ${outDir}`)
