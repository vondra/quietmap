/**
 * Shared plumbing for validation-v2 Leg A snapshot adapters
 * (docs/dev/validation-v2-plan.md §Leg A): per-network agent-run pulls that
 * write (a) raw+normalized SQLite under data/validation/ (gitignored,
 * re-fetchable) and (b) compact per-station annual JSON snapshots committed
 * under benchmarks/validation/snapshots/. No cron until a network has two
 * stable manual runs.
 */
import { DatabaseSync } from 'node:sqlite'
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { ISO2_TO_ISO3 } from '../lib/country-polygon.js'

export const REPO_ROOT = resolve(import.meta.dirname, '../..')
export const VALIDATION_DATA_DIR = resolve(REPO_ROOT, 'data/validation')
export const SNAPSHOT_DIR = resolve(REPO_ROOT, 'benchmarks/validation/snapshots')
export const SQLITE_PATH = resolve(VALIDATION_DATA_DIR, 'validation.sqlite')

/** END periods over LOCAL time: ld 07–19, le 19–23, ln 23–07. */
export type EndPeriod = 'ld' | 'le' | 'ln'
export const PERIOD_MINUTES_PER_DAY: Record<EndPeriod, number> = { ld: 720, le: 240, ln: 480 }

export function endPeriodForLocalHour(hour: number): EndPeriod {
  if (hour >= 7 && hour < 19) return 'ld'
  if (hour >= 19 && hour < 23) return 'le'
  return 'ln'
}

/** Energetic (logarithmic) mean of A-weighted levels: 10·log10(Σ10^(L/10)/n). */
export function energeticMeanDb(energySum: number, n: number): number | null {
  if (n <= 0 || !Number.isFinite(energySum) || energySum <= 0) return null
  return 10 * Math.log10(energySum / n)
}

/**
 * Adapters tolerate at most 0.5% timestamp/window rounding overlap, but the
 * committed schema must never claim more than complete (100%) coverage.
 */
export function boundedCoveragePercent(fraction: number): number {
  if (!Number.isFinite(fraction) || fraction < 0 || fraction > 1.005) {
    throw new Error(`coverage fraction must be finite and in 0..1.005 (got ${fraction})`)
  }
  return Math.min(100, +(fraction * 100).toFixed(1))
}

/** END Lden from period levels (levels may be null when a period is silent/missing). */
export function ldenFromPeriods(ld: number | null, le: number | null, ln: number | null): number | null {
  if (ld == null || le == null || ln == null) return null
  const num = 12 * 10 ** (ld / 10) + 4 * 10 ** ((le + 5) / 10) + 8 * 10 ** ((ln + 10) / 10)
  return 10 * Math.log10(num / 24)
}

export function openValidationDb(): DatabaseSync {
  mkdirSync(VALIDATION_DATA_DIR, { recursive: true })
  const db = new DatabaseSync(SQLITE_PATH)
  db.exec(`
    CREATE TABLE IF NOT EXISTS station (
      network TEXT NOT NULL, station_id TEXT NOT NULL,
      name TEXT, lat REAL, lng REAL, meta_json TEXT,
      PRIMARY KEY (network, station_id)
    );
    -- Raw-but-compact layer: per station × LOCAL calendar day × END period,
    -- the linear-energy sum and the minute count it covers. The 1-minute
    -- source rows are never stored (a Barcelona month alone is 2.1 GB) —
    -- re-fetchable from the portal, this is the smallest faithful reduction.
    CREATE TABLE IF NOT EXISTS daily_period (
      network TEXT NOT NULL, station_id TEXT NOT NULL,
      date TEXT NOT NULL, period TEXT NOT NULL,
      energy_sum REAL NOT NULL, minutes INTEGER NOT NULL,
      PRIMARY KEY (network, station_id, date, period)
    );
    -- Normalized layer: one row per station × year × metric. Metric names:
    -- ld/le/ln/lden (END, computed) or documented window variants as-is
    -- (e.g. laeq_tag_0622 for ZRH Tag) — metric honesty, never fake-converted.
    CREATE TABLE IF NOT EXISTS annual_value (
      network TEXT NOT NULL, station_id TEXT NOT NULL, year INTEGER NOT NULL,
      metric TEXT NOT NULL, value REAL NOT NULL, meta_json TEXT,
      PRIMARY KEY (network, station_id, year, metric)
    );
  `)
  return db
}

