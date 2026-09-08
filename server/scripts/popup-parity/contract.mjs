//! One canonical popup comparison policy shared by capture, compare, and tests.

import { SEMANTIC_LAYERS, semanticLayerForSegment } from './payload-schema.mjs'

const CRUISE_CELL_SENTINEL = '<representation-specific-cruise-cell>'
const SYNTHETIC_AIRPORT_SENTINEL = '<synthetic-airport>'

function sortObject(value) {
  if (Array.isArray(value)) return value.map(sortObject)
  if (value === null || typeof value !== 'object') return value
  return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sortObject(value[key])]))
}

function stableJson(value) {
  return JSON.stringify(sortObject(value))
}

function normalizeAirportIdentity(value) {
  if (value?.metadata?.kind !== 'aircraft') return
  const key = value.metadata.airport_key
  if (typeof key === 'string' && (key.startsWith('auto-') || key.startsWith('strip:'))) {
    value.metadata.airport_key = SYNTHETIC_AIRPORT_SENTINEL
  }
}

function contributorIdentity(value) {
  return stableJson([
    value.source_type,
    value.osm_id,
    value.name,
    value.subtype,
    value.metadata?.kind ?? null,
    value.metadata?.variant ?? null,
  ])
}

function normalizeCruiseSegment(value) {
  value.name = 'Cruise cell'
  delete value.emission.r7_hex
  delete value.emission.square
  value.emission.cell_id = CRUISE_CELL_SENTINEL
  const polygonPresent = Object.hasOwn(value, 'hex_polygon') || Object.hasOwn(value, 'cell_polygon')
  delete value.hex_polygon
  delete value.cell_polygon
  value.cell_polygon_present = polygonPresent
  for (const key of ['start_lat', 'start_lon', 'end_lat', 'end_lon', 'cp_lat', 'cp_lon']) {
    delete value[key]
  }
  value.cruise_buckets?.sort((a, b) =>
    a.class - b.class || a.fl_bin - b.fl_bin || a.period - b.period)
  value.cruise_top_flights?.sort((a, b) =>
    b.lmax_db - a.lmax_db || a.date.localeCompare(b.date) ||
    a.time_utc.localeCompare(b.time_utc) || a.icao_hex.localeCompare(b.icao_hex))
}

function segmentIdentity(value, layer) {
  if (layer === 'aircraft_cruise') {
    return stableJson([
      -value.received_lden.full,
      value.emission.n_unique_flights,
      value.emission.rep_alt_m,
    ])
  }
  if (layer === 'aircraft_airborne') {
    return stableJson([
      value.emission.icao_hex,
      value.emission.start_unix,
      value.segment_idx,
      value.emission.callsign,
    ])
  }
  return stableJson([value.osm_id ?? null, value.name, value.subtype, value.segment_idx])
}

/**
 * Runtime aliases and representation-only cruise geometry live here and only
 * here. Numbers are never rounded and no acoustic tolerance is embedded.
 */
export function canonicalizePopup(payload) {
  const result = structuredClone(payload)
  result.center = result.center ?? result.h3_center
  delete result.h3_center
  delete result.compute_time_ms
  delete result.timings

  result.sources = Object.fromEntries(
    result.sources
      .sort((a, b) => a.source_type.localeCompare(b.source_type))
      .map((source) => [source.source_type, source]),
  )
  result.top_contributors.forEach(normalizeAirportIdentity)
  result.top_contributors.sort((a, b) => contributorIdentity(a).localeCompare(contributorIdentity(b)))

  const byLayer = Object.fromEntries(SEMANTIC_LAYERS.map((layer) => [layer, []]))
  for (const segment of result.segments ?? []) {
    const layer = semanticLayerForSegment(segment)
    if (layer === 'aircraft_cruise') normalizeCruiseSegment(segment)
    byLayer[layer].push(segment)
  }
  for (const [layer, segments] of Object.entries(byLayer)) {
    segments.sort((a, b) => segmentIdentity(a, layer).localeCompare(segmentIdentity(b, layer)))
  }
  result.segments = byLayer
  return result
}

