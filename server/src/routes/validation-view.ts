/** Thin read-only join for the React validation map (`/#val=1`). */
import type { FastifyInstance } from 'fastify'
import { createHash } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import {
  currentValidationCohort,
  type ValidationCohort,
  type ValidationCohortProvider,
} from '../validation-cohort.js'

const DEFAULT_ROOT = resolve(import.meta.dirname, '../../..')
const SNAPSHOT_FILE = /^([a-z0-9]+(?:-[a-z0-9]+)*)\.(\d{4})\.json$/
const MODE_REGIME: Record<string, string> = {
  'source:road': 'road', 'source:railway': 'rail', 'source:aircraft': 'aircraft',
  'source:industrial': 'industrial_wind', 'source:building': 'settlement',
}
const WORLD_STATUSES = new Set(['OK', 'EXTERNAL-GAP', 'KNOWN-GAP', 'PENDING', 'SKIPPED', 'DRIFT', 'ERROR'])
const EXTERNAL_SIDES = new Set(['within', 'above', 'below', 'below-unattributable'])
const QUERY_STATUSES = new Set(['ok', 'error', 'no_coverage'])
const MODEL_FIELDS = ['lden', 'ld', 'le', 'ln'] as const
type Obj = Record<string, unknown>
type Meta = {
  generated_at: string | null
  server: string | null
  model_cohort: string | null
  runner_commit: string | null
  runner_dirty: boolean | null
  requested_data_year: number | null
}

const object = (value: unknown): value is Obj => value != null && typeof value === 'object' && !Array.isArray(value)
const finite = (value: unknown): number | null => Number.isFinite(value) ? value as number : null
const text = (value: unknown): string | null => typeof value === 'string' && value.length > 0 ? value : null

function readJson(path: string, label: string, warnings: string[]): unknown | null {
  try { return JSON.parse(readFileSync(path, 'utf8')) as unknown }
  catch (error) {
    warnings.push(`${label} unreadable — ${error instanceof Error ? error.message : String(error)}`)
    return null
  }
}

function fileSha256(path: string): string | null {
  try { return createHash('sha256').update(readFileSync(path)).digest('hex') }
  catch { return null }
}

function metadata(value: Obj, time: 'timestamp' | 'generated_at'): Meta {
  return {
    generated_at: text(value[time]), server: text(value.server),
    model_cohort: text(value.model_cohort),
    runner_commit: text(value.runner_commit),
    runner_dirty: typeof value.runner_dirty === 'boolean' ? value.runner_dirty : null,
    requested_data_year: finite(value.requested_data_year),
  }
}

function comparison(
  mode: unknown, tolerance: unknown, measured: number, model: number,
): { delta: number | null; verdict: string | null } {
  if (mode === 'trend_only') return { delta: null, verdict: 'trend_only' }
  if ((mode !== 'two_sided' && mode !== 'upper_bound') || !Number.isFinite(tolerance) || (tolerance as number) < 0) {
    return { delta: null, verdict: null }
  }
  const delta = Math.round((model - measured) * 1e12) / 1e12
  if (delta > (tolerance as number)) return { delta, verdict: 'above' }
  if (delta >= -(tolerance as number)) return { delta, verdict: 'within_bound' }
  return { delta, verdict: mode === 'upper_bound' ? 'unattributable' : 'below' }
}

function catalogRows(
  value: unknown, key: string, label: string, warnings: string[],
  invalid: (row: Obj) => string | null,
): Obj[] {
  if (!Array.isArray(value)) { if (value !== null) warnings.push(`${label} must be an array`); return [] }
  const seen = new Set<string>()
  return value.filter((row, index): row is Obj => {
    const id = object(row) ? text(row[key]) : null
    const error = id && object(row) ? invalid(row) : `missing ${key}`
    if (!id || error) { warnings.push(`${label}[${index}] ${error} — omitted`); return false }
    if (seen.has(id)) { warnings.push(`${label} duplicates ${id} — later copy omitted`); return false }
    seen.add(id)
    return true
  })
}

