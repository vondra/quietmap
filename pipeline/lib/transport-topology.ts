/** Read original OSM connectivity from the build database beside the prepared year. */

import { basename, dirname, join, resolve } from 'node:path'
import { DatabaseSync, type StatementSync } from 'node:sqlite'
import { normalizeLongitude, type SegmentEndpointKeys } from './prepared-grid.js'
import { flatDist } from './spatial.js'

export const transportTopologyPath = (preparedDirectory: string): string => {
  const directory = resolve(preparedDirectory)
  return join(dirname(directory), `${basename(directory)}.transport.sqlite`)
}

export const transportPieceKey = (wayId: string, segmentIndex: number): string =>
  `${wayId}:${segmentIndex}`

export type SourceNode = [id: string, coordinates: [number, number] | null]

export function sourceNodeDistances(nodes: readonly SourceNode[]): number[] {
  if (nodes.length < 2 || nodes.some(node => node[1] === null)) {
    throw new Error('source distances require a complete way with at least two nodes')
  }
  const distances = [0]
  for (let index = 1; index < nodes.length; index++) {
    distances.push(distances.at(-1)! + flatDist(...nodes[index - 1][1]!, ...nodes[index][1]!))
  }
  return distances
}

function assertSourcePosition(nodes: readonly SourceNode[], way: string, vertex: number, fraction: number): void {
  if (!Number.isInteger(vertex) || vertex < 0 || !nodes[vertex]?.[1] ||
      !Number.isFinite(fraction) || fraction < 0 || fraction >= 1 ||
      (fraction > 0 && !nodes[vertex + 1]?.[1])) {
    throw new Error(`invalid source position ${way}:${vertex}+${fraction}`)
  }
}

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
  reverse: boolean
}

export interface SourceTrainRoute {
  id: string
  status: 'complete' | 'missing_source_ways' | 'unsupported_members' | 'ambiguous_or_disconnected_order'
  missingWays: string[]
  unsupportedMembers: Array<[identifier: string, role: string]>
  ways: OrientedSourceRailWay[]
}

type RouteMember = [type: string, id: string, role: string]
type OrientationState = { end: string; count: number; previous: number }
const observationRoles = new Set(['stop', 'stop_entry_only', 'stop_exit_only',
  'platform', 'platform_entry_only', 'platform_exit_only'])

interface SourcePiece {
  way_id: string
  segment_idx: number
  start_vertex: number
  start_fraction: number
  end_vertex: number
  end_fraction: number
}

function sourcePieceIdentity(nodes: readonly SourceNode[], wayId: string, piece: SourcePiece): SegmentEndpointKeys {
  const endpointKey = (vertex: number, fraction: number): string => {
    assertSourcePosition(nodes, wayId, vertex, fraction)
    return fraction > 0 ? `way:${wayId}:${vertex}+${fraction}` : `node:${nodes[vertex][0]}`
  }
  return { startKey: endpointKey(piece.start_vertex, piece.start_fraction),
    endKey: endpointKey(piece.end_vertex, piece.end_fraction) }
}

export class SourceTransportTopology implements Disposable {
  private readonly database: DatabaseSync
  private readonly pieces: StatementSync
  private readonly way: StatementSync
  private readonly wayPieces: StatementSync
  private readonly pieceById: StatementSync
  private readonly aliases = new Map<string, string>()

  constructor(preparedDirectory: string, private readonly family: 'roads' | 'railways' = 'railways') {
    this.database = new DatabaseSync(transportTopologyPath(preparedDirectory), { readOnly: true })
    try {
      if (this.database.prepare('PRAGMA user_version').get()?.user_version !== 2) {
        throw new Error('source transport topology is incomplete or has an unsupported schema')
      }
      this.pieces = this.database.prepare(`
        SELECT CAST(p.way_id AS TEXT) AS way_id, p.segment_idx,
               p.start_vertex, p.start_fraction, p.end_vertex, p.end_fraction
        FROM source_pieces p JOIN source_ways w ON w.osm_id = p.way_id
        WHERE p.square = ? AND w.family = ? ORDER BY p.way_id, p.segment_idx`)
      this.wayPieces = this.database.prepare(`
        SELECT segment_idx, square, start_vertex, start_fraction, end_vertex, end_fraction
        FROM source_pieces WHERE way_id = ? ORDER BY start_vertex, start_fraction`)
      this.pieceById = this.database.prepare(`
        SELECT square, start_vertex, start_fraction, end_vertex, end_fraction
        FROM source_pieces WHERE way_id = ? AND segment_idx = ?`)
      this.way = this.database.prepare("SELECT nodes_json FROM source_ways WHERE osm_id = ? AND family = ?")
      for (const row of this.database.prepare(`
        SELECT CAST(node_id AS TEXT) AS node_id, CAST(canonical_node AS TEXT) AS canonical_node
        FROM node_aliases WHERE family = ?`).iterate(this.family)) {
        this.aliases.set(row.node_id as string, row.canonical_node as string)
      }
    } catch (error) {
      this.database.close()
      throw error
    }
  }

