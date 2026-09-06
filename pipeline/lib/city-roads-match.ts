/** Original municipal street targeting: names, point-station competition and along-section distance. */

import type { RoadRow } from './roads-arrow.js'
import { M_PER_DEG_LAT, M_PER_DEG_LON_EQ, pointToPolylineDist } from './spatial.js'
import type { CityRoadRecord } from './city-roads-source.js'

export const normalizeCityStreet = (name: string): string => name.normalize('NFD').replace(/[\u0300-\u036f]/g, '').toLowerCase().replace(/\s+/g, ' ').trim()
const MATCH_CAP_M = 40 // Original municipal cap covers divided carriageways, not neighbouring blocks.
const LINE_NEAR_M = 28 // Original Brno calibration separates along-section names from parallel-street bleed.
const POINT_BAND_M = 15 // Original Wien station competition excludes a more distant unrelated arterial.
const effectiveClass = (cls: number) => cls === 10 ? 0.5 : cls === 11 ? 1.5 : cls === 12 ? 2.5 : cls

export function cityRecordDistance(record: CityRoadRecord, row: RoadRow): number {
  const line = record.line!
  if (line.length === 1) return pointToPolylineDist(line[0][1], line[0][0], [[row.startLon, row.startLat], [row.endLon, row.endLat]])
  return Math.max(pointToPolylineDist(row.startLat, row.startLon, line),
    pointToPolylineDist(row.midLat, row.midLon, line), pointToPolylineDist(row.endLat, row.endLon, line))
}

interface Candidate { name: string; cls: number; dist: number }
interface GeometryRecord {
  record: CityRoadRecord
  bbox: readonly [number, number, number, number]
  candidates: Candidate[]
  names: ReadonlySet<string>
}
function nearby(g: GeometryRecord, row: RoadRow): boolean {
  return Math.min(row.startLat, row.endLat) <= g.bbox[2] && Math.max(row.startLat, row.endLat) >= g.bbox[0] &&
    Math.min(row.startLon, row.endLon) <= g.bbox[3] && Math.max(row.startLon, row.endLon) >= g.bbox[1]
}
function pointName(candidates: readonly Candidate[], label: string): string | undefined {
  if (!candidates.length) return undefined
  const hint = normalizeCityStreet(label).replace(/ (i{1,3}|iv|v)$/, '')
  const distance = Math.min(...candidates.map(c => c.dist)) + POINT_BAND_M
  const groups = new Map<string, { cls: number; dist: number }>()
  for (const candidate of candidates) {
    if (candidate.dist > distance) continue
    const group = groups.get(candidate.name), cls = effectiveClass(candidate.cls)
    if (!group) groups.set(candidate.name, { cls, dist: candidate.dist })
    else { group.cls = Math.min(group.cls, cls); group.dist = Math.min(group.dist, candidate.dist) }
  }
  return [...groups].sort((a, b) => a[1].cls - b[1].cls || Number(b[0] === hint) - Number(a[0] === hint) || a[1].dist - b[1].dist)[0][0]
}

export function municipalRoadMatcher(records: readonly CityRoadRecord[], coverage: ReadonlySet<number>) {
  const byName = new Map<string, CityRoadRecord>(), geometry: GeometryRecord[] = []
  for (const record of records) {
    if (!record.line) { byName.set(normalizeCityStreet(record.street), record); continue }
    let south = Infinity, west = Infinity, north = -Infinity, east = -Infinity
    for (const [lon, lat] of record.line) {
      south = Math.min(south, lat); west = Math.min(west, lon); north = Math.max(north, lat); east = Math.max(east, lon)
    }
    const dy = MATCH_CAP_M / M_PER_DEG_LAT, dx = MATCH_CAP_M / (M_PER_DEG_LON_EQ * Math.cos(south * Math.PI / 180))
    geometry.push({ record, bbox: [south - dy, west - dx, north + dy, east + dx], candidates: [], names: new Set() })
  }
  return {
    observe(row: RoadRow): void {
      // Motorways must compete even though they cannot be stamped: otherwise their counters leak onto side streets.
      if (row.roadClass >= 6 && row.roadClass <= 8) return
      for (const g of geometry) {
        if (!nearby(g, row)) continue
        const dist = cityRecordDistance(g.record, row)
        if (dist <= MATCH_CAP_M) g.candidates.push({ name: normalizeCityStreet(row.name ?? ''), cls: row.roadClass, dist })
      }
    },
    finish(): number {
      for (const g of geometry) {
        if (g.record.line!.length === 1) {
          const name = pointName(g.candidates, g.record.street)
          g.names = new Set(name === undefined ? [] : [name])
        } else {
          const groups = new Map<string, { dist: number; cls: number }>()
          for (const candidate of g.candidates) {
            const old = groups.get(candidate.name)
            if (!old || candidate.dist < old.dist) groups.set(candidate.name, candidate)
          }
          const primary = [...groups.values()].sort((a, b) => a.dist - b.dist)[0]
          g.names = new Set(!primary || !coverage.has(primary.cls) ? [] : [...groups].filter(([, value]) =>
            value.dist <= LINE_NEAR_M && coverage.has(value.cls)).map(([name]) => name))
        }
        g.candidates = []
      }
      return geometry.filter(g => !g.names.size).length
    },
    match(row: RoadRow): CityRoadRecord | null {
      const name = normalizeCityStreet(row.name ?? ''), named = row.name ? byName.get(name) : undefined
      if (named) return named
      let best: CityRoadRecord | null = null, distance = MATCH_CAP_M
      for (const g of geometry) {
        if (!nearby(g, row) || !g.names.has(name)) continue
        const candidate = cityRecordDistance(g.record, row)
        if (candidate <= distance) { distance = candidate; best = g.record }
      }
      return best
    },
  }
}