export type SnapshotStation = {
  station_id: string
  name: string
  lat: number
  lng: number
  /** Factor-catalog tags specific to this station (snapshot tags are inherited). */
  tags?: string[]
  /** Independent source-regime classification; overrides the snapshot default. */
  regime?: ValidationRegime
  /** Per-station exceptions merged over the network commensurability defaults. */
  commensurability?: Partial<SnapshotCommensurability>
  /** Per-network extras (dominant source tag, district, coverage…). */
  [key: string]: unknown
}

export type SnapshotMode =
  | 'total'
  | 'source:road'
  | 'source:railway'
  | 'source:industrial'
  | 'source:building'
  | 'source:aircraft'

export type AnchorType = 'measurement' | 'official_map' | 'regression'
export type ValidationRegime = 'road' | 'rail' | 'aircraft' | 'industrial_wind' | 'settlement' | 'mixed'
export type ComparisonMode = 'two_sided' | 'upper_bound' | 'trend_only'
export type ModelMetricField = 'lden' | 'ld' | 'le' | 'ln'
export type MeasuredMetricField = ModelMetricField | 'laeq' | 'laeq_24h' | 'laeq_tag_0622' | 'laeq_nacht_2206' | 'laeq_aircraft_day_0622' | 'laeq_aircraft_night_2206'
export type SnapshotCommensurability = {
  metric_variant: string
  dominance: string
  receiver_convention: string
  coord_uncertainty_m?: number
  [key: string]: unknown
}

export type Snapshot = {
  /** Schema for committed Leg-A network snapshots (separate from world-points v2). */
  schema_version: 2
  network: string
  /** ISO 3166-1 alpha-2, used for honest country-stratified reporting. */
  country_code: string
  year: number
  license: string
  source: string[]
  fetched_at: string
  /**
   * Which model quantity the network's values compare against — same axis as
   * the fixture `mode`: 'total' for street/ambient mics, 'source:aircraft'
   * for event-classified airport NMTs, etc.
   */
  mode: SnapshotMode
  anchor_type: AnchorType
  /** Independent default source regime; a station may override it. */
  regime: ValidationRegime
  /** Factor-catalog tags inherited by every station in the snapshot. */
  tags: string[]
  /**
   * Explicit comparison semantics. Never infer these from metric_variant or
   * dominance: a native-but-incommensurable metric may still be trend-only.
   */
  comparison_mode: ComparisonMode
  /** Verdict slack in dB; null exactly when comparison_mode is trend_only. */
  comparison_tolerance_db: number | null
  /** Why this tolerance is defensible; null exactly for trend-only comparisons. */
  comparison_tolerance_basis: string | null
  /** Explicit station payload field and model output field used by Δ tables. */
  measured_metric_field: MeasuredMetricField
  model_metric_field: ModelMetricField
  /** Rule-2 commensurability defaults for every station in this network. */
  commensurability: SnapshotCommensurability
  method: string
  stations: SnapshotStation[]
}

const SNAPSHOT_MODES = new Set<SnapshotMode>([
  'total', 'source:road', 'source:railway', 'source:industrial', 'source:building', 'source:aircraft',
])
const ANCHOR_TYPES = new Set<AnchorType>(['measurement', 'official_map', 'regression'])
const VALIDATION_REGIMES = new Set<ValidationRegime>(['road', 'rail', 'aircraft', 'industrial_wind', 'settlement', 'mixed'])
const COMPARISON_MODES = new Set<ComparisonMode>(['two_sided', 'upper_bound', 'trend_only'])
const MODEL_METRIC_FIELDS = new Set<ModelMetricField>(['lden', 'ld', 'le', 'ln'])
const MEASURED_METRIC_FIELDS = new Set<MeasuredMetricField>([
  'lden', 'ld', 'le', 'ln', 'laeq', 'laeq_24h', 'laeq_tag_0622', 'laeq_nacht_2206', 'laeq_aircraft_day_0622', 'laeq_aircraft_night_2206',
])
const NETWORK_SLUG = /^[a-z0-9]+(?:-[a-z0-9]+)*$/
type FactorVocab = {
  tags: Record<string, unknown>
  metric_variants: Record<string, { band_capable?: boolean }>
  dominance_values: Record<string, unknown>
  receiver_conventions: string[]
}
let cachedFactorVocab: FactorVocab | null = null