export function performanceSnapshot(payload, elapsedMs) {
  return {
    request_elapsed_ms: elapsedMs,
    compute_time_ms: payload.compute_time_ms,
    ...payload.timings,
  }
}

function comparisonType(value) {
  if (value === null) return 'null'
  if (Array.isArray(value)) return 'array'
  return typeof value
}

function normalizedNumericPath(path) {
  return path.replaceAll(/\[\d+\]/g, '[]')
}

function compareValue(reference, candidate, path, state) {
  const referenceType = comparisonType(reference)
  const candidateType = comparisonType(candidate)
  if (referenceType !== candidateType) {
    state.structural.push({ path, reference: referenceType, candidate: candidateType })
    return
  }
  if (referenceType === 'number') {
    const key = normalizedNumericPath(path)
    const values = state.numeric.get(key) ?? []
    values.push({ reference, candidate, delta: candidate - reference })
    state.numeric.set(key, values)
    return
  }
  if (referenceType === 'array') {
    if (reference.length !== candidate.length) {
      state.structural.push({ path: `${path}.length`, reference: reference.length, candidate: candidate.length })
    }
    for (let index = 0; index < Math.min(reference.length, candidate.length); index += 1) {
      compareValue(reference[index], candidate[index], `${path}[${index}]`, state)
    }
    return
  }
  if (referenceType === 'object') {
    const keys = [...new Set([...Object.keys(reference), ...Object.keys(candidate)])].sort()
    for (const key of keys) {
      const hasReference = Object.hasOwn(reference, key)
      const hasCandidate = Object.hasOwn(candidate, key)
      if (!hasReference || !hasCandidate) {
        state.structural.push({
          path: `${path}.${key}`,
          reference: hasReference ? 'present' : 'missing',
          candidate: hasCandidate ? 'present' : 'missing',
        })
      } else {
        compareValue(reference[key], candidate[key], `${path}.${key}`, state)
      }
    }
    return
  }
  if (reference !== candidate) state.categorical.push({ path, reference, candidate })
}

function percentile(sorted, fraction) {
  return sorted[Math.floor((sorted.length - 1) * fraction)]
}

function summarizeNumeric(values) {
  const deltas = values.map((value) => value.delta)
  const absolute = deltas.map(Math.abs).sort((a, b) => a - b)
  const reference = values.map((value) => value.reference)
  const candidate = values.map((value) => value.candidate)
  return {
    count: values.length,
    exact_equal_count: deltas.filter((delta) => delta === 0).length,
    reference_min: Math.min(...reference),
    reference_max: Math.max(...reference),
    candidate_min: Math.min(...candidate),
    candidate_max: Math.max(...candidate),
    signed_delta_min: Math.min(...deltas),
    signed_delta_mean: deltas.reduce((sum, delta) => sum + delta, 0) / deltas.length,
    signed_delta_max: Math.max(...deltas),
    absolute_delta_p50: percentile(absolute, 0.50),
    absolute_delta_p95: percentile(absolute, 0.95),
    absolute_delta_max: absolute.at(-1),
  }
}

export function createComparisonAccumulator() {
  return { structural: [], categorical: [], numeric: new Map() }
}

export function compareCanonical(reference, candidate, pointId, state = createComparisonAccumulator()) {
  compareValue(reference, candidate, `points[${JSON.stringify(String(pointId))}]`, state)
  return state
}

export function comparisonReport(state) {
  return {
    policy: {
      acoustic_tolerances: null,
      numeric_verdict: 'diagnostic_only',
      ignored_runtime_fields: ['compute_time_ms', 'timings'],
      cruise_spatial_identity: 'validated_but_not_compared_literally',
      synthetic_airport_identity: 'auto- and strip: airport_key identifiers are normalized; names and quantities remain compared',
    },
    structural_differences: state.structural,
    categorical_differences: state.categorical,
    numeric: Object.fromEntries(
      [...state.numeric.entries()].sort(([a], [b]) => a.localeCompare(b))
        .map(([path, values]) => [path, summarizeNumeric(values)]),
    ),
  }
}
