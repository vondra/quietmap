//! Variant-specific contributor metadata assertions for popup payloads.

import {
  array,
  boolean,
  exactKeys,
  fail,
  finite,
  integer,
  nullableFinite,
  object,
  string,
} from './schema-values.mjs'

export const ROAD_TRAFFIC_KEYS = ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'traffic_estimated']

export function validateRoadTraffic(value, path) {
  for (const key of ROAD_TRAFFIC_KEYS.slice(0, 4)) {
    finite(value[key], `${path}.${key}`)
    if (value[key] < 0) fail(`${path}.${key}`, 'negative traffic')
  }
  if (Object.hasOwn(value, 'time_profile_attribution')) {
    const at = `${path}.time_profile_attribution`
    const timing = value.time_profile_attribution
    exactKeys(timing, at, ['source', 'window', 'total_transfer'])
    string(timing.source, `${at}.source`)
    string(timing.window, `${at}.window`)
    boolean(timing.total_transfer, `${at}.total_transfer`)
  }
  integer(value.traffic_estimated, `${path}.traffic_estimated`, 0)
  if (value.traffic_estimated > 15) fail(`${path}.traffic_estimated`, 'invalid category bitmask')
}

export function validateProvenance(value, path) {
  if (value === null) return
  exactKeys(value, path, ['name', 'year', 'license', 'url', 'tier'])
  string(value.name, `${path}.name`)
  if (value.year !== null) integer(value.year, `${path}.year`, 0)
  for (const key of ['license', 'url']) {
    if (value[key] !== null) string(value[key], `${path}.${key}`)
  }
  string(value.tier, `${path}.tier`)
}

function validatePeriods(value, path) {
  const keys = ['ld_db', 'le_db', 'ln_db', 'lden_db']
  exactKeys(value, path, keys)
  for (const key of keys) nullableFinite(value[key], `${path}.${key}`)
}

function validateAircraftMetadata(value, path) {
  exactKeys(value, path, ['kind', 'variant'], ['airport_name', 'airport_key', 'airborne', 'ground_ops'])
  if (value.kind !== 'aircraft') fail(`${path}.kind`, 'expected aircraft')
  if (!['airborne', 'ground_ops'].includes(value.variant)) fail(`${path}.variant`, 'unknown variant')
  for (const key of ['airport_name', 'airport_key']) {
    if (Object.hasOwn(value, key)) string(value[key], `${path}.${key}`)
  }
  if (value.airborne) {
    const p = `${path}.airborne`
    exactKeys(value.airborne, p, [
      'periods', 'observed_flights_per_day', 'helicopter_flights_per_day',
      'cruise_transits_per_day', 'lmax_peak', 'faint', 'audible', 'disruptive',
      'top_day_energy_share', 'top_day_date', 'top_flight_energy_share',
      'sample_days', 'ga_sample_days',
    ], ['top_flights'])
    validatePeriods(value.airborne.periods, `${p}.periods`)
    for (const key of ['faint', 'audible', 'disruptive']) {
      exactKeys(value.airborne[key], `${p}.${key}`, ['observed_events_per_day', 'avg_altitude_m', 'top_aircraft'])
      finite(value.airborne[key].observed_events_per_day, `${p}.${key}.observed_events_per_day`)
      finite(value.airborne[key].avg_altitude_m, `${p}.${key}.avg_altitude_m`)
      string(value.airborne[key].top_aircraft, `${p}.${key}.top_aircraft`)
    }
    if (value.airborne.lmax_peak !== null) finite(value.airborne.lmax_peak, `${p}.lmax_peak`)
    if (value.airborne.top_flights) array(value.airborne.top_flights, `${p}.top_flights`)
  }
  if (value.ground_ops) {
    const p = `${path}.ground_ops`
    exactKeys(value.ground_ops, p, [
      'periods', 'periods_free', 'observed_movements_per_day', 'modeled_movements_per_day',
      'distance_m', 'emission_db', 'received_bands', 'runway_roll', 'taxi',
      'apron_movement', 'baseline', 'terrain', 'screening', 'vegetation',
      'terrain_impact_db', 'screening_impact_db', 'vegetation_impact_db',
      'atmospheric_impact_db', 'ground_impact_db', 'arrivals_per_day',
      'departures_per_day', 'gse_per_day',
    ], ['profile_mix'])
    validatePeriods(value.ground_ops.periods, `${p}.periods`)
    validatePeriods(value.ground_ops.periods_free, `${p}.periods_free`)
    array(value.ground_ops.received_bands, `${p}.received_bands`, 8)
    array(value.ground_ops.gse_per_day, `${p}.gse_per_day`, 3)
  }
}

