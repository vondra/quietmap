//! Strict runtime schema and acoustic invariants for popup parity payloads.

import { validateMetadata, validateProvenance } from './payload-metadata-schema.mjs'
import {
  array,
  boolean,
  coordinatePair,
  exactKeys,
  fail,
  finite,
  integer,
  nullableFinite,
  numericObject,
  object,
  string,
} from './schema-values.mjs'

export const SOURCE_TYPES = Object.freeze(['road', 'railway', 'building', 'industrial', 'aircraft'])
export const SEMANTIC_LAYERS = Object.freeze([
  'road',
  'railway',
  'building',
  'industrial',
  'aircraft_ground',
  'aircraft_airborne',
  'aircraft_cruise',
])

const SOURCE_SET = new Set(SOURCE_TYPES)
const META_PREFIXES = SEMANTIC_LAYERS
const ROOT_KEYS = [
  'elevation_m', 'total_lden', 'total_lden_free', 'sources', 'top_contributors',
  'other_sources_lden', 'compute_time_ms', 'segments_meta', 'timings',
]
const ROOT_OPTIONAL_KEYS = [
  'center', 'h3_center', 'envelope_class', 'envelope_delta_db', 'facade_lden',
  'indoor_lden_tilted', 'segments',
]
const SEGMENT_KEYS = [
  'kind', 'segment_idx', 'name', 'subtype', 'is_dominant_of_group',
  'start_lat', 'start_lon', 'end_lat', 'end_lon', 'cp_lat', 'cp_lon',
  'length_m', 'dist_m', 'd_slant_m', 'bridge', 'tunnel', 'emission',
  'propagation', 'received_lden',
]
const SEGMENT_OPTIONAL_KEYS = [
  'osm_id', 'aircraft_subtype', 'polyline', 'hex_polygon', 'cell_polygon',
  'cruise_buckets', 'cruise_top_flights', 'length_m_per_kind',
]
const LDEN_VARIANT_KEYS = [
  'full', 'free_field', 'no_terrain', 'no_screening', 'no_vegetation',
  'no_ground', 'no_atmospheric',
]
const TIMING_KEYS = [
  'load_ms', 'collect_ms', 'road_ms', 'rail_ms', 'building_ms', 'industrial_ms',
  'aircraft_airborne_ms', 'aircraft_cruise_ms', 'aircraft_ground_ms',
]

function validateSource(value, path) {
  const keys = ['source_type', 'lden', 'lden_free', 'ld', 'le', 'ln', 'segment_count', 'displayed_count']
  exactKeys(value, path, keys)
  if (!SOURCE_SET.has(value.source_type)) fail(`${path}.source_type`, 'unknown source type')
  for (const key of ['lden', 'lden_free', 'ld', 'le', 'ln']) nullableFinite(value[key], `${path}.${key}`)
  integer(value.segment_count, `${path}.segment_count`, 0)
  integer(value.displayed_count, `${path}.displayed_count`, 0)
  if (value.displayed_count > value.segment_count) fail(path, 'displayed_count exceeds segment_count')
}

function validateContributor(value, path) {
  const required = [
    'source_type', 'osm_id', 'name', 'subtype', 'distance_m', 'emission_db',
    'emission_bands', 'baseline', 'terrain', 'screening', 'vegetation',
    'terrain_impact_db', 'screening_impact_db', 'vegetation_impact_db',
    'atmospheric_impact_db', 'ground_impact_db', 'received_lden',
    'received_lden_free', 'received_ln', 'received_bands',
  ]
  exactKeys(value, path, required, ['metadata', 'geometry'])
  if (!SOURCE_SET.has(value.source_type)) fail(`${path}.source_type`, 'unknown source type')
  if (value.osm_id !== null) integer(value.osm_id, `${path}.osm_id`)
  string(value.name, `${path}.name`)
  string(value.subtype, `${path}.subtype`)
  integer(value.distance_m, `${path}.distance_m`, 0)
  for (const key of [
    'emission_db', 'terrain_impact_db', 'screening_impact_db', 'vegetation_impact_db',
    'atmospheric_impact_db', 'ground_impact_db', 'received_lden',
    'received_lden_free', 'received_ln',
  ]) finite(value[key], `${path}.${key}`)
  array(value.emission_bands, `${path}.emission_bands`, 0)
  numericObject(value.baseline, `${path}.baseline`, ['geometric_db', 'ground_factor'])
  exactKeys(value.terrain, `${path}.terrain`, ['delta_m', 'profile_points'])
  finite(value.terrain.delta_m, `${path}.terrain.delta_m`)
  integer(value.terrain.profile_points, `${path}.terrain.profile_points`, 0)
  exactKeys(value.screening, `${path}.screening`, [], ['obstacle'])
  numericObject(value.vegetation, `${path}.vegetation`, ['forest_depth_m', 'sampled_path_m'])
  array(value.received_bands, `${path}.received_bands`, 8).forEach((v, i) => finite(v, `${path}.received_bands[${i}]`))
  if (value.metadata) validateMetadata(value.metadata, value.source_type, `${path}.metadata`)
}