function factorVocab(): FactorVocab {
  if (cachedFactorVocab) return cachedFactorVocab
  const path = resolve(REPO_ROOT, 'benchmarks/validation/factor-tags.json')
  const parsed = JSON.parse(readFileSync(path, 'utf8')) as Partial<FactorVocab>
  if (!parsed.tags || !parsed.metric_variants || !parsed.dominance_values || !Array.isArray(parsed.receiver_conventions)) {
    throw new Error(`${path}: incomplete validation vocabulary`)
  }
  return (cachedFactorVocab = parsed as FactorVocab)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === 'object' && !Array.isArray(value)
}

function validateTags(value: unknown, label: string): asserts value is string[] {
  if (!Array.isArray(value)) throw new Error(`${label}: tags must be an array`)
  const seen = new Set<string>()
  for (const tag of value) {
    if (typeof tag !== 'string' || !Object.hasOwn(factorVocab().tags, tag)) throw new Error(`${label}: unknown factor tag ${JSON.stringify(tag)}`)
    if (seen.has(tag)) throw new Error(`${label}: duplicate factor tag ${JSON.stringify(tag)}`)
    seen.add(tag)
  }
}

export function mergedStationCommensurability(snapshot: Snapshot, station: SnapshotStation): SnapshotCommensurability {
  return { ...snapshot.commensurability, ...(station.commensurability ?? {}) }
}

function validateCommensurability(value: unknown, label: string): asserts value is SnapshotCommensurability {
  if (!isRecord(value)) throw new Error(`${label}: commensurability must be an object`)
  const vocab = factorVocab()
  if (typeof value.metric_variant !== 'string' || !Object.hasOwn(vocab.metric_variants, value.metric_variant)) {
    throw new Error(`${label}: unknown commensurability.metric_variant ${JSON.stringify(value.metric_variant)}`)
  }
  if (typeof value.dominance !== 'string' || !Object.hasOwn(vocab.dominance_values, value.dominance)) {
    throw new Error(`${label}: unknown commensurability.dominance ${JSON.stringify(value.dominance)}`)
  }
  if (typeof value.receiver_convention !== 'string' || !vocab.receiver_conventions.includes(value.receiver_convention)) {
    throw new Error(`${label}: unknown commensurability.receiver_convention ${JSON.stringify(value.receiver_convention)}`)
  }
  if (value.coord_uncertainty_m != null && (!Number.isFinite(value.coord_uncertainty_m) || (value.coord_uncertainty_m as number) < 0)) {
    throw new Error(`${label}: coord_uncertainty_m must be finite and non-negative`)
  }
}

function requireFiniteMetric(station: Record<string, unknown>, field: string, label: string): void {
  if (!Number.isFinite(station[field])) throw new Error(`${label}: measured metric ${field} must be finite`)
}