  private wayNodes(id: string): SourceNode[] | undefined {
    const row = this.way.get(id, this.family)
    if (!row) return undefined
    const nodes = JSON.parse(row.nodes_json as string) as SourceNode[]
    for (const node of nodes) node[0] = this.aliases.get(node[0]) ?? node[0]
    return nodes
  }

  *trainRoutes(): Generator<SourceTrainRoute> {
    for (const row of this.database.prepare(`
      SELECT CAST(osm_id AS TEXT) AS id, members_json FROM source_train_routes ORDER BY osm_id`).iterate()) {
      yield this.orderTrainRoute(row.id as string, row.members_json as string)
    }
  }

  trainRoute(id: string): SourceTrainRoute | undefined {
    const row = this.database.prepare('SELECT members_json FROM source_train_routes WHERE osm_id = ?').get(id)
    return row ? this.orderTrainRoute(id, row.members_json as string) : undefined
  }

  private orderTrainRoute(id: string, membersJson: string): SourceTrainRoute {
    const ways: OrientedSourceRailWay[] = []
    const missingWays: string[] = []
    const unsupportedMembers: Array<[string, string]> = []
    for (const [type, wayId, role] of JSON.parse(membersJson) as RouteMember[]) {
      if (observationRoles.has(role)) continue
      if (type !== 'w' || !['', 'forward', 'backward'].includes(role)) {
        unsupportedMembers.push([`${type}${wayId}`, role])
        continue
      }
      const nodes = this.wayNodes(wayId)
      if (!nodes || nodes.length < 2 || nodes.some(node => node[1] === null)) {
        missingWays.push(`w${wayId}`)
        continue
      }
      ways.push({ id: wayId, role, nodes, reverse: false })
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

  passagePieces(passage: { way: string; from: number; to: number }): SourcePiecePassage[] {
    const nodes = this.wayNodes(passage.way)
    if (!nodes) throw new Error(`source way missing for passage ${passage.way}`)
    const distances = sourceNodeDistances(nodes)
    const lower = Math.min(passage.from, passage.to), upper = Math.max(passage.from, passage.to)
    if (!Number.isFinite(lower) || !Number.isFinite(upper) || lower < 0 || upper > distances.at(-1)! || lower === upper) {
      throw new Error(`invalid source passage ${passage.way}:${passage.from}..${passage.to}`)
    }
    const distanceAt = (vertex: number, fraction: number): number => {
      assertSourcePosition(nodes, passage.way, vertex, fraction)
      return fraction === 0 ? distances[vertex] :
        distances[vertex] + fraction * (distances[vertex + 1] - distances[vertex])
    }
    const pieces: SourcePiecePassage[] = []
    let coveredUntil = lower
    for (const raw of this.wayPieces.iterate(passage.way)) {
      const piece = raw as unknown as Omit<SourcePiece, 'way_id'> & { square: string }
      const start = distanceAt(piece.start_vertex, piece.start_fraction)
      const end = distanceAt(piece.end_vertex, piece.end_fraction)
      if (end <= start) throw new Error(`invalid source piece interval ${passage.way}:${piece.segment_idx}`)
      const from = Math.max(lower, start), to = Math.min(upper, end)
      if (to <= from) continue
      if (from !== coveredUntil) throw new Error(`source piece gap or overlap for passage ${passage.way} at ${coveredUntil}`)
      pieces.push({ segmentIndex: piece.segment_idx, square: piece.square, from, to })
      coveredUntil = to
    }
    if (coveredUntil !== upper) throw new Error(`source pieces do not cover passage ${passage.way} to ${upper}`)
    return passage.from < passage.to ? pieces : pieces.reverse().map(piece => ({ ...piece, from: piece.to, to: piece.from }))
  }

  pieceExtent(wayId: string, segmentIndex: number): { square: string; from: number; to: number } {
    const nodes = this.wayNodes(wayId)
    if (!nodes) throw new Error(`source way missing for piece ${wayId}:${segmentIndex}`)
    const distances = sourceNodeDistances(nodes)
    const row = this.pieceById.get(wayId, segmentIndex) as
      { square: string; start_vertex: number; start_fraction: number; end_vertex: number; end_fraction: number } | undefined
    if (!row) throw new Error(`source piece missing ${wayId}:${segmentIndex}`)
    const distanceAt = (vertex: number, fraction: number): number => {
      assertSourcePosition(nodes, wayId, vertex, fraction)
      return fraction === 0 ? distances[vertex] :
        distances[vertex] + fraction * (distances[vertex + 1] - distances[vertex])
    }
    const from = distanceAt(row.start_vertex, row.start_fraction)
    const to = distanceAt(row.end_vertex, row.end_fraction)
    if (to <= from) throw new Error(`invalid source piece interval ${wayId}:${segmentIndex}`)
    return { square: row.square, from, to }
  }

  *squares(): Generator<string> {
    // Seek each next owner instead of joining every acoustic piece just to deduplicate it.
    for (const row of this.database.prepare(`
      WITH RECURSIVE owners(square) AS (
        SELECT min(square) FROM source_pieces
        UNION ALL
        SELECT (SELECT min(square) FROM source_pieces WHERE square > owners.square)
        FROM owners WHERE square IS NOT NULL
      )
      SELECT square FROM owners WHERE square IS NOT NULL AND EXISTS (
        SELECT 1 FROM source_pieces p JOIN source_ways w ON w.osm_id=p.way_id
        WHERE p.square=owners.square AND w.family=?)`).iterate(this.family)) yield row.square as string
  }

  squareParentGeometries(square: string, wayIds: Iterable<string>): Map<string, {
    start: [number, number]; end: [number, number]; lengthM: number
  }> {
    const result = new Map<string, { start: [number, number]; end: [number, number]; lengthM: number }>()
    for (const wayId of new Set(wayIds)) {
      const nodes = this.wayNodes(wayId)
      if (!nodes) throw new Error(`source way missing ${wayId}`)
      const distances = sourceNodeDistances(nodes)
      const at = (vertex: number, fraction: number): { point: [number, number]; distance: number } => {
        assertSourcePosition(nodes, wayId, vertex, fraction)
        const start = nodes[vertex][1]!
        if (fraction === 0) return { point: start, distance: distances[vertex] }
        const end = nodes[vertex + 1][1]!
        return {
          point: [start[0] + (end[0] - start[0]) * fraction,
            normalizeLongitude(start[1] + normalizeLongitude(end[1] - start[1]) * fraction)],
          distance: distances[vertex] + (distances[vertex + 1] - distances[vertex]) * fraction,
        }
      }
      for (const raw of this.wayPieces.iterate(wayId)) {
        if (raw.square !== square) continue
        const piece = raw as unknown as SourcePiece
        const start = at(piece.start_vertex, piece.start_fraction)
        const end = at(piece.end_vertex, piece.end_fraction)
        if (end.distance <= start.distance) throw new Error(`invalid source piece interval ${wayId}:${piece.segment_idx}`)
        result.set(transportPieceKey(wayId, piece.segment_idx), {
          start: start.point, end: end.point, lengthM: end.distance - start.distance,
        })
      }
    }
    return result
  }

  squareWayPieces(square: string, wayIds: Iterable<string>): Map<string, SegmentEndpointKeys> {
    const result = new Map<string, SegmentEndpointKeys>()
    for (const wayId of new Set(wayIds)) {
      const nodes = this.wayNodes(wayId)
      if (!nodes) continue
      for (const raw of this.wayPieces.iterate(wayId)) {
        if (raw.square !== square) continue
        const piece = raw as unknown as SourcePiece
        result.set(transportPieceKey(wayId, piece.segment_idx), sourcePieceIdentity(nodes, wayId, piece))
      }
    }
    return result
  }

  squarePieceKeys(square: string): string[] {
    return Array.from(this.pieces.iterate(square, this.family), raw =>
      transportPieceKey(raw.way_id as string, raw.segment_idx as number))
  }

  squarePieces(square: string): Map<string, SegmentEndpointKeys> {
    const result = new Map<string, SegmentEndpointKeys>()
    let wayId = ''
    let nodes: SourceNode[] = []
    for (const raw of this.pieces.iterate(square, this.family)) {
      const piece = raw as unknown as SourcePiece
      if (piece.way_id !== wayId) {
        wayId = piece.way_id
        nodes = this.wayNodes(wayId)!
      }
      result.set(transportPieceKey(wayId, piece.segment_idx), sourcePieceIdentity(nodes, wayId, piece))
    }
    return result
  }

  [Symbol.dispose](): void {
    this.database.close()
  }
}
