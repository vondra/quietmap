/** Admit all load-bearing special-national inputs before one country-owned Arrow pass. */

import { createHash } from 'node:crypto'
import { readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import type { MatchFacility } from './facility-match.js'
import { gemAreaContains } from './industrial-gem-countries.js'
import { gemIndustrialOwnership, strictGemCountries } from './industrial-gem-source.js'
import { iso2Code } from './prepared-grid.js'
import { geoJsonAreaCentroid } from './spatial.js'
import { PROVENANCE_RANK, SOURCES_BY_ID, SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX } from './sources.js'
import { SPECIAL_COUNTRIES, type SpecialCountry } from './industrial-special-countries.js'
import { SPECIAL_CONTAINMENT_FEEDS, SPECIAL_FEEDS, type SpecialFeed } from './industrial-special-policy.js'
import { colombianConcessions, concessionClassifier, nationalContainmentAreas,
  type IndustrialConcession } from './industrial-special-polygons.js'

export interface SpecialFeature { geometry?: { type: string; coordinates: unknown } | null; properties?: Record<string, unknown> | null }
const source = SOURCES_BY_ID.get(SOURCE_ID_GLOBAL_INDUSTRIAL_NATIONAL_MIX)!
const authority = { id: source.id, rank: PROVENANCE_RANK[source.provenance], year: source.year ?? 0 }

export function readSpecialFeatures(path: string) {
  const before = statSync(path, { bigint: true }), bytes = readFileSync(path)
  const json = JSON.parse(bytes.toString('utf8')) as { type?: unknown; features?: unknown[] }
  if (json?.type !== 'FeatureCollection' || !Array.isArray(json.features) || !json.features.length ||
      json.features.some(f => !f || typeof f !== 'object' || Array.isArray(f))) throw new Error(`${path}: missing or malformed nonempty FeatureCollection`)
  const after = statSync(path, { bigint: true })
  if (before.dev !== after.dev || before.ino !== after.ino || before.size !== after.size || before.mtimeNs !== after.mtimeNs || before.ctimeNs !== after.ctimeNs) {
    throw new Error(`${path}: changed during special-national source admission`)
  }
  return { features: json.features as SpecialFeature[], receipt: { path, bytes: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex') } }
}



export function specialPoints(features: readonly SpecialFeature[], country: SpecialCountry, feed: SpecialFeed) {
  const points = [], counts = { raw: features.length, unlocated: 0 }
  const geometryTypes = feed.geometry ?? ['Point']
  for (const [index, feature] of features.entries()) {
    const g = feature.geometry
    let coordinates: unknown
    if (g?.type === 'Point' && geometryTypes.includes('Point')) coordinates = g.coordinates
    else if (country.country === 'BR' && g?.type === 'MultiPoint' && Array.isArray(g.coordinates)) coordinates = g.coordinates[0]
    else if (g && (g.type === 'Polygon' || g.type === 'MultiPolygon') && geometryTypes.includes(g.type)) {
      // A polygon is a source footprint; preserve published Point locations verbatim.
      coordinates = geoJsonAreaCentroid(g.coordinates, g.type)
    }
    if (!Array.isArray(coordinates) || coordinates[0] == null || coordinates[1] == null) { counts.unlocated++; continue }
    const [lon, lat] = coordinates
    if (typeof lat !== 'number' || typeof lon !== 'number' || !Number.isFinite(lat) || !Number.isFinite(lon) ||
        Math.abs(lat) > 90 || Math.abs(lon) > 180) throw new Error(`${country.country}: invalid point coordinate ${index}`)
    const properties = feature.properties ?? {}
    if (typeof properties !== 'object' || Array.isArray(properties)) throw new Error(`${country.country}: invalid properties ${index}`)
    points.push({ lat, lon, properties })
  }
  return { points, counts }
}

export function classifySpecialPoints(
  features: readonly SpecialFeature[], country: SpecialCountry, feed: SpecialFeed,
  seen: Set<string>, landCodes?: readonly number[],
) {
  const { points, counts: geometryCounts } = specialPoints(features, country, feed)
  if (country.country === 'VN' && landCodes?.length !== points.length) throw new Error('VN source requires strict aligned land countries')
  const counts = { ...geometryCounts, coordinates: points.length, area: 0, active: 0, classified: 0,
    outside: 0, inactive: 0, unclassified: 0, duplicate: 0, emitted: 0 }
  const facilities: MatchFacility[] = []
  for (const [index, point] of points.entries()) {
    const { lat, lon, properties } = point
    const inArea = gemAreaContains(country, lat, lon, 0) && (country.country !== 'VN' || landCodes![index] === iso2Code('VN'))
    if (inArea) counts.area++
    const border = country.country === 'PY' && /itaip|yacyret|acaray/i.test(String(properties.Plant___Project_name ?? ''))
    if (!inArea && !border) { counts.outside++; continue }
    if (feed.active && !feed.active(properties)) { counts.inactive++; continue }
    counts.active++
    const nace4 = feed.classify(properties)
    if (nace4 !== null) counts.classified++
    const key = feed.deduplicateDigits === undefined ? null
      : `${lat.toFixed(feed.deduplicateDigits)}_${lon.toFixed(feed.deduplicateDigits)}`
    if (nace4 !== null || key !== null) {
      if (key !== null && seen.has(key)) { counts.duplicate++; continue }
      if (key !== null) seen.add(key)
    }
    if (nace4 === null) { counts.unclassified++; continue }
    facilities.push({ lat, lon, nace4, ...authority, searchRadiusM: country.radiusM }); counts.emitted++
  }
  if (!counts[feed.require]) throw new Error(`${country.country}/${feed.file}: empty ${feed.require} source; refusing a partial reset`)
  return { facilities, counts }
}

function addContainment(
  byCountry: Map<string, IndustrialConcession[]>,
  country: string,
  polygons: readonly IndustrialConcession[],
): void {
  const existing = byCountry.get(country) ?? []
  existing.push(...polygons)
  byCountry.set(country, existing)
}

export function loadSpecialIndustrialSources(directory: string, boundaries: string) {
  const facilities: MatchFacility[] = [], facilityCountries: string[] = [], receipts = []
  const containmentByCountry = new Map<string, IndustrialConcession[]>()
  for (const country of SPECIAL_COUNTRIES) {
    const seen = new Set<string>()
    const feeds = SPECIAL_FEEDS[country.country]
    if (!feeds?.length) throw new Error(`${country.country}: missing special-industrial feed policy`)
    for (const feed of feeds) {
      const loaded = readSpecialFeatures(resolve(directory, country.country.toLowerCase(), feed.file))
      const points = specialPoints(loaded.features, country, feed).points
      const codes = country.country === 'VN' ? strictGemCountries(points, boundaries) : undefined
      const admitted = classifySpecialPoints(loaded.features, country, feed, seen, codes)
      facilities.push(...admitted.facilities)
      facilityCountries.push(...admitted.facilities.map(() => country.country))
      receipts.push({ country: country.country, ...loaded.receipt, ...admitted.counts })
    }
    if (country.country === 'CO') {
      for (const file of ['mining-titles.geojson', 'oil-gas-blocks.geojson']) {
        const loaded = readSpecialFeatures(resolve(directory, 'co', file))
        const admitted = colombianConcessions(loaded.features, file.startsWith('mining'), country.bbox)
        addContainment(containmentByCountry, country.country, admitted.polygons)
        receipts.push({ country: 'CO', ...loaded.receipt, ...admitted.counts })
      }
    }
    for (const feed of SPECIAL_CONTAINMENT_FEEDS[country.country] ?? []) {
      const loaded = readSpecialFeatures(resolve(directory, country.country.toLowerCase(), feed.file))
      const admitted = nationalContainmentAreas(loaded.features, feed, country.bbox)
      addContainment(containmentByCountry, country.country, admitted.polygons)
      receipts.push({ country: country.country, ...loaded.receipt, ...admitted.counts })
    }
  }
  const ownership = gemIndustrialOwnership(SPECIAL_COUNTRIES, facilityCountries)
  const classifiers = new Map([...containmentByCountry].map(([country, polygons]) => [country, concessionClassifier(polygons)]))
  ownership.rowClassification = (country, polygon) => {
    const nace4 = classifiers.get(country)?.(polygon.lat, polygon.lon) ?? null
    return nace4 === null ? null : { lat: polygon.lat, lon: polygon.lon, nace4, ...authority }
  }
  return { facilities, ownership, resetSourceIds: [authority.id], receipts }
}
