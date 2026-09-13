/** Synthetic source-topology databases for Arrow adapter fixtures; never reconstruct real source identities. */

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { tableFromIPC } from 'apache-arrow'
import { segmentGeometryReader } from './prepared-grid.js'
import { transportTopologyPath } from './transport-topology.js'

export type FixtureSourceNode = [id: string, coordinates: [number, number] | null]
export interface FixtureSourceWay {
  id: string
  family?: 'roads' | 'railways'
  nodes: FixtureSourceNode[]
}
export interface FixtureSourcePiece {
  way: string
  segment: number
  square: string
  start: [vertex: number, fraction: number]
  end: [vertex: number, fraction: number]
}

export function writeTransportFixture(
  prepared: string,
  ways: FixtureSourceWay[],
  pieces: FixtureSourcePiece[],
  aliases: Array<[family: string, node: string, canonical: string]> = [],
): void {
  using database = new DatabaseSync(transportTopologyPath(prepared))
  database.exec(readFileSync(new URL('../../engine/osm-extract/src/transport.sql', import.meta.url), 'utf8'))
  const insertWay = database.prepare('INSERT INTO source_ways VALUES (?, ?, ?)')
  for (const way of ways) insertWay.run(BigInt(way.id), way.family ?? 'railways', JSON.stringify(way.nodes))
  const insertPiece = database.prepare('INSERT INTO source_pieces VALUES (?, ?, ?, ?, ?, ?, ?)')
  for (const piece of pieces) insertPiece.run(BigInt(piece.way), piece.segment, piece.square, ...piece.start, ...piece.end)
  const insertAlias = database.prepare('INSERT INTO node_aliases VALUES (?, ?, ?)')
  for (const [family, node, canonical] of aliases) insertAlias.run(family, BigInt(node), BigInt(canonical))
  database.exec('PRAGMA user_version = 2')
}

/** Existing Arrow-only test cases declare synthetic shared nodes at their matching fixture endpoints. */
export function writeSyntheticRailTopology(prepared: string, squares: readonly string[]): void {
  const nodeIds = new Map<string, string>()
  const nodeId = (coordinateKey: string): string => {
    if (!nodeIds.has(coordinateKey)) nodeIds.set(coordinateKey, String(nodeIds.size + 1))
    return nodeIds.get(coordinateKey)!
  }
  const ways = new Map<string, FixtureSourceWay>()
  const pieces: FixtureSourcePiece[] = []
  for (const square of squares) {
    const table = tableFromIPC(readFileSync(resolve(prepared, square, 'railways.arrow')))
    const geometry = segmentGeometryReader(table)
    for (let index = 0; index < table.numRows; index++) {
      const id = String(table.getChild('osm_id')!.get(index))
      if (!ways.has(id)) ways.set(id, { id, nodes: [] })
      const way = ways.get(id)!
      const vertex = way.nodes.length
      const row = geometry.row(index), keys = geometry.endpointKeys(index)
      way.nodes.push([nodeId(keys.startKey), [row.startLat, row.startLon]], [nodeId(keys.endKey), [row.endLat, row.endLon]])
      pieces.push({ way: id, segment: table.getChild('segment_idx')!.get(index) as number, square,
        start: [vertex, 0], end: [vertex + 1, 0] })
    }
  }
  writeTransportFixture(prepared, [...ways.values()], pieces)
}