export function validateMetadata(value, sourceType, path) {
  object(value, path)
  if (value.kind === 'aircraft') return validateAircraftMetadata(value, path)
  const definitions = {
    road: [
      ['kind', ...ROAD_TRAFFIC_KEYS, 'cross_section_aadt', 'dominant_source_id', 'speed_posted_kmh', 'speed_kmh', 'speed_source', 'road_class', 'surface',
        'surface_corr_db', 'lanes', 'oneway', 'dominant_segment_idx',
        'dominant_distance_m', 'closest_distance_m', 'speed_min_kmh', 'speed_max_kmh',
        'oneway_segment_count', 'twoway_segment_count', 'segment_count', 'total_length_m',
        'bridge_count', 'obstacle_segment_count', 'obstacle_avg_height_m',
        'obstacle_max_height_m', 'obstacle_max_segment_idx'], ['provenance', 'time_profile_attribution', 'profiled_segment_count']],
    rail: [
      ['kind', 'traffic', 'passenger_provenance', 'freight_provenance', 'maxspeed_posted_kmh',
        'speed_kmh', 'speed_source', 'rail_type', 'usage', 'service', 'highspeed',
        'bridge', 'dominant_segment_idx', 'dominant_distance_m', 'closest_distance_m',
        'segment_count', 'total_length_m', 'obstacle_segment_count',
        'obstacle_avg_height_m', 'obstacle_max_height_m', 'obstacle_max_segment_idx'], []],
    building: [['kind', 'height_m', 'floors', 'area_m2', 'building_type', 'address'], []],
    industrial: [['kind', 'area_m2', 'source_type', 'nace', 'grid_point_count', 'source_id', 'provenance'], []],
  }
  const expectedKind = sourceType === 'railway' ? 'rail' : sourceType
  const definition = definitions[expectedKind]
  if (!definition || value.kind !== expectedKind) fail(`${path}.kind`, `expected ${expectedKind}`)
  exactKeys(value, path, definition[0], definition[1])
  if (expectedKind === 'road') {
    validateRoadTraffic(value, path)
    finite(value.cross_section_aadt, `${path}.cross_section_aadt`)
    if (value.cross_section_aadt < 0) fail(`${path}.cross_section_aadt`, 'negative traffic')
    if (Object.hasOwn(value, 'profiled_segment_count')) {
      integer(value.profiled_segment_count, `${path}.profiled_segment_count`, 0)
      if (value.profiled_segment_count > value.segment_count) fail(path, 'profiled_segment_count exceeds segment_count')
    }
  }
  if (expectedKind === 'rail') validateRailTraffic(value, path)
  if (Object.hasOwn(value, 'provenance')) validateProvenance(value.provenance, `${path}.provenance`)
}

export function validateRailTraffic(value, path) {
  exactKeys(value.traffic, `${path}.traffic`, ['passenger', 'freight'])
  for (const category of ['passenger', 'freight']) {
    const traffic = value.traffic[category]
    const at = `${path}.traffic.${category}`
    exactKeys(traffic, at, ['periods', 'status', 'source_id', 'matching'])
    array(traffic.periods, `${at}.periods`, 3)
    traffic.periods.forEach((count, index) => { finite(count, `${at}.periods[${index}]`); if (count < 0) fail(at, 'negative traffic') })
    integer(traffic.status, `${at}.status`, 0)
    integer(traffic.source_id, `${at}.source_id`, 0)
    integer(traffic.matching, `${at}.matching`, 0)
    if (traffic.status > 2 || traffic.source_id > 65535 || traffic.matching > 3) fail(at, 'invalid traffic evidence')
    validateProvenance(value[`${category}_provenance`], `${path}.${category}_provenance`)
  }
}
