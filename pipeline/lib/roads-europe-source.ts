/** Read the 36 staged European city traffic sources without modifying the cache. */

import { createHash } from 'node:crypto'
import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCE_ID_EU_CITY_TRAFFIC } from './source-ids.generated.js'
import type { RoadAadt } from './roads-arrow.js'
import { roadObservation } from './road-observation.js'

export const EUROPEAN_TRAFFIC_CITIES = [
  'Vienna', 'Brno', 'Copenhagen', 'Helsinki', 'Paris', 'Grenoble', 'Toulouse',
  'Lyon', 'Lille', 'Bordeaux', 'Rennes', 'Marseille', 'Rouen', 'Montpellier', 'Tours',
  'Berlin', 'Hamburg', 'Dublin', 'Milan', 'Luxembourg', 'Amsterdam', 'Oslo',
  'Lisbon', 'Valencia', 'Barcelona', 'Madrid', 'Malmo', 'Stockholm', 'Zurich',
  'Geneva', 'London', 'Birmingham', 'Manchester', 'Glasgow', 'Edinburgh', 'Cardiff',
] as const

export interface EuropeanTrafficRecord extends RoadAadt {
  /** GeoJSON-order `[longitude, latitude]` vertices; one for a Point, and a LineString runs in its travel direction. */
  coordinates: ReadonlyArray<readonly [number, number]>
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
    const total = nonnegativeNumber(properties.AADT ?? properties.AAWT ?? 0, `${context} AADT/AAWT`)
    const truck = nonnegativeNumber(properties.TR_AADT ?? properties.TR_AAWT ?? 0, `${context} truck count`)
    const motorcycle = nonnegativeNumber(properties['2W_AADT'] ?? properties['2W_AAWT'] ?? 0, `${context} motorcycle count`)
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
    if (truck + motorcycle > total) {
      result.rejected.push({ feature: index, reason: 'components_exceed_total', total, truck, motorcycle })
      continue
    }
    const roundedTotal = Math.round(total)
    const heavy = Math.round(truck)
    const moto = Math.round(motorcycle)
    const mediumEstimate = Math.min(Math.max(0, roundedTotal - heavy - moto), roundedTotal * 0.02)
    const counts = {
      light: Math.max(0, Math.round(roundedTotal - heavy - moto - mediumEstimate)),
      medium: Math.max(0, Math.round(mediumEstimate)), heavy, moto,
    }
    if (Object.values(counts).some(count => !Number.isSafeInteger(count) || count > 2_147_483_647)) {
      throw new Error(`${context}: vehicle count exceeds the prepared Int32 domain`)
    }
    if (Object.values(counts).every(count => count === 0)) {
      result.rejected.push({ feature: index, reason: 'rounds_to_zero', total, truck, motorcycle })
      continue
    }
    result.records.push({ coordinates, ...counts,
      sourceId: SOURCE_ID_EU_CITY_TRAFFIC,
      ...roadObservation({ city, feature: index, observation: feature },
        properties.raw_oneway === true ? 'directional' : 'street-cross-section'),
      sourceOsmId: typeof properties.osmid === 'number' && Number.isSafeInteger(properties.osmid)
        && properties.osmid > 0 ? properties.osmid : null,
      estimatedClasses: /estimat/i.test(String(properties.raw_techno ?? '')) ? 15
        : 3 | (properties.TR_AADT == null && properties.TR_AAWT == null ? 4 : 0)
          | (properties['2W_AADT'] == null && properties['2W_AAWT'] == null ? 8 : 0),
      rawOneway: properties.raw_oneway ?? null,
      rawDirection: properties.raw_direction ?? null,
      osmOneway: properties.osm_oneway ?? null,
      rawTechnology: properties.raw_techno ?? null,
    })
  }
  if (!result.records.length) throw new Error(`${city}: no usable traffic observations`)
  return result
}

/** Validate every city before the first prepared-road write. */
export function loadEuropeanCityTraffic(directory: string): EuropeanCityTraffic[] {
  return EUROPEAN_TRAFFIC_CITIES.map(city => {
    const path = latestStagedCityFile(city, directory)
    return parseEuropeanCityTraffic(city, path, readFileSync(path))
  })
}
