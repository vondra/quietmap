//! Variant-specific contributor metadata assertions for popup payloads.

import {
  array,
  exactKeys,
  fail,
  finite,
  integer,
  nullableFinite,
  object,
  string,
} from './schema-values.mjs'

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
      ['kind', 'aadt_light_raw', 'aadt_medium_raw', 'aadt_heavy_raw', 'aadt_moto_raw',
        'traffic_source', 'dominant_source_id', 'speed_posted_kmh', 'aadt_light_nominal',
        'aadt_medium_nominal', 'aadt_heavy_nominal', 'aadt_moto_nominal',
        'aadt_light_effective', 'aadt_medium_effective', 'aadt_heavy_effective',
        'aadt_moto_effective', 'speed_kmh', 'speed_source', 'road_class', 'surface',
        'surface_corr_db', 'lanes', 'oneway', 'dominant_segment_idx',
        'dominant_distance_m', 'closest_distance_m', 'speed_min_kmh', 'speed_max_kmh',
        'oneway_segment_count', 'twoway_segment_count', 'segment_count', 'total_length_m',
        'bridge_count', 'obstacle_segment_count', 'obstacle_avg_height_m',
        'obstacle_max_height_m', 'obstacle_max_segment_idx'], ['provenance']],
    rail: [
      ['kind', 'trains_passenger_raw', 'trains_freight_raw', 'trains_passenger_source',
        'trains_freight_source', 'source_id', 'maxspeed_posted_kmh',
        'trains_passenger_effective', 'trains_freight_effective', 'speed_kmh',
        'speed_source', 'rail_type', 'usage', 'service', 'highspeed', 'parallel_divisor',
        'bridge', 'dominant_segment_idx', 'dominant_distance_m', 'closest_distance_m',
        'segment_count', 'total_length_m', 'obstacle_segment_count',
        'obstacle_avg_height_m', 'obstacle_max_height_m', 'obstacle_max_segment_idx'], ['provenance']],
    building: [['kind', 'height_m', 'floors', 'area_m2', 'building_type', 'address'], []],
    industrial: [['kind', 'area_m2', 'source_type', 'nace', 'grid_point_count', 'source_id', 'provenance'], []],
  }
  const expectedKind = sourceType === 'railway' ? 'rail' : sourceType
  const definition = definitions[expectedKind]
  if (!definition || value.kind !== expectedKind) fail(`${path}.kind`, `expected ${expectedKind}`)
  exactKeys(value, path, definition[0], definition[1])
  if (Object.hasOwn(value, 'provenance')) validateProvenance(value.provenance, `${path}.provenance`)
}