function artifactRows(
  rows: unknown, key: string, known: Set<string>, label: string, warnings: string[],
  normalize: (row: Obj) => Obj | null,
): Map<string, Obj> {
  const result = new Map<string, Obj>()
  if (!Array.isArray(rows)) { warnings.push(`${label} has no rows array — ignored`); return result }
  for (const raw of rows) {
    const id = object(raw) ? text(raw[key]) : null
    if (!id || !known.has(id)) continue
    if (result.has(id)) { warnings.push(`${label} duplicates ${id} — later row ignored`); continue }
    const row = normalize(raw as Obj)
    if (row) result.set(id, row); else warnings.push(`${label}/${id} has invalid data — ignored`)
  }
  return result
}

function modelValues(value: unknown): Record<string, number | null> | null {
  if (!object(value) || !MODEL_FIELDS.every(field => Object.hasOwn(value, field)
    && (value[field] === null || Number.isFinite(value[field])))) return null
  return Object.fromEntries(MODEL_FIELDS.map(field => [field, value[field] as number | null]))
}

function worldResult(row: Obj): Obj | null {
  if (!(row.value === null || Number.isFinite(row.value))
    || typeof row.status !== 'string' || !WORLD_STATUSES.has(row.status)
    || !(row.drift === null || Number.isFinite(row.drift))) return null
  if (row.ext === null) return { value: row.value, status: row.status, drift: row.drift, ext: null }
  if (!object(row.ext) || !Number.isFinite(row.ext.delta)
    || typeof row.ext.side !== 'string' || !EXTERNAL_SIDES.has(row.ext.side)) return null
  return {
    value: row.value, status: row.status, drift: row.drift,
    ext: { delta: row.ext.delta, side: row.ext.side },
  }
}

function deltaResult(row: Obj, modelField: string): Obj | null {
  const model = modelValues(row.model)
  const queryStatus = typeof row.query_status === 'string' && QUERY_STATUSES.has(row.query_status)
    ? row.query_status : null
  if (!model || !queryStatus || (queryStatus === 'ok') !== (model[modelField] != null)) return null
  return { model, query_status: queryStatus, dominant_source: text(row.dominant_source) }
}

