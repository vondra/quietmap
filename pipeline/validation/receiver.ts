/**
 * Receivers the criteria define around a station: the interior rule's outdoor point (nearest
 * footprint outline moved outward) and the outdoor, off-carriageway points of the position circle.
 */
import type { PopupAnswer } from './popup.ts'

export type Point = { lat: number; lng: number }
export type Probe = {
  popup: (point: Point) => Promise<PopupAnswer>
  /** True when the point lies inside an enclosed footprint: `/api/building-at` answers `building_exposure`. */
  inside: (point: Point) => Promise<boolean>
}

const METRES_PER_DEGREE_LATITUDE = 111_320
/** Sixteen search bearings: the nearest outline is then found within ±11.25°, refined to ±2.8°. */
const SEARCH_BEARINGS_DEG = Array.from({ length: 16 }, (_, index) => index * 22.5)
const REFINE_OFFSETS_DEG = [-8.4375, -2.8125, 2.8125, 8.4375]
/** A footprint whose outline is farther than this from the station has no usable outdoor receiver. */
const OUTLINE_SEARCH_LIMIT_M = 128
const OUTLINE_PRECISION_M = 0.1
/** Criteria v2 position rule: points on this many bearings of the circle. */
const POSITION_BEARINGS_DEG = [0, 45, 90, 135, 180, 225, 270, 315]
/** Criteria v2 position rule: a point within lanes × 1.75 m of a road centreline is on the carriageway. */
const LANE_HALF_WIDTH_M = 1.75

export function offsetPoint(point: Point, bearingDeg: number, metres: number): Point {
  const bearing = bearingDeg * Math.PI / 180
  return {
    lat: point.lat + metres * Math.cos(bearing) / METRES_PER_DEGREE_LATITUDE,
    lng: point.lng + metres * Math.sin(bearing) / (METRES_PER_DEGREE_LATITUDE * Math.cos(point.lat * Math.PI / 180)),
  }
}

/** Distance along a bearing to the first outdoor point: doubling, then bisection to 0.1 m. */
async function exitDistance(station: Point, bearing: number, probe: Probe): Promise<number | null> {
  let inner = 0
  let outer = 0.5
  while (await probe.inside(offsetPoint(station, bearing, outer))) {
    inner = outer
    outer *= 2
    if (outer > OUTLINE_SEARCH_LIMIT_M) return null
  }
  while (outer - inner > OUTLINE_PRECISION_M) {
    const middle = (inner + outer) / 2
    if (await probe.inside(offsetPoint(station, bearing, middle))) inner = middle
    else outer = middle
  }
  return outer
}

/**
 * Criteria v2 interior rule: the nearest point of the containing footprint's outline, moved outward
 * along the outline normal by the facade offset. The shortest exit bearing stands for the normal.
 */
export async function interiorReceiver(station: Point, offsetM: number, probe: Probe):
  Promise<{ point: Point; moved_m: number; bearing_deg: number } | { reason: string }> {
  const exits: Array<{ bearing: number; distance: number }> = []
  for (const bearing of SEARCH_BEARINGS_DEG) {
    const distance = await exitDistance(station, bearing, probe)
    if (distance != null) exits.push({ bearing, distance })
  }
  if (exits.length === 0) return { reason: `no footprint outline within ${OUTLINE_SEARCH_LIMIT_M} m` }
  const coarse = exits.reduce((best, exit) => exit.distance < best.distance ? exit : best)
  for (const offset of REFINE_OFFSETS_DEG) {
    const bearing = (coarse.bearing + offset + 360) % 360
    const distance = await exitDistance(station, bearing, probe)
    if (distance != null) exits.push({ bearing, distance })
  }
  const best = exits.reduce((nearest, exit) => exit.distance < nearest.distance ? exit : nearest)
  const moved = best.distance + offsetM
  const point = offsetPoint(station, best.bearing, moved)
  if (await probe.inside(point)) return { reason: 'the outward receiver lies inside another footprint' }
  return { point, moved_m: +moved.toFixed(2), bearing_deg: best.bearing }
}

/** A point inside a road's lanes: some road contributor's centreline is nearer than lanes × 1.75 m. */
export function onCarriageway(answer: PopupAnswer): boolean {
  return answer.top_contributors.some(contributor => {
    const metadata = contributor.metadata
    if (contributor.source_type !== 'road' || typeof metadata?.closest_distance_m !== 'number') return false
    const lanes = typeof metadata.lanes === 'number' && metadata.lanes > 0 ? metadata.lanes : 1
    return metadata.closest_distance_m < lanes * LANE_HALF_WIDTH_M
  })
}

export type PositionSample = { bearing_deg: number; lden: number | null; ln: number | null; dropped: string | null }

/** The model on the circle of the position radius; indoor and carriageway points are kept but dropped. */
export async function positionSamples(center: Point, radiusM: number, probe: Probe, read: (answer: PopupAnswer, point: Point) => { lden: number | null; ln: number | null }): Promise<PositionSample[]> {
  const samples: PositionSample[] = []
  for (const bearing of POSITION_BEARINGS_DEG) {
    const point = offsetPoint(center, bearing, radiusM)
    // A point inside a building answers its building exposure, never an outdoor sample.
    if (await probe.inside(point)) {
      samples.push({ bearing_deg: bearing, lden: null, ln: null, dropped: 'inside a footprint' })
      continue
    }
    const answer = await probe.popup(point)
    const levels = read(answer, point)
    samples.push({ bearing_deg: bearing, lden: levels.lden, ln: levels.ln, dropped: onCarriageway(answer) ? 'on a carriageway' : null })
  }
  return samples
}
