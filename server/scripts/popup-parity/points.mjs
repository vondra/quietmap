//! Deterministic worldwide popup parity coordinates and their provenance hashes.

import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'

export const CURATED_POINT_COUNT = 64
export const GENERATED_POINT_COUNT = 200
export const BROWSER_ACCEPTANCE_POINT_COUNT = 200
export const WORLD_POINTS_URL = new URL('../../../benchmarks/world-points.json', import.meta.url)

const GOLDEN_ANGLE_DEG = 137.50776405003785

function roundCoordinate(value) {
  return Number(value.toFixed(7))
}

function generatedRole(index) {
  return new Map([
    [0, 'east_180_boundary'],
    [1, 'east_180_inside'],
    [198, 'west_180_inside'],
    [199, 'west_180_boundary'],
  ]).get(index) ?? 'equal_area'
}

/**
 * Equal steps at latitude-band midpoints in sin(latitude) give equal-area
 * strata without the Web-Mercator-inexpressible poles. Both legal
 * antimeridian spellings and adjacent ±179.999999 probes distinguish input
 * boundary handling from seam handling.
 */
export function generateEqualAreaPoints(count = GENERATED_POINT_COUNT) {
  if (!Number.isInteger(count) || count < 7) {
    throw new Error('equal-area point count must be an integer >= 7')
  }
  return Array.from({ length: count }, (_, index) => {
    const z = 1 - (2 * (index + 0.5)) / count
    const lat = roundCoordinate(Math.asin(z) * 180 / Math.PI)
    let lng = roundCoordinate((((index * GOLDEN_ANGLE_DEG + 180) % 360) + 360) % 360 - 180)
    if (index === 0) lng = 180
    if (index === 1) lng = 179.999999
    if (index === count - 2) lng = -179.999999
    if (index === count - 1) lng = -180
    return {
      id: `equal-area-v1-${String(index).padStart(3, '0')}`,
      lat,
      lng,
      group: 'equal-area-v1',
      role: generatedRole(index),
    }
  })
}

function assertPoint(point, label) {
  if (point === null || typeof point !== 'object' || Array.isArray(point)) {
    throw new Error(`${label} must be an object`)
  }
  if ((typeof point.id !== 'string' && typeof point.id !== 'number') || String(point.id).length === 0) {
    throw new Error(`${label}.id must be a non-empty string or number`)
  }
  if (!Number.isFinite(point.lat) || point.lat < -90 || point.lat > 90) {
    throw new Error(`${label}.lat must be within [-90, 90]`)
  }
  if (!Number.isFinite(point.lng) || point.lng < -180 || point.lng > 180) {
    throw new Error(`${label}.lng must be within [-180, 180]`)
  }
}

export function coordinatesSha256(points) {
  const coordinates = points.map(({ id, lat, lng }) => [String(id), lat, lng])
  return createHash('sha256').update(JSON.stringify(coordinates)).digest('hex')
}

/**
 * Browser acceptance keeps each distinct curated location, then takes
 * midpoint-stratified indices from the 200-point equal-area diagnostic set.
 * The four antimeridian boundary probes are retained explicitly.
 */
export function selectBrowserAcceptancePoints(points) {
  const catalogCurated = points.filter((point) => point.group === 'curated-world')
  const generated = points.filter((point) => point.group === 'equal-area-v1')
  if (catalogCurated.length !== CURATED_POINT_COUNT || generated.length !== GENERATED_POINT_COUNT) {
    throw new Error('browser selection requires the complete 64 + 200 point catalog')
  }
  const location = ({ lat, lng }) => JSON.stringify([lat, lng === 180 ? -180 : lng])
  const seen = new Set()
  const curated = catalogCurated.filter((point) => {
    const key = location(point)
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
  const interiorCount = BROWSER_ACCEPTANCE_POINT_COUNT - curated.length - 4
  const interiorRange = GENERATED_POINT_COUNT - 4
  const generatedIndices = [0, 1]
  for (let index = 0; index < interiorCount; index += 1) {
    generatedIndices.push(2 + Math.floor(((index + 0.5) * interiorRange) / interiorCount))
  }
  generatedIndices.push(GENERATED_POINT_COUNT - 2, GENERATED_POINT_COUNT - 1)
  const selected = [...curated, ...generatedIndices.map((index) => generated[index])]
  if (selected.length !== BROWSER_ACCEPTANCE_POINT_COUNT
      || new Set(selected.map(location)).size !== BROWSER_ACCEPTANCE_POINT_COUNT) {
    throw new Error('browser selection must contain 200 distinct locations')
  }
  return selected
}

export async function loadParityPoints(worldPointsUrl = WORLD_POINTS_URL) {
  const worldBytes = await readFile(worldPointsUrl)
  let curated
  try {
    curated = JSON.parse(worldBytes)
  } catch (error) {
    throw new Error(`invalid world-points JSON: ${error.message}`)
  }
  if (!Array.isArray(curated) || curated.length !== CURATED_POINT_COUNT) {
    throw new Error(`world-points must contain exactly ${CURATED_POINT_COUNT} points`)
  }
  const world = curated.map((point) => ({ ...point, group: 'curated-world' }))
  const generated = generateEqualAreaPoints()
  const points = [...world, ...generated]
  const ids = new Set()
  points.forEach((point, index) => {
    assertPoint(point, `points[${index}]`)
    const id = String(point.id)
    if (ids.has(id)) throw new Error(`duplicate point id: ${id}`)
    ids.add(id)
  })
  const browserPoints = selectBrowserAcceptancePoints(points)
  return {
    points,
    browserPoints,
    browserCoordinatesSha256: coordinatesSha256(browserPoints),
    hashes: {
      world_coordinates_sha256: coordinatesSha256(world),
      equal_area_coordinates_sha256: coordinatesSha256(generated),
      all_coordinates_sha256: coordinatesSha256(points),
    },
  }
}
