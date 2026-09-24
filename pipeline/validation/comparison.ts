/**
 * Score one station: each native indicator against the model on the same local-hour windows and
 * layers (a band scores its distance outside the band), traffic counts against the dominant road,
 * and source-identity guards that fail whatever the total.
 */
import type { CatalogueStation, Indicator, LayerSelector } from './catalogue.ts'
import { SOURCE_LAYERS } from './catalogue.ts'
import { energySumDb, nativeLdenFromModelPeriods, windowLevelFromModelPeriods, type PeriodLevels } from './lib.ts'
import type { StationModel } from './popup.ts'

export type IndicatorComparison = {
  indicator: string
  period: string
  kind: Indicator['kind']
  layer: LayerSelector
  measured: number | null
  band: [number | null, number | null] | null
  /** Local-hour windows the value averages: `06-18/18-22/22-06` for a weighted level. */
  windows: string | null
  model: number | null
  /** model − measured (a band: distance outside it, 0 inside); traffic in 10·lg(model/measured). */
  delta_db: number | null
  unit: 'dB' | 'dB_flow_ratio' | 'percentage_points'
  /** 'exact': native windows are whole END periods. 'piecewise_constant': a window splits an END
   *  period, and the layers read are constant within each period (roads, rail, industry, ships), so
   *  the split is exact too. 'aircraft_split': the split cuts aircraft energy within 10 dB of the
   *  level read, whose timing within the period the model does not keep: diagnostic (criteria v1 §4).
   *  'diagnostic': no model counterpart (L90/L99 beside the model night). */
  period_mapping: 'exact' | 'piecewise_constant' | 'aircraft_split' | 'diagnostic' | 'traffic'
  periods_assumed_end: boolean
}

export type GuardExpectation = { layer: string | null; name: string | null; identity_verified_by_catalogue: boolean }
export type GuardResult = {
  expected: GuardExpectation
  passed: boolean
  expected_layer_share: number
  /** Energy share of the named source among the listed contributors of its layer. */
  identity_share: number | null
  dominant_layer: string | null
  loudest_in_expected_layer: { osm_id: number | null; name: string; distance_m: number; received_lden: number } | null
  reason: string
}

const round2 = (value: number | null): number | null => value == null ? null : Math.round(value * 100) / 100
/** W1 criteria v1 §2: a layer more than 10 dB below the level moves it by at most 0.41 dB. */
const LAYER_RELEVANCE_DB = 10

function selectedLevels(model: StationModel, layer: LayerSelector): { lden: number | null; periods: PeriodLevels } {
  if (layer === null) return model.total
  const layers = layer === 'non_aircraft'
    ? Object.entries(model.layers).filter(([name]) => name !== 'aircraft').map(([, levels]) => levels)
    : model.layers[layer] ? [model.layers[layer]] : []
  const periods = {
    day: energySumDb(layers.map(levels => levels.periods.day)),
    evening: energySumDb(layers.map(levels => levels.periods.evening)),
    night: energySumDb(layers.map(levels => levels.periods.night)),
  }
  return { lden: energySumDb(layers.map(levels => levels.lden)), periods }
}

function distanceOutsideBand(model: number, band: [number | null, number | null]): number {
  if (band[0] != null && model < band[0]) return model - band[0]
  if (band[1] != null && model > band[1]) return model - band[1]
  return 0
}

function compareTraffic(indicator: Indicator & { kind: 'traffic' }, model: StationModel): Pick<IndicatorComparison, 'model' | 'delta_db' | 'unit'> {
  const road = model.dominant_road
  if (!road || indicator.value == null) return { model: null, delta_db: null, unit: indicator.period === 'heavy_share_pct' ? 'percentage_points' : 'dB_flow_ratio' }
  if (indicator.period === 'heavy_share_pct') {
    const share = road.aadt.total > 0 ? 100 * road.aadt.heavy / road.aadt.total : null
    return { model: round2(share), delta_db: share == null ? null : round2(share - indicator.value), unit: 'percentage_points' }
  }
  const flow = road.aadt[indicator.period]
  return { model: flow, delta_db: flow > 0 && indicator.value > 0 ? round2(10 * Math.log10(flow / indicator.value)) : null, unit: 'dB_flow_ratio' }
}

