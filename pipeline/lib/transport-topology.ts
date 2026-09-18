/** Read original OSM connectivity from each square's pieces file; whole ways and train routes come from `railway-globals`. */

import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import type { Table } from 'apache-arrow'
import { contractTable, firstRowAtOrAfter, railwayGlobals, type SourceNode } from './railway-globals.js'
import type { SegmentEndpointKeys } from './prepared-grid.js'

export type TransportFamily = 'roads' | 'railways'

export const sourcePiecesPath = (preparedDirectory: string, square: string, family: TransportFamily): string =>
  resolve(preparedDirectory, square, `${family}.pieces.arrow`)

export const transportPieceKey = (wayId: string, segmentIndex: number): string =>
  `${wayId}:${segmentIndex}`

export interface SourcePiecePassage {
  segmentIndex: number
  square: string
  from: number
  to: number
}

export interface OrientedSourceRailWay {
  id: string
  role: string
  nodes: SourceNode[]
  /** Extractor-computed cumulative metres of every node; the only source of original-way metres. */
  distances: number[]
  reverse: boolean
}

export interface SourceTrainRoute {
  id: string
  status: 'complete' | 'missing_source_ways' | 'unsupported_members' | 'ambiguous_or_disconnected_order'
  missingWays: string[]
  unsupportedMembers: Array<[identifier: string, role: string]>
  ways: OrientedSourceRailWay[]
}

/** Grid cells and length are the extractor's own raw-row values, so a restored parent equals its raw row. */
export interface SourceParentGeometry {
  start: [gx: number, gy: number]
  end: [gx: number, gy: number]
  lengthM: number
}

type OrientationState = { end: string; count: number; previous: number }
const observationRoles = new Set(['stop', 'stop_entry_only', 'stop_exit_only',
  'platform', 'platform_entry_only', 'platform_exit_only'])

const column = <T>(table: Table, name: string): T => table.getChild(name)!.toArray() as T

/** Column-backed pieces of one square, sorted by `(way_id, segment_idx)`. */
export class SquarePieces {
  private readonly wayIds: BigInt64Array
  private readonly segments: Int16Array
  // Flat arrays: a per-row Vector.get dominates a million-piece road square.
  private readonly columns = new Map<string, ArrayLike<number | bigint>>()

  /** `fileBytes` is what a cached square keeps alive: the table is a view of the file's buffer. */
  private constructor(readonly square: string, private readonly table: Table | null, readonly fileBytes = 0) {
    this.wayIds = table ? column(table, 'way_id') : new BigInt64Array()
    this.segments = table ? column(table, 'segment_idx') : new Int16Array()
  }

  /** A square without the file has no pieces; a caller needing one reports the piece it misses. */
  static read(preparedDirectory: string, square: string, family: TransportFamily): SquarePieces {
    const path = sourcePiecesPath(preparedDirectory, square, family)
    if (!existsSync(path)) return new SquarePieces(square, null)
    const bytes = readFileSync(path), table = contractTable(path, 'transport_pieces_contract', bytes)
    if (table.schema.metadata.get('family') !== family) throw new Error(`${path}: not a ${family} pieces file`)
    return new SquarePieces(square, table, bytes.length)
  }

  get count(): number { return this.wayIds.length }

  segmentIndex(row: number): number { return this.segments[row] }

  key(row: number): string { return transportPieceKey(String(this.wayIds[row]), this.segments[row]) }

  /** Row of one piece, or -1. */
  row(wayId: string, segmentIndex: number): number {
    const [first, end] = this.wayRows(wayId)
    for (let row = first; row < end; row++) if (this.segments[row] === segmentIndex) return row
    return -1
  }

  wayRows(wayId: string): [first: number, end: number] {
    const id = BigInt(wayId), first = firstRowAtOrAfter(this.wayIds, id)
    let end = first
    while (end < this.wayIds.length && this.wayIds[end] === id) end++
    return [first, end]
  }

  private value<T = number>(name: string, row: number): T {
    let values = this.columns.get(name)
    if (!values) {
      values = column<ArrayLike<number | bigint>>(this.table!, name)
      this.columns.set(name, values)
    }
    return values[row] as T
  }

