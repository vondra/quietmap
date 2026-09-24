/**
 * Validation station catalogue: JSONL rows (one station each) with native published indicators,
 * parsed into comparable indicators with explicit local-hour windows, layer and measured band.
 */
import { createHash } from 'node:crypto'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { END_PERIOD_WINDOWS, type HourWindow, type ModelPeriod } from './lib.ts'

export type MeasuredValue = number | [number | null, number | null] | null
export type CatalogueStation = {
  station_id: string
  set: string
  name: string
  lat: number
  lng: number
  mic_height_m: number | null
  truth_kind: 'measured' | 'official_map_modelled'
  measurand?: 'sound_level' | 'traffic_count'
  expected_source: string
  guard: boolean
  guard_expected_source: string | null
  holdout: boolean
  diagnostic_only?: boolean
  indicators: Record<string, MeasuredValue>
  native_periods: Record<string, unknown> | null
  [field: string]: unknown
}

/** Model layers a comparison can read: one layer, every layer, or every layer but aircraft. */
export type LayerSelector = string | null | 'non_aircraft'
export type TrafficClass = 'total' | 'light' | 'medium' | 'heavy' | 'moto' | 'heavy_share_pct'
type Common = { key: string; value: number | null; band: [number | null, number | null] | null; layer: LayerSelector; periods_assumed_end: boolean }
export type Indicator =
  | Common & { kind: 'weighted'; period: 'lden' | 'cnel'; windows: Record<ModelPeriod, HourWindow>; evening_penalty_db: number }
  | Common & { kind: 'window'; period: string; window: HourWindow }
  | Common & { kind: 'percentile'; period: string }
  | Common & { kind: 'traffic'; period: TrafficClass }

/** Catalogue source vocabulary → popup layer (`LayerKind`). */
export const SOURCE_LAYERS: Record<string, string> = { road: 'road', rail: 'railway', aircraft: 'aircraft', industry: 'industrial' }
const LAYER_SUFFIXES: Record<string, LayerSelector> = {
  road: 'road', rail: 'railway', railway: 'railway', aircraft: 'aircraft', industrial: 'industrial', industry: 'industrial',
  total: null, non_aircraft: 'non_aircraft',
}
const NAMED_PERIODS: Record<string, ModelPeriod> = {
  lday: 'day', ld: 'day', laeq_day: 'day', levening: 'evening', le: 'evening', laeq_evening: 'evening',
  lnight: 'night', ln: 'night', laeq_night: 'night',
}
/** END Lden adds 5 dB in the evening; CNEL weights the evening ×3 (10·lg 3 dB); both add 10 dB at night. */
const EVENING_PENALTY_DB = { lden: 5, cnel: 10 * Math.log10(3) }
const TRAFFIC_CLASSES: Array<[RegExp, TrafficClass]> = [
  [/heavy share/i, 'heavy_share_pct'], [/^light/i, 'light'], [/^medium/i, 'medium'], [/^heavy/i, 'heavy'],
  [/^motorcycle/i, 'moto'], [/aadt|count/i, 'total'],
]

function parseHour(text: string): number {
  const match = /^(\d{1,2})(?::00)?$/.exec(text.trim())
  if (!match || Number(match[1]) > 24) throw new Error(`not a whole local hour: ${JSON.stringify(text)}`)
  return Number(match[1])
}

export function parseWindow(value: unknown): HourWindow {
  if (typeof value === 'string') {
    const parts = value.split(/[-–]/)
    if (parts.length === 2) return { start: parseHour(parts[0]), end: parseHour(parts[1]) }
  }
  throw new Error(`not an hour window: ${JSON.stringify(value)}`)
}

/**
 * The station's day/evening/night windows. With none named, END hours are assumed and flagged;
 * a catalogue naming some periods leaves the others unknown, never silently END.
 */
export function nativePeriodWindows(nativePeriods: Record<string, unknown> | null): {
  windows: Record<ModelPeriod, HourWindow>; named: Set<ModelPeriod>; assumedEnd: boolean
} {
  const windows = { ...END_PERIOD_WINDOWS }
  const named = new Set<ModelPeriod>()
  for (const period of ['day', 'evening', 'night'] as ModelPeriod[]) {
    if (nativePeriods?.[period] != null) {
      windows[period] = parseWindow(nativePeriods[period])
      named.add(period)
    }
  }
  return { windows, named, assumedEnd: named.size === 0 }
}

/**
 * One published value → one comparable indicator, or the reason it has no model counterpart.
 * `defaultLayer` applies to unsuffixed names where the truth itself is source-specific.
 */
