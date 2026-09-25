/** Read the 36 staged European city traffic sources without modifying the cache. */

import { withholdsCountLine } from './count-holdout.js'
import { createHash } from 'node:crypto'
import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCE_ID_EU_CITY_TRAFFIC, SOURCE_ID_NL_AMSTERDAM_TRAFFIC_MODEL } from './source-ids.generated.js'
import type { RoadAadt } from './roads-arrow.js'
import { WORLD_DEFAULT } from './road-planning-defaults.generated.js'
import { roadObservation, type RoadObservation } from './road-observation.js'

// Tours is left out: its upstream terms require written authorisation (w3-major licence review, 2026-09-24).
export const EUROPEAN_TRAFFIC_CITIES = [
  'Vienna', 'Brno', 'Copenhagen', 'Helsinki', 'Paris', 'Grenoble', 'Toulouse',
  'Lyon', 'Lille', 'Bordeaux', 'Rennes', 'Marseille', 'Rouen', 'Montpellier',
  'Berlin', 'Hamburg', 'Dublin', 'Milan', 'Luxembourg', 'Amsterdam', 'Oslo',
  'Lisbon', 'Valencia', 'Barcelona', 'Madrid', 'Malmo', 'Stockholm', 'Zurich',
  'Geneva', 'London', 'Birmingham', 'Manchester', 'Glasgow', 'Edinburgh', 'Cardiff',
] as const

export interface EuropeanTrafficRecord extends RoadObservation {
  /** GeoJSON-order `[longitude, latitude]` vertices; one for a Point, and a LineString runs in its travel direction. */
  coordinates: ReadonlyArray<readonly [number, number]>
  sourceId: number
  /** Annual average daily total; a weekday-only total is converted by `ANNUAL_PER_WEEKDAY_TRAFFIC`. */
  total: number
  /** Published heavy and motorcycle counts; null where the publisher counted none. */
  heavy: number | null
  moto: number | null
  estimatedClasses: number
  /** The publisher's own OSM match as an engine road class, null outside the highway classes. */
  publisherRoadClass: number | null
  /** Normalized street names the publisher gives (`osm_name`, `raw_name`). */
  names: readonly string[]
  sourceOsmId: number | null
  rawOneway: unknown
  rawDirection: unknown
  osmOneway: unknown
  rawTechnology: unknown
}

export interface RejectedTrafficRecord {
  feature: number
  reason: 'components_exceed_total' | 'rounds_to_zero'
  total: number
  truck: number
  motorcycle: number
}

export interface EuropeanCityTraffic {
  city: string
  path: string
  sha256: string
  features: number
  records: EuropeanTrafficRecord[]
  rejected: RejectedTrafficRecord[]
  nonBooleanOneway: number
}

/** Raw filenames retain their observation year; normalized copies are not a second authority. */
export function latestStagedCityFile(city: string, directory: string): string {
  const prefix = `${city.toLowerCase()}_`
  const choices = readdirSync(directory).filter(name =>
    name.toLowerCase().startsWith(prefix) && /\d{4}\.geojson$/i.test(name))
  choices.sort((a, b) => {
    const year = (name: string) => Number(/(\d{4})\.geojson$/i.exec(name)![1])
    return year(b) - year(a) || (a < b ? -1 : a > b ? 1 : 0)
  })
  if (!choices.length) throw new Error(`${city}: missing staged yearly GeoJSON under ${directory}`)
  return resolve(directory, choices[0])
}