  /** A fractional end is identified by its exact double on the way; a vertex end by its canonical node. */
  identity(row: number): SegmentEndpointKeys {
    const endpointKey = (side: 'start' | 'end'): string => {
      const fraction = this.value(`${side}_fraction`, row)
      return fraction > 0
        ? `way:${this.wayIds[row]}:${this.value(`${side}_vertex`, row)}+${fraction}`
        : `node:${this.value<bigint>(`${side}_node`, row)}`
    }
    return { startKey: endpointKey('start'), endKey: endpointKey('end') }
  }

  geometry(row: number): SourceParentGeometry {
    return { start: [this.value('start_gx', row), this.value('start_gy', row)],
      end: [this.value('end_gx', row), this.value('end_gy', row)], lengthM: this.value('length_m', row) }
  }

  extent(row: number): { from: number; to: number } {
    if (!this.table!.getChild('from_m')!.isValid(row)) {
      throw new Error(`source distances require a complete way: ${this.key(row)}`)
    }
    return { from: this.value('from_m', row), to: this.value('to_m', row) }
  }
}

// A country walk revisits its squares; the proxy process crosses the world (3.8 GB of railway pieces)
// and must not retain it. France, the largest national walk, fits this budget several times.
const CACHED_RAILWAY_SQUARE_BYTES = 256 << 20

export class SourceTransportTopology implements Disposable {
  private readonly prepared: string
  private readonly cachedSquares = new Map<string, SquarePieces>()
  private cachedSquareBytes = 0

  constructor(preparedDirectory: string, private readonly family: TransportFamily = 'railways',
    private readonly cachedSquareBytesBudget = CACHED_RAILWAY_SQUARE_BYTES) {
    this.prepared = resolve(preparedDirectory)
  }

  /** Road squares are read once per run and never cached. */
  squarePieces(square: string): SquarePieces {
    if (this.family === 'roads') return SquarePieces.read(this.prepared, square, this.family)
    let pieces = this.cachedSquares.get(square)
    if (!pieces) {
      pieces = SquarePieces.read(this.prepared, square, this.family)
      this.cachedSquares.set(square, pieces)
      this.cachedSquareBytes += pieces.fileBytes
      // Oldest first; the square just read always stays.
      for (const [oldest, evicted] of this.cachedSquares) {
        if (this.cachedSquareBytes <= this.cachedSquareBytesBudget || oldest === square) break
        this.cachedSquares.delete(oldest)
        this.cachedSquareBytes -= evicted.fileBytes
      }
    }
    return pieces
  }

  /** A caller that knows the square reads one file; a graph visit finds it through the way's squares. */
  pieceExtent(wayId: string, segmentIndex: number, square?: string): { square: string; from: number; to: number } {
    let candidates = [square!]
    if (!square) {
      const way = railwayGlobals(this.prepared).way(wayId)
      if (!way) throw new Error(`source way missing for piece ${wayId}:${segmentIndex}`)
      candidates = way.batch.squares(way.row)
    }
    for (const candidate of candidates) {
      const pieces = this.squarePieces(candidate), row = pieces.row(wayId, segmentIndex)
      if (row >= 0) return { square: candidate, ...pieces.extent(row) }
    }
    throw new Error(`source piece missing ${wayId}:${segmentIndex}`)
  }

  passagePieces(passage: { way: string; from: number; to: number }): SourcePiecePassage[] {
    const way = railwayGlobals(this.prepared).way(passage.way)
    if (!way) throw new Error(`source way missing for passage ${passage.way}`)
    const lower = Math.min(passage.from, passage.to), upper = Math.max(passage.from, passage.to)
    // An incomplete way has NaN length, which fails the upper bound.
    if (!Number.isFinite(lower) || !Number.isFinite(upper) ||
        lower < 0 || !(upper <= way.batch.wayLengthM(way.row)) || lower === upper) {
      throw new Error(`invalid source passage ${passage.way}:${passage.from}..${passage.to}`)
    }
    const wayPieces: SourcePiecePassage[] = []
    for (const square of way.batch.squares(way.row)) {
      const pieces = this.squarePieces(square), [first, end] = pieces.wayRows(passage.way)
      for (let row = first; row < end; row++) {
        wayPieces.push({ segmentIndex: pieces.segmentIndex(row), square, ...pieces.extent(row) })
      }
    }
    // Every bound is the extractor's stored double, so consecutive pieces meet exactly.
    wayPieces.sort((a, b) => a.from - b.from)
    const pieces: SourcePiecePassage[] = []
    let coveredUntil = lower
    for (const piece of wayPieces) {
      if (piece.to <= piece.from) throw new Error(`invalid source piece interval ${passage.way}:${piece.segmentIndex}`)
      const from = Math.max(lower, piece.from), to = Math.min(upper, piece.to)
      if (to <= from) continue
      if (from !== coveredUntil) throw new Error(`source piece gap or overlap for passage ${passage.way} at ${coveredUntil}`)
      pieces.push({ ...piece, from, to })
      coveredUntil = to
    }
    if (coveredUntil !== upper) throw new Error(`source pieces do not cover passage ${passage.way} to ${upper}`)
    return passage.from < passage.to ? pieces : pieces.reverse().map(piece => ({ ...piece, from: piece.to, to: piece.from }))
  }

