/** Enrich z9 road vectors with Great Britain DfT AADF count points: major roads by ref, minor roads by location. */

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { SOURCE_ID_GB_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { shouldOverwrite } from './lib/provenance.js'
import { runRoadLoaderCli, type RoadLoaderArguments } from './lib/road-loader-cli.js'
import { DFT_MINOR_ROAD_RANK, loadDftCountPoints, type DftCountPoint } from './lib/roads-gb-source.js'
import { roadClassTakesCount, writeRoadAadt, type RoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { gridToLonLat, listPreparedSquares, lonLatToGrid } from './lib/prepared-grid.js'
import { writeNationalRoadSquares } from './lib/square-pool.js'
import { flatDist, haversineM, pointToSegmentDist } from './lib/spatial.js'

const SOURCE_ID = SOURCE_ID_GB_NATIONAL_ROADS
const GREAT_BRITAIN_BBOX = [49, -8.5, 61, 2.5] as const
const MAJOR_ROAD_REF_REACH_METRES = 15_000

export type { DftCountPoint } from './lib/roads-gb-source.js'

export function majorRoadIndex(points: readonly DftCountPoint[]): ReadonlyMap<string, readonly DftCountPoint[]> {
  const index = new Map<string, DftCountPoint[]>()
  // A slip-road point never stands for its mainline; minor points carry only "C" or "U" as their name.
  for (const point of points) {
    if (!point.ref || point.isRamp || point.rank === DFT_MINOR_ROAD_RANK) continue
    const bucket = index.get(point.ref)
    if (bucket) bucket.push(point)
    else index.set(point.ref, [point])
  }
  return index
}

/** The nearest same-ref point within 15 km whose DfT class the row can carry (a track tagged A538 takes no A-road count). */
export function matchDftPoint(
  row: RoadRow,
  pointsByRef: ReadonlyMap<string, readonly DftCountPoint[]>,
): DftCountPoint | null {
  let closest: DftCountPoint | null = null
  let closestDistance = MAJOR_ROAD_REF_REACH_METRES
  for (const candidate of pointsByRef.get(row.ref?.replace(/\s+/g, '') ?? '') ?? []) {
    if (!roadClassTakesCount(row.roadClass, candidate)) continue
    const distance = haversineM(row.midLat, row.midLon, candidate.latitude, candidate.longitude)
    if (distance < closestDistance) {
      closest = candidate
      closestDistance = distance
    }
  }
  return closest
}

// A minor-road point names no road ("C" or "U"): it counts the OSM way it sits on when that way is
// unambiguous (w3-local pilot, 2026-09-24: 4,576 training and 1,616 holdout manual counts at these gates).
const MINOR_POINT_ON_WAY_METRES = 12
const MINOR_POINT_OTHER_CLASS_CLEARANCE_METRES = 20

interface NearestRows { distance: number; roadClass: number; osmId: number; classDistances: Map<number, number> }

// z30 cells span at most 0.0245 m in Great Britain (49 degrees north), so 1,000 cells cover the 20 m clearance.
const CLEARANCE_IN_GRID_CELLS = 1_000
const COARSE_CELL_SHIFT = 16

/** Which OSM way each minor-road point counts, read from the geometry of every GB square; traffic writes never
 *  change geometry, so sibling shards compute the same assignment. */
export function assignMinorPointsToWays(preparedDirectory: string, points: readonly DftCountPoint[]): Map<number, DftCountPoint[]> {
  const coarse = new Map<string, Array<{ point: DftCountPoint; gx: number; gy: number }>>()
  for (const point of points) {
    if (point.rank !== DFT_MINOR_ROAD_RANK) continue
    const [gx, gy] = lonLatToGrid(point.longitude, point.latitude)
    const key = `${gx >> COARSE_CELL_SHIFT}_${gy >> COARSE_CELL_SHIFT}`
    coarse.set(key, [...coarse.get(key) ?? [], { point, gx, gy }])
  }
  const nearest = new Map<DftCountPoint, NearestRows>()
  for (const square of coarse.size ? listPreparedSquares(preparedDirectory, GREAT_BRITAIN_BBOX) : []) {
    const table = tableFromIPC(readFileSync(resolve(preparedDirectory, square, 'roads.arrow')))
    const [sx, sy, ex, ey] = ['start_gx', 'start_gy', 'end_gx', 'end_gy'].map(name => table.getChild(name)!.toArray() as Int32Array)
    const ids = table.getChild('osm_id')!, classes = table.getChild('road_class')!
    for (let index = 0; index < table.numRows; index++) {
      const west = Math.min(sx[index], ex[index]) - CLEARANCE_IN_GRID_CELLS, east = Math.max(sx[index], ex[index]) + CLEARANCE_IN_GRID_CELLS
      const south = Math.min(sy[index], ey[index]) - CLEARANCE_IN_GRID_CELLS, north = Math.max(sy[index], ey[index]) + CLEARANCE_IN_GRID_CELLS
      for (let cx = west >> COARSE_CELL_SHIFT; cx <= east >> COARSE_CELL_SHIFT; cx++) {
        for (let cy = south >> COARSE_CELL_SHIFT; cy <= north >> COARSE_CELL_SHIFT; cy++) {
          for (const { point, gx, gy } of coarse.get(`${cx}_${cy}`) ?? []) {
            if (gx < west || gx > east || gy < south || gy > north) continue
            const start = gridToLonLat(sx[index], sy[index]), end = gridToLonLat(ex[index], ey[index])
            const distance = pointToSegmentDist(point.latitude, point.longitude, start.lat, start.lon, end.lat, end.lon)
            if (distance > MINOR_POINT_OTHER_CLASS_CLEARANCE_METRES) continue
            const roadClass = Number(classes.get(index)), osmId = Number(ids.get(index))
            const best = nearest.get(point) ?? { distance: Infinity, roadClass, osmId, classDistances: new Map() }
            best.classDistances.set(roadClass, Math.min(best.classDistances.get(roadClass) ?? Infinity, distance))
            if (distance < best.distance || (distance === best.distance && osmId < best.osmId)) {
              Object.assign(best, { distance, roadClass, osmId })
            }
            nearest.set(point, best)
          }
        }
      }
    }
  }
  const byWay = new Map<number, DftCountPoint[]>()
  for (const [point, { distance, roadClass, osmId, classDistances }] of nearest) {
    const anotherClassNearby = [...classDistances.keys()].some(otherClass => otherClass !== roadClass)
    if (distance > MINOR_POINT_ON_WAY_METRES || anotherClassNearby || !roadClassTakesCount(roadClass, point)) continue
    byWay.set(osmId, [...byWay.get(osmId) ?? [], point])
  }
  return byWay
}

/** Every piece of a counted way takes the count of that way's nearest counted point. */
export function matchMinorRoadPoint(row: RoadRow, pointsByWay: ReadonlyMap<number, readonly DftCountPoint[]>): DftCountPoint | null {
  let closest: DftCountPoint | null = null, closestDistance = Infinity
  for (const point of row.osmId === null ? [] : pointsByWay.get(row.osmId) ?? []) {
    const distance = flatDist(row.midLat, row.midLon, point.latitude, point.longitude)
    if (distance < closestDistance || (distance === closestDistance && point.observationId < closest!.observationId)) {
      closest = point
      closestDistance = distance
    }
  }
  return closest
}

export async function enrichGreatBritainRoads(
  preparedDirectory: string,
  points: readonly DftCountPoint[],
) {
  const pointsByRef = majorRoadIndex(points)
  const pointsByWay = assignMinorPointsToWays(preparedDirectory, points)
  const match = (row: RoadRow): DftCountPoint | null => matchDftPoint(row, pointsByRef) ?? matchMinorRoadPoint(row, pointsByWay)
  return writeNationalRoadSquares(preparedDirectory, GREAT_BRITAIN_BBOX, 'Great Britain', {}, path =>
    writeRoadAadt(
      path,
      (row): RoadAadt | null => {
        if (!shouldOverwrite(row.existingSourceId, SOURCE_ID)) return null
        const point = match(row)
        return point ? { countBasis: point.countBasis, observationId: point.observationId,
          light: point.light, medium: point.medium, heavy: point.heavy,
          moto: point.moto, sourceId: SOURCE_ID, estimatedClasses: 0, // DfT publishes every class
        } : null
      },
      undefined,
      undefined,
      { sourceIds: [SOURCE_ID], when: row => match(row) === null },
    ))
}

async function main(options: RoadLoaderArguments) {
  const points = await loadDftCountPoints(options)
  const result = await enrichGreatBritainRoads(options.preparedDirectory, points)
  return { points: points.length, slipRoadPoints: points.filter(point => point.isRamp).length,
    minorRoadPoints: points.filter(point => point.rank === DFT_MINOR_ROAD_RANK).length, ...result }
}

runRoadLoaderCli(import.meta.url, main)