export async function validationViewRoutes(app: FastifyInstance, options: {
  repoRoot?: string
  cohortProvider?: ValidationCohortProvider
} = {}): Promise<void> {
  const root = options.repoRoot ?? DEFAULT_ROOT
  const cohortProvider = options.cohortProvider ?? currentValidationCohort

  app.get('/api/validation/cohort', async (_request, reply) => {
    reply.header('Cache-Control', 'no-store')
    try {
      return reply.send(await cohortProvider())
    } catch (error) {
      app.log.error(error, 'validation cohort fingerprint failed')
      return reply.code(503).send({ error: 'validation cohort unavailable' })
    }
  })

  app.get('/api/validation/points', async (_request, reply) => {
    const warnings: string[] = []
    let currentCohort: ValidationCohort | null = null
    try {
      currentCohort = await cohortProvider()
    } catch (error) {
      warnings.push(`model cohort unavailable — model results hidden; ${error instanceof Error ? error.message : String(error)}`)
    }
    const pointPath = resolve(root, 'benchmarks/world-points.json')
    const points = catalogRows(readJson(pointPath, 'world fixture catalog', warnings), 'id', 'world fixture', warnings,
      row => Number.isFinite(row.lat) && Number.isFinite(row.lng) ? null : 'has non-finite coordinates')
    const pointIds = new Set(points.map(point => point.id as string))
    const fixturesSha256 = fileSha256(pointPath)

    const runPath = resolve(root, 'data/validation/world-lastrun.json')
    let run: Obj | null = null
    if (existsSync(runPath)) {
      const parsed = readJson(runPath, 'world model run', warnings)
      if (!object(parsed)) {
        if (parsed !== null) warnings.push('world model run is not an object — ignored')
      } else if (parsed.schema_version !== 2) {
        warnings.push('world model run has unsupported schema_version — ignored; rerun /check-world')
      } else if (!fixturesSha256 || text(parsed.fixtures_sha256) !== fixturesSha256) {
        warnings.push('world model run belongs to different fixture content — ignored; rerun /check-world')
      } else if (!currentCohort || text(parsed.model_cohort) !== currentCohort.cohort_id) {
        warnings.push('world model run belongs to a different model/data cohort — ignored; rerun /check-world')
      } else run = parsed
    } else warnings.push('no world model run — run /check-world; fixtures show without model values')
    const runById = artifactRows(run ? run.results : [], 'id', pointIds, 'world model run', warnings, worldResult)
    if (run && (!Array.isArray(run.results) || run.results.length !== pointIds.size || runById.size !== pointIds.size)) {
      warnings.push(`world model run covers ${runById.size}/${pointIds.size} fixtures — ignored as incomplete`)
      run = null
      runById.clear()
    }
    const fixtures = points.map(point => {
      const result = runById.get(point.id as string)
      return {
        kind: 'fixture', id: point.id, lat: point.lat, lng: point.lng,
        regime: point.mode === 'total' ? point.regime : MODE_REGIME[point.mode as string],
        mode: point.mode, metric_field: point.metric_field ?? 'lden', anchor_type: point.anchor_type,
        role: point.role, tags: point.tags, pair_id: point.pair_id ?? null,
        external: point.external, commensurability: point.commensurability,
        regression_band: point.regression_band ?? null, known_gap: point.known_gap ?? null,
        tolerance_note: point.tolerance_note, caveats: point.caveats ?? null,
        model_value: finite(result?.value), status: text(result?.status), drift: finite(result?.drift),
        ext: object(result?.ext) ? result.ext : null,
      }
    })

    const manifestPath = resolve(root, 'benchmarks/validation/approved-snapshots.v1.json')
    const manifest = readJson(manifestPath, 'approved snapshot catalog', warnings)
    const rawFiles = object(manifest) && Array.isArray(manifest.files) ? manifest.files : []
    if (!object(manifest) || !Array.isArray(manifest.files)) warnings.push('approved snapshot catalog has no files array')
    const seenFiles = new Set<string>()
    const files = rawFiles.filter((file): file is string => {
      if (typeof file !== 'string' || !SNAPSHOT_FILE.test(file) || seenFiles.has(file)) {
        warnings.push(`approved snapshot filename ${JSON.stringify(file)} is unsafe or duplicated — omitted`); return false
      }
      seenFiles.add(file); return true
    })

    const networks: Obj[] = []
    for (const file of files) {
      const match = SNAPSHOT_FILE.exec(file)!
      const snapshotPath = resolve(root, 'benchmarks/validation/snapshots', file)
      const snapshot = readJson(snapshotPath, file, warnings)
      const snapshotSha256 = fileSha256(snapshotPath)
      if (!object(snapshot) || snapshot.network !== match[1] || snapshot.year !== Number(match[2])
        || !Array.isArray(snapshot.stations) || !text(snapshot.measured_metric_field) || !text(snapshot.model_metric_field)) {
        if (snapshot !== null) warnings.push(`${file} has invalid catalog identity — omitted`)
        continue
      }
      const measuredField = snapshot.measured_metric_field as string
      const modelField = snapshot.model_metric_field as string
      const stations = catalogRows(snapshot.stations, 'station_id', `${snapshot.network} station`, warnings, row => {
        if (!Number.isFinite(row.lat) || !Number.isFinite(row.lng)) return 'has non-finite coordinates'
        return Number.isFinite(row[measuredField]) ? null : `has non-finite ${measuredField}`
      })
      const stationIds = new Set(stations.map(station => station.station_id as string))
      const deltaPath = resolve(root, 'data/validation/deltas', file)
      let delta: Obj | null = null
      if (existsSync(deltaPath)) {
        const parsed = readJson(deltaPath, `${snapshot.network}/${snapshot.year} model delta`, warnings)
        if (!object(parsed)) {
          if (parsed !== null) warnings.push(`${snapshot.network}/${snapshot.year} model delta is not an object — ignored`)
        } else if (parsed.schema_version !== 2) {
          warnings.push(`${snapshot.network}/${snapshot.year} model delta has unsupported schema_version — ignored; regenerate it`)
        } else if (parsed.network !== snapshot.network || parsed.year !== snapshot.year) {
          warnings.push(`${snapshot.network}/${snapshot.year} model delta has a different catalog identity — ignored; regenerate it`)
        } else if (!snapshotSha256 || text(parsed.snapshot_sha256) !== snapshotSha256) {
          warnings.push(`${snapshot.network}/${snapshot.year} model delta belongs to different snapshot content — ignored; regenerate it`)
        } else if (!currentCohort || text(parsed.model_cohort) !== currentCohort.cohort_id) {
          warnings.push(`${snapshot.network}/${snapshot.year} model delta belongs to a different model/data cohort — ignored; regenerate it`)
        } else delta = parsed
      } else warnings.push(`${snapshot.network}/${snapshot.year}: no model delta artifact; measurements remain visible`)
      const deltaById = artifactRows(
        delta ? delta.rows : [], 'station_id', stationIds, `${snapshot.network} delta`, warnings,
        row => deltaResult(row, modelField),
      )
      if (delta && (!Array.isArray(delta.rows) || delta.rows.length !== stationIds.size || deltaById.size !== stationIds.size)) {
        warnings.push(`${snapshot.network}/${snapshot.year} model delta covers ${deltaById.size}/${stationIds.size} stations — ignored as incomplete`)
        delta = null
        deltaById.clear()
      }
      networks.push({
        schema_version: snapshot.schema_version, network: snapshot.network, country_code: snapshot.country_code,
        year: snapshot.year, fetched_at: snapshot.fetched_at, mode: snapshot.mode,
        anchor_type: snapshot.anchor_type, regime: snapshot.regime, tags: snapshot.tags,
        license: snapshot.license, source: snapshot.source, method: snapshot.method,
        commensurability: snapshot.commensurability, comparison_mode: snapshot.comparison_mode,
        comparison_tolerance_db: snapshot.comparison_tolerance_db,
        comparison_tolerance_basis: snapshot.comparison_tolerance_basis,
        measured_metric_field: measuredField, model_metric_field: modelField,
        delta_meta: delta ? metadata(delta, 'generated_at') : null,
        stations: stations.map(station => {
          const result = deltaById.get(station.station_id as string)
          const model = modelValues(result?.model)
          const modelValue = finite(model?.[modelField])
          const queryStatus = text(result?.query_status)
          const compared = modelValue == null
            ? { delta: null, verdict: queryStatus === 'error' || queryStatus === 'no_coverage' ? queryStatus : null }
            : comparison(snapshot.comparison_mode, snapshot.comparison_tolerance_db, station[measuredField] as number, modelValue)
          return {
            ...station, kind: 'station', network: snapshot.network, model,
            measured_metric_field: measuredField, model_metric_field: modelField,
            measured_value: station[measuredField], model_value: modelValue,
            delta_db: compared.delta,
            verdict: compared.verdict, dominant_source: result?.dominant_source ?? null,
          }
        }),
      })
    }
    return reply.send({
      model_cohort: currentCohort,
      lastrun: run ? metadata(run, 'timestamp') : null,
      warnings,
      fixtures,
      networks,
    })
  })
  app.get('/validation', async (_request, reply) => reply.redirect('/#val=1'))
}
