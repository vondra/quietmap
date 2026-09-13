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

type SourceNode = [id: string, coordinates: [number, number] | null]

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
      this.way = this.database.prepare('SELECT nodes_json FROM source_ways WHERE osm_id = ?')
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
      const id = nodes[vertex][0]
      return `node:${this.aliases.get(id) ?? id}`
    }
    for (const raw of this.pieces.iterate(square)) {
      const piece = raw as unknown as SourcePiece
      if (piece.way_id !== wayId) {
        wayId = piece.way_id
        nodes = JSON.parse(this.way.get(wayId)!.nodes_json as string) as SourceNode[]
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