export function semanticLayerForSegment(segment, path = 'segment') {
  if (segment.kind !== 'aircraft') {
    if (!['road', 'railway', 'building', 'industrial'].includes(segment.kind)) {
      fail(`${path}.kind`, 'unknown segment kind')
    }
    if (segment.aircraft_subtype !== undefined && segment.aircraft_subtype !== 0) {
      fail(`${path}.aircraft_subtype`, 'non-aircraft segment has aircraft subtype')
    }
    return segment.kind
  }
  const layer = { 1: 'aircraft_ground', 2: 'aircraft_airborne', 3: 'aircraft_cruise' }[segment.aircraft_subtype]
  if (!layer) fail(`${path}.aircraft_subtype`, 'aircraft subtype must be 1, 2, or 3')
  return layer
}

function validateEmission(value, layer, path) {
  const keys = {
    road: [['kind', 'aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'speed_kmh',
      'surface_corr_db', 'surface', 'traffic_source', 'source_id', 'road_class',
      'bridge', 'tunnel', 'oneway', 'lanes'], ['provenance']],
    railway: [['kind', 'trains_passenger', 'trains_freight', 'trains_passenger_source',
      'trains_freight_source', 'source_id', 'speed_kmh', 'bridge', 'highspeed',
      'rail_type', 'service'], ['provenance']],
    aircraft_ground: [['kind', 'class', 'observed_movements', 'modeled_movements',
      'arrivals_per_day', 'departures_per_day', 'gse_per_day', 'class_mix', 'osm_ref'], []],
    aircraft_airborne: [['kind', 'class', 'callsign', 'aircraft_type', 'cpa_distance_m',
      'altitude_m_at_cpa', 'is_departure', 'icao_hex', 'start_unix'], []],
    aircraft_cruise: [['kind', 'n_unique_flights', 'rep_alt_m'], ['r7_hex', 'square']],
    building: [['kind', 'building_type', 'height_m', 'floors', 'area_m2'], []],
    industrial: [['kind', 'source_type', 'area_m2', 'effective_area_source_dist_m'],
      ['nace', 'hub_height_m', 'rated_power_kw']],
  }[layer]
  exactKeys(value, path, keys[0], keys[1])
  if (value.kind !== layer) fail(`${path}.kind`, `expected ${layer}`)
  if (layer === 'aircraft_cruise') {
    const ids = ['r7_hex', 'square'].filter((key) => Object.hasOwn(value, key))
    if (ids.length !== 1) fail(path, 'expected exactly one cruise cell identifier')
    string(value[ids[0]], `${path}.${ids[0]}`)
  }
  if (value.provenance) validateProvenance(value.provenance, `${path}.provenance`)
}

