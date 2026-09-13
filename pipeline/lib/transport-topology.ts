/** Read original OSM connectivity from the build database beside the prepared year. */

import { basename, dirname, join, resolve } from 'node:path'
import { DatabaseSync, type StatementSync } from 'node:sqlite'
import type { SegmentEndpointKeys } from './prepared-grid.js'

export const transportTopologyPath = (preparedDirectory: string): string => {
  const directory = resolve(preparedDirectory)
  return join(dirname(directory), `${basename(directory)}.transport.sqlite`)
}

export const transportPieceKey = (wayId: string, segmentIndex: number): string =>
  `${wayId}:${segmentIndex}`

export type SourceNode = [id: string, coordinates: [number, number] | null]

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

export class SourceTransportTopology implements Disposable {
  private readonly database: DatabaseSync
  private readonly pieces: StatementSync
  private readonly way: StatementSync
  private readonly aliases = new Map<string, string>()

  constructor(preparedDirectory: string) {
    this.database = new DatabaseSync(transportTopologyPath(preparedDirectory), { readOnly: true })
    try {
      if (this.database.prepare('PRAGMA user_version').get()?.user_version !== 2) {
        throw new Error('source transport topology is incomplete or has an unsupported schema')
      }
      this.pieces = this.database.prepare(`
        SELECT CAST(p.way_id AS TEXT) AS way_id, p.segment_idx,
               p.start_vertex, p.start_fraction, p.end_vertex, p.end_fraction
        FROM source_pieces p JOIN source_ways w ON w.osm_id = p.way_id
        WHERE p.square = ? AND w.family = 'railways' ORDER BY p.way_id, p.segment_idx`)
      this.way = this.database.prepare("SELECT nodes_json FROM source_ways WHERE osm_id = ? AND family = 'railways'")
      for (const row of this.database.prepare(`
        SELECT CAST(node_id AS TEXT) AS node_id, CAST(canonical_node AS TEXT) AS canonical_node
        FROM node_aliases WHERE family = 'railways'`).iterate()) {
        this.aliases.set(row.node_id as string, row.canonical_node as string)
      }
    } catch (error) {
      this.database.close()
      throw error
    }
  }

  private railWayNodes(id: string): SourceNode[] | undefined {
    const row = this.way.get(id)
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
      const nodes = this.railWayNodes(wayId)
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

  squarePieces(square: string): Map<string, SegmentEndpointKeys> {
    const result = new Map<string, SegmentEndpointKeys>()
    let wayId = ''
    let nodes: SourceNode[] = []
    const endpointKey = (vertex: number, fraction: number): string => {
      if (!Number.isInteger(vertex) || vertex < 0 || !nodes[vertex]?.[1] ||
          !Number.isFinite(fraction) || fraction < 0 || fraction >= 1 ||
          (fraction > 0 && !nodes[vertex + 1]?.[1])) {
        throw new Error(`invalid source position ${wayId}:${vertex}+${fraction}`)
      }
      if (fraction > 0) return `way:${wayId}:${vertex}+${fraction}`
      return `node:${nodes[vertex][0]}`
    }
    for (const raw of this.pieces.iterate(square)) {
      const piece = raw as unknown as SourcePiece
      if (piece.way_id !== wayId) {
        wayId = piece.way_id
        nodes = this.railWayNodes(wayId)!
      }
      result.set(transportPieceKey(wayId, piece.segment_idx), {
        startKey: endpointKey(piece.start_vertex, piece.start_fraction),
        endKey: endpointKey(piece.end_vertex, piece.end_fraction),
      })
    }
    return result
  }

  [Symbol.dispose](): void {
    this.database.close()
  }
}
