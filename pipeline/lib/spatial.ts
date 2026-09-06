/** Canonical distance and topology helpers used by road enrichment loaders. */

const METRES_PER_DEGREE_LATITUDE = 110_540
const METRES_PER_DEGREE_LONGITUDE_AT_EQUATOR = 111_320

/** Stable ~1 metre endpoint identity shared by road topology passes. */
export function nodeKey(latitude: number, longitude: number): string {
  return `${latitude.toFixed(5)}_${longitude.toFixed(5)}`
}

function wrappedLongitudeDelta(deltaDegrees: number): number {
  if (deltaDegrees > 180) return deltaDegrees - 360
  if (deltaDegrees < -180) return deltaDegrees + 360
  return deltaDegrees
}

/** Flat-earth distance in metres, accurate for the local matching radii used here. */
export function flatDist(
  firstLatitude: number,
  firstLongitude: number,
  secondLatitude: number,
  secondLongitude: number,
): number {
  const cosineLatitude = Math.cos((firstLatitude + secondLatitude) / 2 * Math.PI / 180)
  const x = wrappedLongitudeDelta(secondLongitude - firstLongitude) *
    METRES_PER_DEGREE_LONGITUDE_AT_EQUATOR * cosineLatitude
  const y = (secondLatitude - firstLatitude) * METRES_PER_DEGREE_LATITUDE
  return Math.hypot(x, y)
}

/** Great-circle distance in metres for loaders whose matching spans longer ranges. */
export function haversineM(
  firstLatitude: number,
  firstLongitude: number,
  secondLatitude: number,
  secondLongitude: number,
): number {
  const radiusMetres = 6_371_008.8
  const firstRadians = firstLatitude * Math.PI / 180
  const secondRadians = secondLatitude * Math.PI / 180
  const latitudeDelta = (secondLatitude - firstLatitude) * Math.PI / 180
  const longitudeDelta = (secondLongitude - firstLongitude) * Math.PI / 180
  const halfChordSquared = Math.sin(latitudeDelta / 2) ** 2 +
    Math.cos(firstRadians) * Math.cos(secondRadians) * Math.sin(longitudeDelta / 2) ** 2
  return 2 * radiusMetres * Math.asin(Math.min(1, Math.sqrt(halfChordSquared)))
}

/** Distance from a point to a line segment, all coordinates latitude first. */
export function pointToSegmentDist(
  pointLatitude: number,
  pointLongitude: number,
  startLatitude: number,
  startLongitude: number,
  endLatitude: number,
  endLongitude: number,
): number {
  const cosineLatitude = Math.cos(pointLatitude * Math.PI / 180)
  const pointX = wrappedLongitudeDelta(pointLongitude - startLongitude) *
    METRES_PER_DEGREE_LONGITUDE_AT_EQUATOR * cosineLatitude
  const pointY = (pointLatitude - startLatitude) * METRES_PER_DEGREE_LATITUDE
  const endX = wrappedLongitudeDelta(endLongitude - startLongitude) *
    METRES_PER_DEGREE_LONGITUDE_AT_EQUATOR * cosineLatitude
  const endY = (endLatitude - startLatitude) * METRES_PER_DEGREE_LATITUDE
  const lengthSquared = endX * endX + endY * endY
  if (lengthSquared < 1e-6) {
    return flatDist(pointLatitude, pointLongitude, startLatitude, startLongitude)
  }
  const parameter = Math.max(0, Math.min(1, (pointX * endX + pointY * endY) / lengthSquared))
  return Math.hypot(pointX - parameter * endX, pointY - parameter * endY)
}

/** Minimum distance from a point to a GeoJSON-order `[longitude, latitude]` polyline. */
export function pointToPolylineDist(
  pointLatitude: number,
  pointLongitude: number,
  coordinates: ReadonlyArray<readonly [number, number]>,
): number {
  if (coordinates.length === 0) return Infinity
  if (coordinates.length === 1) {
    return flatDist(pointLatitude, pointLongitude, coordinates[0][1], coordinates[0][0])
  }
  let closest = Infinity
  for (let index = 0; index < coordinates.length - 1; index++) {
    closest = Math.min(closest, pointToSegmentDist(
      pointLatitude,
      pointLongitude,
      coordinates[index][1],
      coordinates[index][0],
      coordinates[index + 1][1],
      coordinates[index + 1][0],
    ))
  }
  return closest
}

export interface PointCoordinates {
  latitude: number
  longitude: number
}