/** Fail-loud runtime validation for downloaded or committed snapshot JSON. */
export function validateSnapshot(value: unknown, label = 'snapshot'): asserts value is Snapshot {
  if (!isRecord(value)) throw new Error(`${label}: expected an object`)
  if (value.schema_version !== 2) throw new Error(`${label}: schema_version must be 2`)
  if (typeof value.network !== 'string' || value.network.length > 128 || !NETWORK_SLUG.test(value.network)) {
    throw new Error(`${label}: network must be a lowercase hyphenated slug`)
  }
  if (typeof value.country_code !== 'string' || !Object.hasOwn(ISO2_TO_ISO3, value.country_code)) {
    throw new Error(`${label}: country_code must be a real ISO alpha-2 code from ISO2_TO_ISO3`)
  }
  if (!Number.isInteger(value.year) || (value.year as number) < 1900 || (value.year as number) > 2100) {
    throw new Error(`${label}: invalid year ${JSON.stringify(value.year)}`)
  }
  if (typeof value.license !== 'string' || value.license.length === 0) throw new Error(`${label}: license must be non-empty`)
  if (!Array.isArray(value.source) || value.source.length === 0 || value.source.some(s => typeof s !== 'string' || s.length === 0)) {
    throw new Error(`${label}: source must be a non-empty string array`)
  }
  if (typeof value.fetched_at !== 'string'
    || !Number.isFinite(Date.parse(value.fetched_at))
    || new Date(value.fetched_at).toISOString() !== value.fetched_at) {
    throw new Error(`${label}: fetched_at must be a canonical UTC ISO instant`)
  }
  if (!SNAPSHOT_MODES.has(value.mode as SnapshotMode)) throw new Error(`${label}: unknown mode ${JSON.stringify(value.mode)}`)
  if (!ANCHOR_TYPES.has(value.anchor_type as AnchorType)) throw new Error(`${label}: unknown anchor_type ${JSON.stringify(value.anchor_type)}`)
  if (!VALIDATION_REGIMES.has(value.regime as ValidationRegime)) throw new Error(`${label}: unknown regime ${JSON.stringify(value.regime)}`)
  if (!COMPARISON_MODES.has(value.comparison_mode as ComparisonMode)) {
    throw new Error(`${label}: unknown comparison_mode ${JSON.stringify(value.comparison_mode)}`)
  }
  if (!MEASURED_METRIC_FIELDS.has(value.measured_metric_field as MeasuredMetricField)) {
    throw new Error(`${label}: unknown measured_metric_field ${JSON.stringify(value.measured_metric_field)}`)
  }
  if (!MODEL_METRIC_FIELDS.has(value.model_metric_field as ModelMetricField)) {
    throw new Error(`${label}: unknown model_metric_field ${JSON.stringify(value.model_metric_field)}`)
  }
  validateTags(value.tags, label)
  validateCommensurability(value.commensurability, label)
  if (value.comparison_mode === 'trend_only') {
    if (value.comparison_tolerance_db !== null) throw new Error(`${label}: trend_only requires comparison_tolerance_db=null`)
    if (value.comparison_tolerance_basis !== null) throw new Error(`${label}: trend_only requires comparison_tolerance_basis=null`)
  } else if (!Number.isFinite(value.comparison_tolerance_db) || (value.comparison_tolerance_db as number) < 0) {
    throw new Error(`${label}: ${value.comparison_mode} requires a non-negative comparison_tolerance_db`)
  } else if (typeof value.comparison_tolerance_basis !== 'string' || value.comparison_tolerance_basis.trim().length === 0) {
    throw new Error(`${label}: ${value.comparison_mode} requires a non-empty comparison_tolerance_basis`)
  }
  if (value.comparison_mode !== 'trend_only' && !factorVocab().metric_variants[value.commensurability.metric_variant]?.band_capable) {
    throw new Error(`${label}: non-trend comparison requires a band-capable metric_variant`)
  }
  if (typeof value.method !== 'string' || value.method.length === 0) throw new Error(`${label}: method must be non-empty`)
  if (!Array.isArray(value.stations) || value.stations.length === 0) throw new Error(`${label}: stations must be non-empty`)

  const stationIds = new Set<string>()
  for (const [index, station] of value.stations.entries()) {
    const stationLabel = `${label}: station[${index}]`
    if (!isRecord(station)) throw new Error(`${stationLabel}: expected an object`)
    if (typeof station.station_id !== 'string'
      || station.station_id.length === 0
      || station.station_id !== station.station_id.trim()
      || station.station_id !== station.station_id.normalize('NFC')
      || station.station_id.includes('/')
      || station.station_id.includes('\0')) {
      throw new Error(`${stationLabel}: station_id must be trimmed NFC text without '/' or NUL`)
    }
    if (stationIds.has(station.station_id)) throw new Error(`${label}: duplicate station_id ${JSON.stringify(station.station_id)}`)
    stationIds.add(station.station_id)
    if (typeof station.name !== 'string' || station.name.length === 0) throw new Error(`${stationLabel}: name must be non-empty`)
    if (!Number.isFinite(station.lat) || (station.lat as number) < -90 || (station.lat as number) > 90) throw new Error(`${stationLabel}: invalid lat`)
    if (!Number.isFinite(station.lng) || (station.lng as number) < -180 || (station.lng as number) > 180) throw new Error(`${stationLabel}: invalid lng`)
    if (station.tags != null) validateTags(station.tags, stationLabel)
    if (station.regime != null && !VALIDATION_REGIMES.has(station.regime as ValidationRegime)) {
      throw new Error(`${stationLabel}: unknown regime ${JSON.stringify(station.regime)}`)
    }
    if (station.commensurability != null && !isRecord(station.commensurability)) {
      throw new Error(`${stationLabel}: commensurability override must be an object`)
    }
    const effective = mergedStationCommensurability(value as unknown as Snapshot, station as SnapshotStation)
    validateCommensurability(effective, stationLabel)
    if (value.comparison_mode !== 'trend_only' && !factorVocab().metric_variants[effective.metric_variant]?.band_capable) {
      throw new Error(`${stationLabel}: non-trend comparison requires an effective band-capable metric_variant`)
    }
    if (value.comparison_mode === 'upper_bound' && effective.dominance !== 'total_ambient') {
      throw new Error(`${stationLabel}: upper_bound requires effective dominance=total_ambient`)
    }
    if (value.comparison_mode === 'two_sided' && effective.dominance === 'total_ambient') {
      throw new Error(`${stationLabel}: total_ambient cannot use a two_sided comparison`)
    }
    requireFiniteMetric(station, value.measured_metric_field as string, stationLabel)
    if (effective.metric_variant === 'period_split') {
      if (!MODEL_METRIC_FIELDS.has(value.measured_metric_field as ModelMetricField)
        || value.measured_metric_field !== value.model_metric_field) {
        throw new Error(`${stationLabel}: period_split requires matching END measured/model metric fields`)
      }
      for (const field of ['ld', 'le', 'ln', 'lden']) requireFiniteMetric(station, field, stationLabel)
      const recomputedLden = ldenFromPeriods(
        station.ld as number,
        station.le as number,
        station.ln as number,
      )
      // Snapshot period values are rounded to 0.1 dB, so recomputation may
      // differ slightly from the pre-rounded annual aggregate.
      if (recomputedLden == null || Math.abs(recomputedLden - (station.lden as number)) > 0.15) {
        throw new Error(`${stationLabel}: lden is inconsistent with the period_split values`)
      }
      if (!Number.isInteger(station.months_covered) || (station.months_covered as number) < 9 || (station.months_covered as number) > 12) {
        throw new Error(`${stationLabel}: period_split requires months_covered in 9..12`)
      }
      if (!Number.isFinite(station.coverage_pct) || (station.coverage_pct as number) < 70 || (station.coverage_pct as number) > 100) {
        throw new Error(`${stationLabel}: period_split requires coverage_pct in 70..100`)
      }
    }
  }
}