function validatePropagation(value, layer, path) {
  object(value, path)
  const expected = ['aircraft_airborne', 'aircraft_cruise'].includes(layer) ? 'doc29' : 'cnossos'
  if (value.model !== expected) fail(`${path}.model`, `expected ${expected}`)
  if (expected === 'doc29') {
    const keys = ['model', 'sel_npd_db', 'delta_v_db', 'delta_i_db', 'lambda_db', 'delta_f_db',
      'd_p_m', 'lateral_m', 'beta_deg', 'seg_len_m', 'd_bar_m', 'installation',
      'cffk_fast_path', 'screening_kind', 'screening_db']
    exactKeys(value, path, keys)
    return
  }
  exactKeys(value, path, [
    'model', 'baseline', 'path_profile', 'terrain', 'screening', 'vegetation',
    'ground', 'lw_bands', 'lw_db_a', 'received_bands',
  ])
  exactKeys(value.path_profile, `${path}.path_profile`, [
    't', 'elevation_m', 'forest_u8', 'imd_u8', 'dist_m', 'step_m_med',
    'src_lat', 'src_lon', 'rcv_lat', 'rcv_lon', 'src_alt_m', 'rcv_alt_m',
  ])
  const lengths = ['t', 'elevation_m', 'forest_u8', 'imd_u8'].map((key) => array(value.path_profile[key], `${path}.path_profile.${key}`).length)
  if (!lengths.every((length) => length === lengths[0])) fail(`${path}.path_profile`, 'sample arrays differ in length')
}

function validateSegment(value, path) {
  exactKeys(value, path, SEGMENT_KEYS, SEGMENT_OPTIONAL_KEYS)
  const layer = semanticLayerForSegment(value, path)
  if (value.osm_id !== undefined && value.osm_id !== null) integer(value.osm_id, `${path}.osm_id`)
  integer(value.segment_idx, `${path}.segment_idx`)
  for (const key of ['name', 'subtype']) string(value[key], `${path}.${key}`)
  for (const key of ['is_dominant_of_group', 'bridge', 'tunnel']) boolean(value[key], `${path}.${key}`)
  for (const key of ['start_lat', 'end_lat', 'cp_lat']) {
    finite(value[key], `${path}.${key}`)
    if (value[key] < -90 || value[key] > 90) fail(`${path}.${key}`, 'outside [-90, 90]')
  }
  for (const key of ['start_lon', 'end_lon', 'cp_lon']) {
    finite(value[key], `${path}.${key}`)
    if (value[key] < -180 || value[key] > 180) fail(`${path}.${key}`, 'outside [-180, 180]')
  }
  for (const key of ['length_m', 'dist_m', 'd_slant_m']) finite(value[key], `${path}.${key}`)
  validateEmission(value.emission, layer, `${path}.emission`)
  validatePropagation(value.propagation, layer, `${path}.propagation`)
  numericObject(value.received_lden, `${path}.received_lden`, LDEN_VARIANT_KEYS)
  if (value.polyline) array(value.polyline, `${path}.polyline`).forEach((p, i) => coordinatePair(p, `${path}.polyline[${i}]`))
  const polygons = ['hex_polygon', 'cell_polygon'].filter((key) => Object.hasOwn(value, key))
  if (polygons.length > 1) fail(path, 'both cruise polygon aliases are present')
  if (polygons.length === 1) {
    const polygon = array(value[polygons[0]], `${path}.${polygons[0]}`)
    polygon.forEach((p, i) => coordinatePair(p, `${path}.${polygons[0]}[${i}]`))
    if (polygon.length > 0 && JSON.stringify(polygon[0]) !== JSON.stringify(polygon.at(-1))) {
      fail(`${path}.${polygons[0]}`, 'polygon is not closed')
    }
  }
  return layer
}

function validateMeta(value, segments, path) {
  const fields = ['total_count', 'truncated', ...META_PREFIXES.flatMap((prefix) => [`${prefix}_count`, `${prefix}_total`])]
  exactKeys(value, path, fields)
  integer(value.total_count, `${path}.total_count`, 0)
  boolean(value.truncated, `${path}.truncated`)
  const actual = Object.fromEntries(SEMANTIC_LAYERS.map((layer) => [layer, 0]))
  segments.forEach((layer) => { actual[layer] += 1 })
  let totalSum = 0
  let anyTruncated = false
  for (const prefix of META_PREFIXES) {
    const count = value[`${prefix}_count`]
    const total = value[`${prefix}_total`]
    integer(count, `${path}.${prefix}_count`, 0)
    integer(total, `${path}.${prefix}_total`, 0)
    if (count !== actual[prefix]) fail(path, `${prefix}_count does not match returned segments`)
    if (count > total) fail(path, `${prefix}_count exceeds total`)
    if (count > 150) fail(path, `${prefix}_count exceeds default popup cap 150`)
    totalSum += total
    anyTruncated ||= total > count
  }
  if (value.total_count < segments.length || value.total_count > totalSum) {
    fail(path, 'total_count is outside [returned segments, sum of semantic totals]')
  }
  if (value.truncated !== anyTruncated) fail(path, 'truncated disagrees with per-layer count/total pairs')
}