export interface RankedPoint extends PointCoordinates {
  /** Null means the source does not publish a compatible functional class. */
  rank: number | null
}

const POINT_GRID_SCALE = 100
const LONGITUDE_CELL_COUNT = 360 * POINT_GRID_SCALE
const MINIMUM_LONGITUDE_CELL = -180 * POINT_GRID_SCALE

function longitudeCell(longitude: number): number {
  const normalized = ((longitude + 180) % 360 + 360) % 360 - 180
  return Math.floor(normalized * POINT_GRID_SCALE)
}

function wrappedLongitudeCell(cell: number): number {
  return ((cell - MINIMUM_LONGITUDE_CELL) % LONGITUDE_CELL_COUNT + LONGITUDE_CELL_COUNT) %
    LONGITUDE_CELL_COUNT + MINIMUM_LONGITUDE_CELL
}

const pointGridKey = (latitudeCell: number, longitudeCellValue: number): string =>
  `${latitudeCell}_${longitudeCellValue}`

/** Index point observations once for the road loaders' bounded proximity queries. */
export function buildOneHundredthDegreePointGrid<T extends PointCoordinates>(
  points: readonly T[],
): ReadonlyMap<string, readonly T[]> {
  const grid = new Map<string, T[]>()
  for (const point of points) {
    if (!Number.isFinite(point.latitude) || !Number.isFinite(point.longitude) ||
        point.latitude < -90 || point.latitude > 90 ||
        point.longitude < -180 || point.longitude > 180) {
      throw new Error(`invalid ranked point coordinates: ${point.latitude},${point.longitude}`)
    }
    const key = pointGridKey(
      Math.floor(point.latitude * POINT_GRID_SCALE),
      longitudeCell(point.longitude),
    )
    const bucket = grid.get(key)
    if (bucket) bucket.push(point)
    else grid.set(key, [point])
  }
  return grid
}

/** Degree reaches enclosing both shared distance models, including polar queries. */
export function pointSearchReach(latitude: number, radiusMetres: number): [number, number] {
  const latitudeReach = radiusMetres / METRES_PER_DEGREE_LATITUDE
  const edgeLatitude = Math.min(90, Math.abs(latitude) + latitudeReach)
  return [latitudeReach, Math.min(180, latitudeReach / Math.cos(edgeLatitude * Math.PI / 180))]
}

/** Conservative cell candidates; callers retain their exact distance and class gates. */
export function* pointGridCandidates<T extends PointCoordinates>(
  latitude: number,
  longitude: number,
  radiusMetres: number,
  grid: ReadonlyMap<string, readonly T[]>,
): Iterable<T> {
  // The smaller latitude scale safely bounds both flatDist and haversineM.
  const [latitudeReach, longitudeReach] = pointSearchReach(latitude, radiusMetres)
  const west = Math.floor((longitude - longitudeReach) * POINT_GRID_SCALE)
  const east = Math.min(west + LONGITUDE_CELL_COUNT - 1,
    Math.floor((longitude + longitudeReach) * POINT_GRID_SCALE))
  for (let y = Math.floor((latitude - latitudeReach) * POINT_GRID_SCALE);
    y <= Math.floor((latitude + latitudeReach) * POINT_GRID_SCALE); y++) {
    for (let x = west; x <= east; x++) {
      const bucket = grid.get(pointGridKey(y, wrappedLongitudeCell(x)))
      if (bucket) yield* bucket
    }
  }
}

/** Find the nearest class-compatible point under the proven strict 200 m cap. */
export function nearestCompatiblePointWithin200Metres<T extends RankedPoint>(
  latitude: number,
  longitude: number,
  roadRank: number,
  rankTolerance: number,
  grid: ReadonlyMap<string, readonly T[]>,
): T | null {
  let closest: T | null = null
  let closestDistance = 200
  for (const point of pointGridCandidates(latitude, longitude, closestDistance, grid)) {
    if (point.rank !== null && Math.abs(point.rank - roadRank) > rankTolerance) continue
    const distance = haversineM(latitude, longitude, point.latitude, point.longitude)
    if (distance < closestDistance) {
      closest = point
      closestDistance = distance
    }
  }
  return closest
}

/** Inclusive `[south, west, north, east]` membership. */
export function inBbox(
  latitude: number,
  longitude: number,
  bbox: readonly [number, number, number, number],
): boolean {
  return latitude >= bbox[0] && latitude <= bbox[2] &&
    longitude >= bbox[1] && longitude <= bbox[3]
}