  *trainRoutes(): Generator<SourceTrainRoute> {
    const routes = railwayGlobals(this.prepared).routes
    const ids = column<BigInt64Array>(routes, 'osm_id')
    const [kinds, memberIds, roles] = ['member_kind', 'member_id', 'member_role'].map(name => routes.getChild(name)!)
    for (let row = 0; row < ids.length; row++) {
      const memberRoles = Array.from(roles.get(row) as Iterable<string>)
      const memberKinds = Array.from(kinds.get(row) as Iterable<number>)
      yield this.orderTrainRoute(String(ids[row]), Array.from(memberIds.get(row) as Iterable<bigint>,
        (id, member) => [String.fromCharCode(memberKinds[member]), String(id), memberRoles[member]]))
    }
  }

  private orderTrainRoute(id: string, members: Array<[type: string, id: string, role: string]>): SourceTrainRoute {
    const ways: OrientedSourceRailWay[] = []
    const missingWays: string[] = []
    const unsupportedMembers: Array<[string, string]> = []
    for (const [type, wayId, role] of members) {
      if (observationRoles.has(role)) continue
      if (type !== 'w' || !['', 'forward', 'backward'].includes(role)) {
        unsupportedMembers.push([`${type}${wayId}`, role])
        continue
      }
      const way = railwayGlobals(this.prepared).way(wayId), chain = way && way.batch.trainRouteMemberChain(way.row)
      if (!chain) {
        missingWays.push(`w${wayId}`)
        continue
      }
      ways.push({ id: wayId, role, ...chain, reverse: false })
    }
    const unresolved = (status: SourceTrainRoute['status']): SourceTrainRoute =>
      ({ id, status, missingWays, unsupportedMembers, ways: [] })
    if (missingWays.length) return unresolved('missing_source_ways')
    if (unsupportedMembers.length) return unresolved('unsupported_members')

    const layers: Array<Array<OrientationState | undefined>> = []
    for (const way of ways) {
      const layer: Array<OrientationState | undefined> = [undefined, undefined]
      for (const reverse of [0, 1]) {
        if ((way.role === 'forward' && reverse === 1) || (way.role === 'backward' && reverse === 0)) continue
        const start = way.nodes[reverse ? way.nodes.length - 1 : 0][0]
        const end = way.nodes[reverse ? 0 : way.nodes.length - 1][0]
        const previous = layers.at(-1)
        if (!previous) { layer[reverse] = { end, count: 1, previous: -1 }; continue }
        let count = 0, predecessor = -1
        previous.forEach((state, orientation) => {
          if (state?.end !== start) return
          // Only uniqueness matters; saturated counts cannot overflow on repeated loops.
          count = Math.min(2, count + state.count)
          predecessor = orientation
        })
        if (count) layer[reverse] = { end, count, previous: predecessor }
      }
      layers.push(layer)
    }
    const last = layers.at(-1)
    if (!last || last.reduce((sum, state) => sum + (state?.count ?? 0), 0) !== 1) {
      return unresolved('ambiguous_or_disconnected_order')
    }
    let orientation = last.findIndex(state => state !== undefined)
    for (let index = ways.length - 1; index >= 0; index--) {
      ways[index].reverse = orientation === 1
      orientation = layers[index][orientation]!.previous
    }
    return { id, status: 'complete', missingWays, unsupportedMembers, ways }
  }

  [Symbol.dispose](): void {
    this.cachedSquares.clear()
    this.cachedSquareBytes = 0
  }
}
