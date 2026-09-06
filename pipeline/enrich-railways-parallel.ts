/** Terminal z9 pass that prevents parallel railway rows multiplying traffic. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { DataType, tableFromIPC, type Table, type Vector } from 'apache-arrow'
import { DATASETS } from './lib/enrichment-datasets.js'
import {
  listPreparedSquares, segmentGeometryReader, type PreparedBbox, type SegmentEndpointKeys, type SegmentGeometry,
} from './lib/prepared-grid.js'
import { writeRailParallelDivisor } from './lib/railways-arrow.js'
import {
  M_PER_DEG_LAT, M_PER_DEG_LON_EQ, pointToSegmentDist, wrapLonDeltaDeg,
} from './lib/spatial.js'

const PARALLEL_RADIUS_M = 50
const DIVISOR_CAP = 3
const GRID_CELL_M = 128
const QUERY_PAD_M = PARALLEL_RADIUS_M + 10
const Z9_AXIS = 512
const WORLD_BBOX: PreparedBbox = [-90, -180, 90, 180]

/** The registry is the sole authority for graph-walk-owned divisors. */
const RAIL_DIVISOR_FROM_WALK_SOURCE_IDS = new Set(
  DATASETS.filter(dataset => dataset.railDivisorFromWalk).map(dataset => dataset.id),
)

export interface ParallelRailRow extends SegmentGeometry, SegmentEndpointKeys {
  osmId: string
  corridorToken: string
  railType: number
  usage: number
  service: number
  sourceId: number
}

export interface RailwayParallelStats {
  rows: number
  eligibleRows: number
  serviceRows: number
  noTokenRows: number
  computedHist: [number, number, number]
  writtenHist: [number, number, number]
  keptExistingAbove1: number
  changed: boolean
}

export interface RailwayParallelRunStats extends RailwayParallelStats {
  squares: number
  squaresUpdated: number
}

function requiredInteger(table: Table, name: string, signed: boolean, bitWidth: number): Vector {
  const vector = table.getChild(name)
  if (!vector || !DataType.isInt(vector.type) || vector.type.isSigned !== signed ||
      vector.type.bitWidth !== bitWidth || vector.nullCount !== 0) {
    throw new Error(`railways Arrow '${name}' must be non-null ${signed ? 'Int' : 'Uint'}${bitWidth}`)
  }
  return vector
}

function requiredUtf8(table: Table, name: string): Vector {
  const vector = table.getChild(name)
  if (!vector || !DataType.isUtf8(vector.type)) throw new Error(`railways Arrow '${name}' must be Utf8`)
  return vector
}

function readParallelRows(path: string): { rows: ParallelRailRow[]; divisors: Uint8Array } {
  const table = tableFromIPC(readFileSync(path))
  const geometry = segmentGeometryReader(table)
  const osmId = requiredInteger(table, 'osm_id', true, 64)
  const ref = requiredUtf8(table, 'ref')
  const name = requiredUtf8(table, 'name')
  const railType = requiredInteger(table, 'rail_type', false, 8)
  const usage = requiredInteger(table, 'usage', false, 8)
  const service = requiredInteger(table, 'service', false, 8)
  const source = requiredInteger(table, 'source_id', false, 16)
  const existingDivisor = table.getChild('parallel_divisor')
  if (existingDivisor && (!DataType.isInt(existingDivisor.type) || existingDivisor.type.isSigned ||
      existingDivisor.type.bitWidth !== 8 || existingDivisor.nullCount !== 0)) {
    throw new Error("railways Arrow 'parallel_divisor' must be non-null Uint8")
  }

  const rows: ParallelRailRow[] = []
  const divisors = new Uint8Array(table.numRows)
  for (let index = 0; index < table.numRows; index++) {
    const rowGeometry = geometry.row(index)
    const reference = ((ref.get(index) as string | null) ?? '').trim()
    const rowName = ((name.get(index) as string | null) ?? '').trim()
    rows.push({
      ...rowGeometry,
      ...geometry.endpointKeys(index),
      osmId: String(osmId.get(index)),
      corridorToken: reference || rowName,
      railType: railType.get(index) as number,
      usage: usage.get(index) as number,
      service: service.get(index) as number,
      sourceId: source.get(index) as number,
    })
    divisors[index] = ((existingDivisor?.get(index) as number | null) ?? 1) || 1
  }
  return { rows, divisors }
}

interface ProjectedSegment {
  startX: number
  endX: number
  startY: number
  endY: number
}