export function compareIndicator(indicator: Indicator, model: StationModel): IndicatorComparison {
  const hours = (window: { start: number; end: number }) => `${String(window.start).padStart(2, '0')}-${String(window.end).padStart(2, '0')}`
  const windows = indicator.kind === 'weighted' ? [indicator.windows.day, indicator.windows.evening, indicator.windows.night].map(hours).join('/')
    : indicator.kind === 'window' ? hours(indicator.window) : null
  const base = {
    indicator: indicator.key, period: indicator.period, kind: indicator.kind, layer: indicator.layer,
    measured: indicator.value, band: indicator.band, windows, periods_assumed_end: indicator.periods_assumed_end,
  }
  if (indicator.kind === 'traffic') return { ...base, ...compareTraffic(indicator, model), period_mapping: 'traffic' }
  if (indicator.kind === 'percentile') {
    // A percentile level has no model counterpart: report the model's night LAeq beside it.
    return { ...base, model: round2(model.total.periods.night), delta_db: null, unit: 'dB', period_mapping: 'diagnostic' }
  }
  const levels = selectedLevels(model, indicator.layer)
  const mapped = indicator.kind === 'weighted'
    ? nativeLdenFromModelPeriods(levels.periods, indicator.windows, indicator.evening_penalty_db)
    : windowLevelFromModelPeriods(levels.periods, indicator.window)
  // On END windows the model's own Lden is exact (the rounded wire periods are not).
  const value = indicator.kind === 'weighted' && indicator.period === 'lden' && mapped.exact && levels.lden != null ? levels.lden : mapped.level
  const delta = value == null ? null
    : indicator.band ? distanceOutsideBand(value, indicator.band)
      : indicator.value == null ? null : value - indicator.value
  const aircraft = indicator.layer === null || indicator.layer === 'aircraft' ? model.layers.aircraft?.lden ?? null : null
  const aircraftRelevant = aircraft != null && levels.lden != null && aircraft > levels.lden - LAYER_RELEVANCE_DB
  const mapping = mapped.exact ? 'exact' : aircraftRelevant ? 'aircraft_split' : 'piecewise_constant'
  return { ...base, model: round2(value), delta_db: round2(delta), unit: 'dB', period_mapping: mapping }
}

/**
 * The catalogue states a guard's source as text: `rail (evidence: …)`, `industry: Mrač quarry
 * (evidence: …)` or `industry (evidence: …) - facility identity (Tata Steel) unverified`.
 */
export function parseGuardExpectation(text: string | null, expectedSource: string): GuardExpectation {
  const source = /^([a-z_]+)/i.exec(text ?? '')?.[1].toLowerCase() ?? expectedSource
  const named = /^[a-z_]+:\s*([^()]+?)\s*\(/i.exec(text ?? '')?.[1] ?? null
  const unverified = /facility identity \(([^)]+)\)/i.exec(text ?? '')?.[1] ?? null
  return { layer: SOURCE_LAYERS[source] ?? null, name: named ?? unverified, identity_verified_by_catalogue: unverified == null }
}

const fold = (text: string): string => text.normalize('NFD').replace(/\p{Diacritic}/gu, '').toLowerCase()

/** A named source matches when any of its words of four or more letters appears in the contributor's name. */
export function nameMatches(expected: string, contributor: string): boolean {
  const words = fold(expected).split(/[^\p{Letter}]+/u).filter(word => word.length >= 4)
  return words.some(word => fold(contributor).includes(word))
}

/** W1 criteria v1 guards (Tata): the named source carries at least half of its layer's energy. */
const IDENTITY_MIN_SHARE_OF_LAYER = 0.5

/**
 * A guard passes only when its expected layer carries the most energy at the receiver and, when
 * the catalogue names the source, that source carries at least half of the layer's energy.
 */
export function evaluateGuard(station: CatalogueStation, model: StationModel): GuardResult {
  const expected = parseGuardExpectation(station.guard_expected_source, station.expected_source)
  const share = expected.layer ? model.layers[expected.layer]?.share_lden ?? 0 : 0
  const loudest = expected.layer ? model.loudest_by_layer[expected.layer] ?? null : null
  const layerLden = expected.layer ? model.layers[expected.layer]?.lden ?? null : null
  const identityShare = expected.name == null || !expected.layer || layerLden == null ? null
    : model.contributors.filter(contributor => contributor.source_type === expected.layer && nameMatches(expected.name!, contributor.name))
      .reduce((sum, contributor) => sum + 10 ** (contributor.received_lden / 10), 0) / 10 ** (layerLden / 10)
  const result = (passed: boolean, reason: string): GuardResult => ({
    expected, passed, expected_layer_share: round2(share)!, identity_share: round2(identityShare), dominant_layer: model.dominant_layer,
    loudest_in_expected_layer: loudest && { osm_id: loudest.osm_id, name: loudest.name, distance_m: loudest.distance_m, received_lden: loudest.received_lden },
    reason,
  })
  if (!expected.layer) return result(false, `no model layer for expected source ${station.expected_source}`)
  if (model.dominant_layer !== expected.layer) return result(false, `dominant layer is ${model.dominant_layer ?? 'none'}`)
  if (expected.name == null) return result(true, 'expected layer dominates')
  if (identityShare! >= IDENTITY_MIN_SHARE_OF_LAYER) return result(true, 'the named source carries most of its layer')
  return result(false, `${expected.name} carries ${round2(identityShare)} of the ${expected.layer} energy; loudest is `
    + `${loudest ? `${loudest.osm_id ?? ''} ${loudest.name}`.trim() : 'unlisted'}`)
}
