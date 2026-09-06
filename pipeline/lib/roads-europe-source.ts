/** Read the 36 staged European city traffic sources without modifying the cache. */

import { createHash } from 'node:crypto'
import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { SOURCE_ID_EU_CITY_TRAFFIC } from './source-ids.generated.js'
import type { RoadAadt } from './roads-arrow.js'

export const EUROPEAN_TRAFFIC_CITIES = [
  'Vienna', 'Brno', 'Copenhagen', 'Helsinki', 'Paris', 'Grenoble', 'Toulouse',
  'Lyon', 'Lille', 'Bordeaux', 'Rennes', 'Marseille', 'Rouen', 'Montpellier', 'Tours',
  'Berlin', 'Hamburg', 'Dublin', 'Milan', 'Luxembourg', 'Amsterdam', 'Oslo',
  'Lisbon', 'Valencia', 'Barcelona', 'Madrid', 'Malmo', 'Stockholm', 'Zurich',
  'Geneva', 'London', 'Birmingham', 'Manchester', 'Glasgow', 'Edinburgh', 'Cardiff',
] as const

export interface EuropeanTrafficRecord extends RoadAadt {
  latitude: number
  longitude: number
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
    const coordinates = geometry.coordinates
    const point = geometry.type === 'Point' ? coordinates
      : geometry.type === 'LineString' && Array.isArray(coordinates)
        ? coordinates[Math.floor(coordinates.length / 2)] : null
    if (!Array.isArray(point) || point.length < 2 ||
        typeof point[0] !== 'number' || typeof point[1] !== 'number' ||
        !Number.isFinite(point[0]) || !Number.isFinite(point[1]) ||
        point[0] < -180 || point[0] > 180 || point[1] < -90 || point[1] > 90) {
      throw new Error(`${context}: invalid Point/LineString representative coordinate`)
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
    // Preserve dev1 rounding, directional totals and its 2% medium-vehicle estimate.
    const factor = properties.raw_oneway === true ? 2 : 1
    const directionalTotal = Math.round(total) * factor
    const heavy = Math.round(truck) * factor
    const moto = Math.round(motorcycle) * factor
    const mediumEstimate = directionalTotal * 0.02
    const counts = {
      light: Math.max(0, Math.round(directionalTotal - heavy - moto - mediumEstimate)),
      medium: Math.max(0, Math.round(mediumEstimate)), heavy, moto,
    }
    if (Object.values(counts).some(count => !Number.isSafeInteger(count) || count > 2_147_483_647)) {
      throw new Error(`${context}: vehicle count exceeds the prepared Int32 domain`)
    }
    if (Object.values(counts).every(count => count === 0)) {
      result.rejected.push({ feature: index, reason: 'rounds_to_zero', total, truck, motorcycle })
      continue
    }
    result.records.push({ latitude: point[1], longitude: point[0], ...counts, sourceId: SOURCE_ID_EU_CITY_TRAFFIC })
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