/** Compute a divisor per row; null means this terminal pass does not own it. */
export function computeParallelDivisors(rows: readonly ParallelRailRow[]): Array<number | null> {
  const divisors: Array<number | null> = new Array(rows.length).fill(null)
  const groups = new Map<string, number[]>()
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index]
    if (row.service > 0 || !row.corridorToken ||
        RAIL_DIVISOR_FROM_WALK_SOURCE_IDS.has(row.sourceId)) continue
    const key = `${row.corridorToken}\x1f${row.railType}\x1f${row.usage}`
    const group = groups.get(key)
    if (group) group.push(index)
    else groups.set(key, [index])
  }

  for (const indices of groups.values()) {
    const firstOsmId = rows[indices[0]].osmId
    if (indices.every(index => rows[index].osmId === firstOsmId)) {
      for (const index of indices) divisors[index] = 1
      continue
    }

    const anchorLongitude = rows[indices[0]].startLon
    const metresPerLongitudeDegree = Math.max(
      1e-6,
      M_PER_DEG_LON_EQ * Math.cos(rows[indices[0]].midLat * Math.PI / 180),
    )
    const projected = new Map<number, ProjectedSegment>()
    const grid = new Map<string, number[]>()
    for (const index of indices) {
      const row = rows[index]
      const startLongitude = wrapLonDeltaDeg(row.startLon - anchorLongitude)
      const endLongitude = startLongitude + wrapLonDeltaDeg(row.endLon - row.startLon)
      const segment = {
        startX: startLongitude * metresPerLongitudeDegree,
        endX: endLongitude * metresPerLongitudeDegree,
        startY: row.startLat * M_PER_DEG_LAT,
        endY: row.endLat * M_PER_DEG_LAT,
      }
      projected.set(index, segment)
      for (let x = Math.floor(Math.min(segment.startX, segment.endX) / GRID_CELL_M);
        x <= Math.floor(Math.max(segment.startX, segment.endX) / GRID_CELL_M); x++) {
        for (let y = Math.floor(Math.min(segment.startY, segment.endY) / GRID_CELL_M);
          y <= Math.floor(Math.max(segment.startY, segment.endY) / GRID_CELL_M); y++) {
          const key = `${x}_${y}`
          const cell = grid.get(key)
          if (cell) cell.push(index)
          else grid.set(key, [index])
        }
      }
    }

    for (const index of indices) {
      const row = rows[index]
      const segment = projected.get(index)!
      const midpointX = (segment.startX + segment.endX) / 2
      const midpointY = (segment.startY + segment.endY) / 2
      const foundOsmIds = new Set<string>([row.osmId])
      const seenSegments = new Set<number>()
      outer: for (let x = Math.floor((midpointX - QUERY_PAD_M) / GRID_CELL_M);
        x <= Math.floor((midpointX + QUERY_PAD_M) / GRID_CELL_M); x++) {
        for (let y = Math.floor((midpointY - QUERY_PAD_M) / GRID_CELL_M);
          y <= Math.floor((midpointY + QUERY_PAD_M) / GRID_CELL_M); y++) {
          for (const candidateIndex of grid.get(`${x}_${y}`) ?? []) {
            if (foundOsmIds.size >= DIVISOR_CAP) break outer
            if (candidateIndex === index || seenSegments.has(candidateIndex)) continue
            seenSegments.add(candidateIndex)
            const candidate = rows[candidateIndex]
            if (foundOsmIds.has(candidate.osmId)) continue
            if (candidate.startKey === row.startKey || candidate.startKey === row.endKey ||
                candidate.endKey === row.startKey || candidate.endKey === row.endKey) continue
            if (pointToSegmentDist(
              row.midLat, row.midLon,
              candidate.startLat, candidate.startLon, candidate.endLat, candidate.endLon,
            ) < PARALLEL_RADIUS_M) foundOsmIds.add(candidate.osmId)
          }
        }
      }
      divisors[index] = Math.min(DIVISOR_CAP, foundOsmIds.size)
    }
  }
  return divisors
}

function emptyStats(): RailwayParallelStats {
  return {
    rows: 0,
    eligibleRows: 0,
    serviceRows: 0,
    noTokenRows: 0,
    computedHist: [0, 0, 0],
    writtenHist: [0, 0, 0],
    keptExistingAbove1: 0,
    changed: false,
  }
}