function assertEnergySum(total, sources, field, path) {
  const sum = sources.reduce((energy, source) => {
    const level = source[field]
    return energy + (level === null ? 0 : 10 ** (level / 10))
  }, 0)
  const actual = total === null ? 0 : 10 ** (total / 10)
  const roundoff = Math.max(1, sum, actual) * Number.EPSILON * 128
  if (Math.abs(actual - sum) > roundoff) fail(path, `not the linear-energy sum of sources.${field}`)
}

export function validatePopupPayload(value, point, label = 'payload') {
  exactKeys(value, label, ROOT_KEYS, ROOT_OPTIONAL_KEYS)
  const centers = ['center', 'h3_center'].filter((key) => Object.hasOwn(value, key))
  if (centers.length !== 1) fail(label, 'expected exactly one center alias')
  coordinatePair(value[centers[0]], `${label}.${centers[0]}`)
  if (point && (value[centers[0]][0] !== point.lat || value[centers[0]][1] !== point.lng)) {
    fail(`${label}.${centers[0]}`, 'does not echo requested coordinates')
  }
  finite(value.elevation_m, `${label}.elevation_m`)
  for (const key of ['total_lden', 'total_lden_free', 'other_sources_lden']) nullableFinite(value[key], `${label}.${key}`)
  finite(value.compute_time_ms, `${label}.compute_time_ms`)
  if (value.compute_time_ms < 0) fail(`${label}.compute_time_ms`, 'must be non-negative')
  array(value.sources, `${label}.sources`).forEach((source, i) => validateSource(source, `${label}.sources[${i}]`))
  const sourceNames = value.sources.map((source) => source.source_type)
  if (new Set(sourceNames).size !== sourceNames.length) fail(`${label}.sources`, 'duplicate source_type')
  array(value.top_contributors, `${label}.top_contributors`)
  if (value.top_contributors.length > 30) fail(`${label}.top_contributors`, 'exceeds display cap 30')
  value.top_contributors.forEach((contributor, i) => validateContributor(contributor, `${label}.top_contributors[${i}]`))
  const segments = value.segments ?? []
  const layers = array(segments, `${label}.segments`).map((segment, i) => validateSegment(segment, `${label}.segments[${i}]`))
  validateMeta(value.segments_meta, layers, `${label}.segments_meta`)
  numericObject(value.timings, `${label}.timings`, TIMING_KEYS)
  // Indoor projection floors each source independently at 0 dB, so only the
  // outdoor wire retains exact linear-energy additivity.
  if (!Object.hasOwn(value, 'envelope_delta_db')) {
    assertEnergySum(value.total_lden, value.sources, 'lden', `${label}.total_lden`)
    assertEnergySum(value.total_lden_free, value.sources, 'lden_free', `${label}.total_lden_free`)
  }
  return value
}

export function payloadCoverage(payload) {
  const sources = new Set(payload.sources.map((source) => source.source_type))
  const layers = new Set()
  for (const layer of SEMANTIC_LAYERS) {
    if (payload.segments_meta[`${layer}_total`] > 0) layers.add(layer)
  }
  return { sources, layers }
}

export function assertCompleteSuiteCoverage(coverages, label) {
  const sources = new Set()
  const layers = new Set()
  for (const coverage of coverages) {
    coverage.sources.forEach((source) => sources.add(source))
    coverage.layers.forEach((layer) => layers.add(layer))
  }
  const missingSources = SOURCE_TYPES.filter((source) => !sources.has(source))
  const missingLayers = SEMANTIC_LAYERS.filter((layer) => !layers.has(layer))
  if (missingSources.length || missingLayers.length) {
    fail(label, `missing coverage: sources=[${missingSources}] layers=[${missingLayers}]`)
  }
  return { sources: [...sources].sort(), layers: [...layers].sort() }
}