export function writeSnapshot(snapshot: Snapshot): string {
  validateSnapshot(snapshot, `snapshot ${snapshot.network}.${snapshot.year}`)
  mkdirSync(SNAPSHOT_DIR, { recursive: true })
  const path = resolve(SNAPSHOT_DIR, `${snapshot.network}.${snapshot.year}.json`)
  const temporary = `${path}.tmp-${process.pid}`
  try {
    writeFileSync(temporary, JSON.stringify(snapshot, null, 2) + '\n', { flag: 'wx' })
    renameSync(temporary, path)
  } finally {
    rmSync(temporary, { force: true })
  }
  return path
}

/** CKAN package_show → resources (Open Data BCN and friends). */
export async function ckanResources(portalBase: string, packageId: string): Promise<Array<{ name: string; url: string; format: string }>> {
  const r = await fetch(`${portalBase}/api/3/action/package_show?id=${packageId}`, { signal: AbortSignal.timeout(30000) })
  if (!r.ok) throw new Error(`CKAN package_show ${packageId}: HTTP ${r.status}`)
  const body = (await r.json()) as { success: boolean; result: { resources: Array<{ name: string; url: string; format: string }> } }
  if (!body.success) throw new Error(`CKAN package_show ${packageId}: success=false`)
  return body.result.resources
}

/** Minimal CSV line splitter for well-behaved portal CSVs (no embedded newlines). */
export function splitCsvLine(line: string): string[] {
  if (!line.includes('"')) return line.split(',')
  const out: string[] = []
  let cur = ''
  let inQ = false
  for (let i = 0; i < line.length; i++) {
    const c = line[i]
    if (inQ) {
      if (c === '"' && line[i + 1] === '"') { cur += '"'; i++ }
      else if (c === '"') inQ = false
      else cur += c
    } else if (c === '"') inQ = true
    else if (c === ',') { out.push(cur); cur = '' }
    else cur += c
  }
  out.push(cur)
  return out
}

export function assertDir(path: string): string {
  mkdirSync(dirname(path), { recursive: true })
  return path
}