export function parseIndicator(
  key: string, raw: MeasuredValue, nativePeriods: Record<string, unknown> | null, defaultLayer: LayerSelector,
): { indicator: Indicator } | { unsupported: string } {
  if (raw == null) return { unsupported: 'no published value' }
  const value = typeof raw === 'number' ? raw : null
  const band = Array.isArray(raw) ? raw : null
  const trafficClass = /\((veh\/day|%)\)/.test(key) ? TRAFFIC_CLASSES.find(([pattern]) => pattern.test(key))?.[1] : undefined
  if (trafficClass) return { indicator: { key, value, band, kind: 'traffic', period: trafficClass, layer: 'road', periods_assumed_end: false } }
  const suffix = Object.keys(LAYER_SUFFIXES).sort((a, b) => b.length - a.length).find(name => key.toLowerCase().endsWith(`_${name}`))
  const layer = suffix === undefined ? defaultLayer : LAYER_SUFFIXES[suffix]
  let base = suffix === undefined ? key : key.slice(0, -suffix.length - 1)
  if (base.toLowerCase().endsWith('_class')) base = base.slice(0, -'_class'.length)
  const lower = base.toLowerCase()
  const common = { key, value, band, layer }
  try {
    if (/^la?\d{1,2}$/i.test(base)) return { indicator: { ...common, layer: null, kind: 'percentile', period: base.toUpperCase(), periods_assumed_end: false } }
    const { windows, named, assumedEnd } = nativePeriodWindows(nativePeriods)
    const unnamed = (periods: ModelPeriod[]) => !assumedEnd && periods.some(period => !named.has(period))
    if (lower === 'lden' || lower === 'cnel') {
      if (unnamed(['day', 'evening', 'night'])) return { unsupported: 'native_periods lacks a day, evening or night window' }
      return { indicator: { ...common, kind: 'weighted', period: lower, windows, evening_penalty_db: EVENING_PENALTY_DB[lower], periods_assumed_end: assumedEnd } }
    }
    if (NAMED_PERIODS[lower]) {
      const period = NAMED_PERIODS[lower]
      if (unnamed([period])) return { unsupported: `native_periods names no ${period} window` }
      return { indicator: { ...common, kind: 'window', period, window: windows[period], periods_assumed_end: assumedEnd } }
    }
    if (lower === 'laeq_24h') return { indicator: { ...common, kind: 'window', period: '24h', window: { start: 0, end: 24 }, periods_assumed_end: false } }
    const hours = /^laeq_(\d{1,2})_(\d{1,2})$/i.exec(base)
    if (hours) {
      const window = { start: parseHour(hours[1]), end: parseHour(hours[2]) }
      return { indicator: { ...common, kind: 'window', period: `${hours[1]}-${hours[2]}`, window, periods_assumed_end: false } }
    }
  } catch (error) {
    return { unsupported: error instanceof Error ? error.message : String(error) }
  }
  return { unsupported: 'no model counterpart for this indicator name' }
}

function requireField(row: Record<string, unknown>, field: string, check: (value: unknown) => boolean, label: string): void {
  if (!check(row[field])) throw new Error(`${label}: invalid ${field} ${JSON.stringify(row[field])}`)
}

export function validateStation(row: unknown, label: string): asserts row is CatalogueStation {
  if (!row || typeof row !== 'object' || Array.isArray(row)) throw new Error(`${label}: expected an object`)
  const station = row as Record<string, unknown>
  const text = (value: unknown) => typeof value === 'string' && value.length > 0 && value === value.trim()
  const measured = (value: unknown) => value == null || Number.isFinite(value)
    || (Array.isArray(value) && value.length === 2 && value.every(edge => edge == null || Number.isFinite(edge)))
  requireField(station, 'station_id', text, label)
  requireField(station, 'set', text, label)
  requireField(station, 'name', text, label)
  requireField(station, 'lat', value => Number.isFinite(value) && Math.abs(value as number) <= 90, label)
  requireField(station, 'lng', value => Number.isFinite(value) && Math.abs(value as number) <= 180, label)
  requireField(station, 'mic_height_m', value => value == null || (Number.isFinite(value) && (value as number) > 0), label)
  requireField(station, 'truth_kind', value => value === 'measured' || value === 'official_map_modelled', label)
  requireField(station, 'expected_source', text, label)
  requireField(station, 'guard', value => typeof value === 'boolean', label)
  requireField(station, 'holdout', value => typeof value === 'boolean', label)
  requireField(station, 'native_periods', value => value == null || (typeof value === 'object' && !Array.isArray(value)), label)
  requireField(station, 'indicators', value => !!value && typeof value === 'object' && Object.keys(value).length > 0, label)
  for (const [key, value] of Object.entries(station.indicators as object)) {
    if (!measured(value)) throw new Error(`${label}: indicator ${key} must be a number, a [low, high] band or null`)
  }
}

/** Every `*.jsonl` under a directory (or one file), sorted, validated, with a content digest per file. */
export function loadCatalogue(path: string): { stations: CatalogueStation[]; files: Array<{ file: string; sha256: string; rows: number }> } {
  const files = statSync(path).isDirectory()
    ? readdirSync(path).filter(name => name.endsWith('.jsonl')).sort().map(name => join(path, name))
    : [path]
  if (files.length === 0) throw new Error(`${path}: no .jsonl catalogue files`)
  const stations: CatalogueStation[] = []
  const identities: Array<{ file: string; sha256: string; rows: number }> = []
  const seen = new Set<string>()
  for (const file of files) {
    const bytes = readFileSync(file)
    const lines = bytes.toString('utf8').split('\n').filter(line => line.trim().length > 0)
    lines.forEach((line, index) => {
      const row = JSON.parse(line) as unknown
      validateStation(row, `${file}:${index + 1}`)
      if (seen.has(row.station_id)) throw new Error(`${file}:${index + 1}: duplicate station ${row.station_id}`)
      seen.add(row.station_id)
      stations.push(row)
    })
    identities.push({ file, sha256: createHash('sha256').update(bytes).digest('hex'), rows: lines.length })
  }
  return { stations, files: identities }
}

/** Unsuffixed indicators of a source-specific truth (a guard or an official source map) read that layer. */
export function stationDefaultLayer(station: CatalogueStation): LayerSelector {
  const sourceSpecific = station.guard || station.truth_kind === 'official_map_modelled'
  return sourceSpecific ? SOURCE_LAYERS[station.expected_source] ?? null : null
}