/** Enrich one owner square, using adjacent squares as read-only 50 m context. */
export async function enrichRailwayParallelSquare(
  arrowPath: string,
  options: { contextPaths?: readonly string[]; dryRun?: boolean } = {},
): Promise<RailwayParallelStats> {
  const owner = readParallelRows(arrowPath)
  const stats = emptyStats()
  stats.rows = owner.rows.length
  const rows = [...owner.rows]
  for (const contextPath of options.contextPaths ?? []) rows.push(...readParallelRows(contextPath).rows)
  const computed = computeParallelDivisors(rows)
  const decisions: Array<number | null> = new Array(owner.rows.length).fill(null)
  let needsWrite = false
  for (let index = 0; index < owner.rows.length; index++) {
    const row = owner.rows[index]
    if (row.service > 0) stats.serviceRows++
    else if (!row.corridorToken) stats.noTokenRows++
    const divisor = computed[index]
    if (divisor === null) continue
    stats.eligibleRows++
    stats.computedHist[divisor - 1]++
    if (owner.divisors[index] > 1) {
      stats.keptExistingAbove1++
    } else if (divisor > 1) {
      decisions[index] = divisor
      stats.writtenHist[divisor - 1]++
      needsWrite = true
    }
  }
  if (needsWrite && !options.dryRun) {
    stats.changed = (await writeRailParallelDivisor(arrowPath, index => decisions[index] ?? null)).updated
  }
  return stats
}

function neighbouringRailwayPaths(preparedDirectory: string, square: string): string[] {
  const match = /^z9\/(\d+)\/(\d+)$/.exec(square)
  if (!match) throw new Error(`invalid prepared square '${square}'`)
  const ownerX = Number(match[1])
  const ownerY = Number(match[2])
  const paths: string[] = []
  for (let xOffset = -1; xOffset <= 1; xOffset++) {
    for (let yOffset = -1; yOffset <= 1; yOffset++) {
      if (xOffset === 0 && yOffset === 0) continue
      const x = (ownerX + xOffset + Z9_AXIS) % Z9_AXIS
      const y = ownerY + yOffset
      if (y < 0 || y >= Z9_AXIS) continue
      const path = resolve(preparedDirectory, 'z9', String(x), String(y), 'railways.arrow')
      if (existsSync(path)) paths.push(path)
    }
  }
  return paths
}

/** Run last, after every national, proxy and GTFS railway count pass. */
export async function enrichRailwayParallelDirectory(
  preparedDirectory: string,
  bbox: PreparedBbox,
  options: { dryRun?: boolean } = {},
): Promise<RailwayParallelRunStats> {
  const prepared = resolve(preparedDirectory)
  const squares = listPreparedSquares(prepared, bbox, 'railways.arrow')
  if (squares.length === 0) throw new Error(`no railways.arrow source squares found under ${prepared}`)
  const total: RailwayParallelRunStats = { ...emptyStats(), squares: squares.length, squaresUpdated: 0 }
  for (const square of squares) {
    const stats = await enrichRailwayParallelSquare(resolve(prepared, square, 'railways.arrow'), {
      contextPaths: neighbouringRailwayPaths(prepared, square),
      dryRun: options.dryRun,
    })
    total.rows += stats.rows
    total.eligibleRows += stats.eligibleRows
    total.serviceRows += stats.serviceRows
    total.noTokenRows += stats.noTokenRows
    total.keptExistingAbove1 += stats.keptExistingAbove1
    total.squaresUpdated += Number(stats.changed)
    for (let index = 0; index < 3; index++) {
      total.computedHist[index] += stats.computedHist[index]
      total.writtenHist[index] += stats.writtenHist[index]
    }
  }
  total.changed = total.squaresUpdated > 0
  return total
}

function parseCli(argv: readonly string[]): { prepared: string; bbox: PreparedBbox; dryRun: boolean } {
  const { values } = parseArgs({
    args: [...argv],
    strict: true,
    allowPositionals: false,
    options: {
      'prepared-dir': { type: 'string' },
      bbox: { type: 'string' },
      world: { type: 'boolean' },
      'dry-run': { type: 'boolean' },
    },
  })
  if (!values['prepared-dir'] || Boolean(values.world) === Boolean(values.bbox)) {
    throw new Error(
      'usage: enrich-railways-parallel.ts --prepared-dir DIR (--world | --bbox S,W,N,E) [--dry-run]',
    )
  }
  const bbox = values.world ? WORLD_BBOX : values.bbox!.split(',').map(Number) as unknown as PreparedBbox
  if (bbox.length !== 4) throw new Error(`invalid bbox '${values.bbox}'`)
  return { prepared: resolve(values['prepared-dir']), bbox, dryRun: values['dry-run'] ?? false }
}

async function main(): Promise<void> {
  const options = parseCli(process.argv.slice(2))
  console.log(JSON.stringify(await enrichRailwayParallelDirectory(options.prepared, options.bbox, options)))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
