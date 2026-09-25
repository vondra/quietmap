/**
 * Validation runner: query the popup at every catalogue station (outdoors, at the microphone's
 * height when the server supports it) and at guard points, and write one run directory.
 *
 * Run: node --import tsx validation/run.ts --server http://127.0.0.1:8600 \
 *   --catalogue <dir of *.jsonl | file> --out <new run dir> --position-radius-default-m 15 \
 *   --facade-offset-default-m 2 [--identity <ops identity json>] [--label <name>] \
 *   [--receiver-height microphone|engine-default] [--concurrency 6] [--guard-points <json list>]
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { loadCatalogue, parseIndicator, stationDefaultLayer, type CatalogueStation } from './catalogue.ts'
import { compareIndicator, evaluateGuard } from './comparison.ts'
import { readPopupAnswer, type PopupAnswer, type StationModel } from './popup.ts'
import { interiorReceiver, positionSamples, type Point, type Probe } from './receiver.ts'
import { latency, renderReport, type GuardPointRow, type StationRow } from './report.ts'

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
    'position-radius-default-m': { type: 'string' },
    'facade-offset-default-m': { type: 'string' },
    'guard-points': { type: 'string' },
  },
})
if (!args.server || !args.catalogue || !args.out || !args['position-radius-default-m'] || !args['facade-offset-default-m']) {
  console.error('usage: run.ts --server URL --catalogue PATH --out NEW_DIR --position-radius-default-m M --facade-offset-default-m M '
    + '[--identity JSON] [--label NAME] [--guard-points JSON]')
  process.exit(2)
}
if (args['receiver-height'] !== 'microphone' && args['receiver-height'] !== 'engine-default') {
  console.error('--receiver-height must be microphone or engine-default')
  process.exit(2)
}
const server = args.server.replace(/\/$/, '')
const concurrency = Number(args.concurrency)
const positionRadiusDefault = Number(args['position-radius-default-m'])
const facadeOffsetDefault = Number(args['facade-offset-default-m'])
const outDir = resolve(args.out)
if (existsSync(outDir)) throw new Error(`${outDir} exists: a run directory is written once`)

const catalogue = loadCatalogue(resolve(args.catalogue))
const guardPoints = args['guard-points']
  ? JSON.parse(readFileSync(resolve(args['guard-points']), 'utf8')) as Array<{ guard: string; label: string; lat: number; lng: number }> : []
const health = await fetch(`${server}/api/health`, { signal: AbortSignal.timeout(5000) })
const instance = health.headers.get(INSTANCE_HEADER)
if (!health.ok || !instance) throw new Error(`${server}: unhealthy or without ${INSTANCE_HEADER}`)

async function get(path: string): Promise<unknown> {
  const response = await fetch(`${server}${path}`, { signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) })
  if (response.headers.get(INSTANCE_HEADER) !== instance) throw new Error('server instance changed')
  if (!response.ok) throw new Error(`HTTP ${response.status} for ${path.split('?')[0]}`)
  return await response.json()
}
const probeAt = (receiverHeightM: number | null): Probe => ({
  popup: point => get(`/api/noise-onfly-v2?lat=${point.lat}&lng=${point.lng}${receiverHeightM == null ? '' : `&receiver_height_m=${receiverHeightM}`}`) as Promise<PopupAnswer>,
  inside: async point => (await get(`/api/building-at?lat=${point.lat}&lng=${point.lng}`)) !== null,
})

async function cohort(): Promise<ModelCohort> {
  const value = await get('/api/validation/cohort') as ModelCohort
  if (value.cohort_unstable) throw new Error('server files changed under the running process; restart it before validating')
  return value
}

function requestedHeight(station: CatalogueStation): number | null {
  if (args['receiver-height'] === 'engine-default' || station.mic_height_m == null) return null
  return Math.max(station.mic_height_m, ENGINE_RECEIVER_HEIGHT_FLOOR_M)
}

const number = (value: unknown) => typeof value === 'number' ? value : null
const text = (value: unknown) => typeof value === 'string' ? value : null

async function scoreStation(station: CatalogueStation): Promise<StationRow> {
  const requested = requestedHeight(station)
  const probe = probeAt(requested)
  const parsed = Object.entries(station.indicators)
    .map(([key, value]) => ({ key, result: parseIndicator(key, value, station.native_periods, stationDefaultLayer(station)) }))
  const base: StationRow = {
    key: station.station_id, set: station.set, station_id: station.station_id, name: station.name, lat: station.lat, lng: station.lng,
    expected_source: station.expected_source, guard_osm: text(station.guard_expected_source_osm), site_class: text(station.site_class), physical_station_id: text(station.physical_station_id),
    instrument_class: text(station.instrument_class) ?? number(station.instrument_class)?.toString() ?? null,
    truth_kind: station.truth_kind, measurand: station.measurand ?? 'sound_level', year: number(station.year),
    months_covered: number(station.months_covered), holdout_square: station.z9_holdout_square === true,
    diagnostic_only: station.diagnostic_only === true, diagnostic_reason: text(station.diagnostic_reason),
    native_periods: station.native_periods, mount: text(station.mount), facade_distance_m: number(station.facade_distance_m),
    publisher_facade_correction_db: number(station.publisher_facade_correction_db),
    publisher_facade_correction_applied: typeof station.publisher_facade_correction_applied === 'boolean' ? station.publisher_facade_correction_applied : null,
    position_uncertainty_m: number(station.position_uncertainty_m), mic_height_m: station.mic_height_m,
    requested_receiver_height_m: requested, receiver: null, receiver_height_used_m: null, height_matches_microphone: false,
    request_ms: 0, model: null, comparisons: [], guard: null, position_samples: null,
    unsupported_indicators: parsed.flatMap(entry => 'unsupported' in entry.result ? [`${entry.key}: ${entry.result.unsupported}`] : []),
    unscored: null, error: null,
  }
  const started = performance.now()
  try {
    let receiver: Point = station
    let basis: NonNullable<StationRow['receiver']>['basis'] = 'station point, outdoors'
    let moved = 0
    // Criteria v2 interior rule: never a building-exposure value; the outline point moved outward, asked there.
    if (await probe.inside(station)) {
      const interior = await interiorReceiver(station, base.facade_distance_m ?? facadeOffsetDefault, probe)
      if ('reason' in interior) return { ...base, unscored: `interior station: ${interior.reason}` }
      receiver = interior.point
      moved = interior.moved_m
      basis = 'interior rule: nearest outline moved outward'
    }
    const popupStarted = performance.now()
    const answer = await probe.popup(receiver)
    base.request_ms = Math.round(performance.now() - popupStarted)
    if (answer.building_exposure) return { ...base, unscored: 'the receiver returned a building exposure' }
    const model: StationModel = readPopupAnswer(answer, receiver)
    // A server without the height parameter ignores it and answers at the engine default.
    const used = model.receiver.height_m ?? ENGINE_DEFAULT_RECEIVER_HEIGHT_M
    if (requested != null && model.receiver.height_m != null && Math.abs(used - requested) > 1e-9) {
      throw new Error(`server computed ${used} m for a requested ${requested} m`)
    }
    const radius = (base.position_uncertainty_m ?? positionRadiusDefault) + moved
    const sampled = base.truth_kind === 'measured' && base.measurand === 'sound_level'
    return {
      ...base, model, receiver_height_used_m: used,
      receiver: { lat: receiver.lat, lng: receiver.lng, basis, moved_m: moved, position_radius_m: radius },
      height_matches_microphone: station.mic_height_m != null && Math.abs(used - station.mic_height_m) < 1e-9,
      comparisons: parsed.flatMap(entry => 'indicator' in entry.result ? [compareIndicator(entry.result.indicator, model)] : []),
      guard: station.guard ? evaluateGuard(station, model) : null,
      position_samples: sampled ? await positionSamples(receiver, radius, probe, (sample, point) => {
        const read = readPopupAnswer(sample, point)
        return { lden: read.total.lden, ln: read.total.periods.night }
      }) : null,
      unscored: model.unavailable_layers.length ? `unavailable layers: ${model.unavailable_layers.join(', ')}` : null,
    }
  } catch (error) {
    return { ...base, request_ms: Math.round(performance.now() - started), error: error instanceof Error ? error.message : String(error) }
  }
}

async function scoreGuardPoint(point: { guard: string; label: string; lat: number; lng: number }): Promise<GuardPointRow> {
  try {
    return { ...point, model: readPopupAnswer(await probeAt(null).popup(point), point), error: null }
  } catch (error) {
    return { ...point, model: null, error: error instanceof Error ? error.message : String(error) }
  }
}

function productIdentity(): Record<string, unknown> {
  const repo = resolve(import.meta.dirname, '../..')
  const git = (...command: string[]) => execFileSync('git', ['-C', repo, ...command], { encoding: 'utf8' }).trim()
  return { runner_commit: git('rev-parse', 'HEAD'), runner_dirty_files: git('status', '--porcelain', '--', 'pipeline/validation').split('\n').filter(Boolean) }
}

async function pool<T, R>(items: T[], work: (item: T) => Promise<R>): Promise<R[]> {
  const results: R[] = new Array(items.length)
  let next = 0
  await Promise.all(Array.from({ length: concurrency }, async () => {
    while (next < items.length) {
      const index = next++
      results[index] = await work(items[index])
      if ((index + 1) % 25 === 0) console.error(`  ${index + 1}/${items.length}`)
    }
  }))
  return results
}

const startedAt = new Date()
const initialCohort = await cohort()
const rows = await pool(catalogue.stations, scoreStation)
const guards = await pool(guardPoints, scoreGuardPoint)
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
  position_radius_default_m: positionRadiusDefault,
  facade_offset_default_m: facadeOffsetDefault,
  concurrency,
  latency: latency(rows),
  catalogue: catalogue.files,
  ...productIdentity(),
  ...(args.identity ? { ops: JSON.parse(readFileSync(resolve(args.identity), 'utf8')) as unknown } : {}),
}
mkdirSync(outDir, { recursive: true })
writeFileSync(resolve(outDir, 'identity.json'), JSON.stringify(identity, null, 2) + '\n', { flag: 'wx' })
writeFileSync(resolve(outDir, 'stations.jsonl'), rows.map(row => JSON.stringify(row)).join('\n') + '\n', { flag: 'wx' })
writeFileSync(resolve(outDir, 'guard-points.jsonl'), guards.map(row => JSON.stringify(row)).join('\n') + '\n', { flag: 'wx' })
writeFileSync(resolve(outDir, 'report.md'), renderReport(rows, identity, runSeconds), { flag: 'wx' })
console.error(`[validation] ${rows.length} stations, ${guards.length} guard points → ${outDir}`)