function object(value: unknown, description: string): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${description}: expected an object`)
  }
  return value as Record<string, unknown>
}

function nonnegativeNumber(value: unknown, description: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    throw new Error(`${description}: expected a finite non-negative number`)
  }
  return value
}

// Median of city medians of AADT/AAWT, using only training geometries of holdout rule v1:
// 8,425 paired records in 13 staged cities, recomputed 2026-09-25 from the files this loader reads.
// The unrounded median is 0.9274139531168848; reserved counts do not change the rounded value.
export const ANNUAL_PER_WEEKDAY_TRAFFIC = 0.9274
// 4,568 of Marseille's 10,819 records publish zero trucks on streets with buses and deliveries; the
// publisher does not say they were counted (owner decision, 2026-09-24: treat as missing).
const CITIES_WITH_PLACEHOLDER_ZERO_TRUCKS: ReadonlySet<string> = new Set(['Marseille'])
const CITY_TRAFFIC_MODELS: ReadonlyMap<string, number> = new Map([['Amsterdam', SOURCE_ID_NL_AMSTERDAM_TRAFFIC_MODEL]])

/** `road_class` of the OSM extractor (`osm-extract` mappers.rs) for the highway values publishers use. */
const OSM_HIGHWAY_ROAD_CLASS: Readonly<Record<string, number>> = {
  motorway: 0, trunk: 1, primary: 2, secondary: 3, secondary_link: 3, tertiary: 4, tertiary_link: 4,
  residential: 5, living_street: 6, service: 7, track: 8, unclassified: 9,
  motorway_link: 10, trunk_link: 11, primary_link: 12,
}

export const normalizedStreetName = (name: unknown): string =>
  typeof name === 'string' ? name.normalize('NFKC').toLowerCase().replace(/\s+/g, ' ').trim() : ''

const optionalCount = (value: unknown, description: string): number | null =>
  value === null || value === undefined ? null : nonnegativeNumber(value, description)

export function parseEuropeanCityTraffic(city: string, path: string, bytes: Buffer): EuropeanCityTraffic {
  const data = object(JSON.parse(bytes.toString('utf8')), `${city}: GeoJSON`)
  if (data.type !== 'FeatureCollection' || !Array.isArray(data.features) || !data.features.length) {
    throw new Error(`${city}: expected a nonempty GeoJSON FeatureCollection`)
  }
  const result: EuropeanCityTraffic = {
    city, path, sha256: createHash('sha256').update(bytes).digest('hex'),
    features: data.features.length, records: [], rejected: [], nonBooleanOneway: 0,
  }
  for (const [index, value] of data.features.entries()) {
    const context = `${city}: feature ${index}`
    const feature = object(value, context)
    const properties = object(feature.properties, context)
    const weekdayOnly = properties.AADT == null && properties.AAWT != null
    const published = nonnegativeNumber(properties.AADT ?? properties.AAWT ?? 0, `${context} AADT/AAWT`)
    const percentage = optionalCount(properties.TR_pct_AADT ?? properties.TR_pct_AAWT, `${context} truck share`)
    let truck = optionalCount(properties.TR_AADT ?? properties.TR_AAWT, `${context} truck count`) ??
      (percentage === null ? null : published * percentage / 100)
    if (truck === 0 && CITIES_WITH_PLACEHOLDER_ZERO_TRUCKS.has(city)) truck = null
    const motorcycle = optionalCount(properties['2W_AADT'] ?? properties['2W_AAWT'], `${context} motorcycle count`)
    const geometry = object(feature.geometry, `${context} geometry`)
    const vertices: unknown = geometry.type === 'Point' ? [geometry.coordinates]
      : geometry.type === 'LineString' ? geometry.coordinates : null
    if (!Array.isArray(vertices) || !vertices.length) throw new Error(`${context}: expected a nonempty Point/LineString`)
    const coordinates: Array<readonly [number, number]> = []
    for (const vertex of vertices) {
      if (!Array.isArray(vertex) || vertex.length < 2 ||
          typeof vertex[0] !== 'number' || typeof vertex[1] !== 'number' ||
          !Number.isFinite(vertex[0]) || !Number.isFinite(vertex[1]) ||
          vertex[0] < -180 || vertex[0] > 180 || vertex[1] < -90 || vertex[1] > 90) {
        throw new Error(`${context}: invalid Point/LineString coordinate`)
      }
      // A repeated vertex has no heading, and a line of one repeated vertex is a point.
      const previous = coordinates.at(-1)
      if (previous?.[0] !== vertex[0] || previous[1] !== vertex[1]) coordinates.push([vertex[0], vertex[1]])
    }
    if (properties.raw_oneway !== undefined && typeof properties.raw_oneway !== 'boolean') {
      result.nonBooleanOneway++
    }
    // The publisher defines trucks and motorcycles as components of the total.
    // Reject contradictory observations; neither swapping nor capping is evidence.
    if ((truck ?? 0) + (motorcycle ?? 0) > published) {
      result.rejected.push({ feature: index, reason: 'components_exceed_total', total: published, truck: truck ?? 0, motorcycle: motorcycle ?? 0 })
      continue
    }
    const scale = weekdayOnly ? ANNUAL_PER_WEEKDAY_TRAFFIC : 1
    const total = Math.round(published * scale)
    if (total === 0) {
      result.rejected.push({ feature: index, reason: 'rounds_to_zero', total: published, truck: truck ?? 0, motorcycle: motorcycle ?? 0 })
      continue
    }
    if (total > 2_147_483_647) throw new Error(`${context}: vehicle count exceeds the prepared Int32 domain`)
    const modelSource = CITY_TRAFFIC_MODELS.get(city)
    const heavy = truck === null ? null : Math.round(truck * scale), moto = motorcycle === null ? null : Math.round(motorcycle * scale)
    result.records.push({ coordinates, total, heavy, moto,
      sourceId: modelSource ?? SOURCE_ID_EU_CITY_TRAFFIC,
      ...roadObservation({ city, feature: index, observation: feature },
        properties.raw_oneway === true ? 'directional' : 'street-cross-section'),
      sourceOsmId: typeof properties.osmid === 'number' && Number.isSafeInteger(properties.osmid)
        && properties.osmid > 0 ? properties.osmid : null,
      estimatedClasses: modelSource !== undefined || weekdayOnly || /estimat/i.test(String(properties.raw_techno ?? '')) ? 15
        : 3 | (heavy === null ? 4 : 0) | (moto === null ? 8 : 0),
      publisherRoadClass: OSM_HIGHWAY_ROAD_CLASS[String(properties.osm_type)] ?? null,
      names: [...new Set([properties.osm_name, properties.raw_name].map(normalizedStreetName).filter(Boolean))],
      rawOneway: properties.raw_oneway ?? null,
      rawDirection: properties.raw_direction ?? null,
      osmOneway: properties.osm_oneway ?? null,
      rawTechnology: properties.raw_techno ?? null,
    })
  }
  if (!result.records.length) throw new Error(`${city}: no usable traffic observations`)
  result.records = result.records.filter(record => !withholdsCountLine(record.coordinates))
  return result
}

/** Four classes on one row: published classes as counted, the rest in the row class's prior proportions. */
export function europeanTrafficAadt(record: EuropeanTrafficRecord, roadClass: number): RoadAadt {
  const [light, medium, heavy, moto] = WORLD_DEFAULT[Math.min(roadClass, WORLD_DEFAULT.length - 1)]
  // Estimated classes never exceed what the published classes leave of the total; light takes the rest.
  let remaining = record.total - (record.heavy ?? 0) - (record.moto ?? 0)
  const estimate = (part: number): number => {
    const value = Math.min(remaining, Math.round(record.total * part / (light + medium + heavy + moto)))
    remaining -= value
    return value
  }
  const heavyCount = record.heavy ?? estimate(heavy), motoCount = record.moto ?? estimate(moto), mediumCount = estimate(medium)
  return { countBasis: record.countBasis, observationId: record.observationId, sourceId: record.sourceId,
    estimatedClasses: record.estimatedClasses, light: remaining, medium: mediumCount, heavy: heavyCount, moto: motoCount }
}

/** Validate every city before the first prepared-road write. */
export function loadEuropeanCityTraffic(directory: string): EuropeanCityTraffic[] {
  return EUROPEAN_TRAFFIC_CITIES.map(city => {
    const path = latestStagedCityFile(city, directory)
    return parseEuropeanCityTraffic(city, path, readFileSync(path))
  })
}
