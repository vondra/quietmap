/** Synthetic source-topology files for Arrow adapter fixtures; never reconstruct real source identities. */

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import {
  Field, Float32, Float64, Int16, Int32, Int64, List, Schema, Table, Uint16, Uint32, Uint8, Utf8,
  tableFromIPC, tableToIPC, vectorFromArray, type DataType,
} from 'apache-arrow'
import { lonLatToGrid, normalizeLongitude, segmentGeometryReader } from './prepared-grid.js'
import { flatDist } from './spatial.js'
import { railwayGlobalPath } from './railway-globals.js'
import { sourcePiecesPath, type TransportFamily } from './transport-topology.js'

export type FixtureSourceNode = [id: string, coordinates: [number, number] | null]
export interface FixtureSourceWay {
  id: string
  family?: TransportFamily
  nodes: FixtureSourceNode[]
}
export interface FixtureSourcePiece {
  way: string
  segment: number
  square: string
  start: [vertex: number, fraction: number]
  end: [vertex: number, fraction: number]
}
export interface FixtureTrainRoute {
  id: string
  members: ReadonlyArray<[type: string, id: string, role: string]>
}

/** The extractor owns real metres; fixtures and hand-built ways only need self-consistent ones. */
export function fixtureNodeDistances(nodes: readonly FixtureSourceNode[]): number[] | null {
  if (nodes.length < 2 || nodes.some(node => node[1] === null)) return null
  const distances = [0]
  for (let index = 1; index < nodes.length; index++) {
    distances.push(distances.at(-1)! + flatDist(...nodes[index - 1][1]!, ...nodes[index][1]!))
  }
  return distances
}

function writeArrow(path: string, contractKey: string, extra: Record<string, string>,
  columns: Array<[name: string, type: DataType, values: unknown[]]>, streamRowsPerBatch?: number): void {
  const rows = Math.max(columns[0][2].length, 1), batches = []
  for (let first = 0; first < rows; first += streamRowsPerBatch ?? rows) {
    batches.push(new Table(Object.fromEntries(columns.map(([name, type, values]) =>
      [name, vectorFromArray(values.slice(first, first + (streamRowsPerBatch ?? rows)), type)]))))
  }
  const schema = new Schema(batches[0].schema.fields, new Map([[contractKey, '1'], ...Object.entries(extra)]))
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, tableToIPC(new Table(schema, batches.flatMap(table => table.batches)), streamRowsPerBatch ? 'stream' : 'file'))
}

const listOf = (type: DataType): List => new List(new Field('item', type, true))
const byBigInt = (a: string, b: string): number => (BigInt(a) < BigInt(b) ? -1 : Number(BigInt(a) > BigInt(b)))

export function writeTransportFixture(
  prepared: string,
  ways: FixtureSourceWay[],
  pieces: FixtureSourcePiece[],
  aliases: Array<[family: string, node: string, canonical: string]> = [],
  trainRoutes: readonly FixtureTrainRoute[] = [],
): void {
  const wayById = new Map(ways.map(way => [way.id, way]))
  // Real node ids are Int64; a symbolic fixture name gets a stable synthetic id.
  const symbolic = new Map<string, bigint>()
  const canonical = (family: string, node: string): bigint => {
    const id = aliases.find(alias => alias[0] === family && alias[1] === node)?.[2] ?? node
    if (/^\d+$/.test(id)) return BigInt(id)
    if (!symbolic.has(id)) symbolic.set(id, 9_000_000_000_000_000n + BigInt(symbolic.size))
    return symbolic.get(id)!
  }
  const groups = new Map<string, FixtureSourcePiece[]>()
  for (const piece of pieces) {
    const group = `${wayById.get(piece.way)!.family ?? 'railways'}\n${piece.square}`
    groups.set(group, [...groups.get(group) ?? [], piece])
  }
  for (const [group, members] of groups) {
    const [family, square] = group.split('\n') as [TransportFamily, string]
    members.sort((a, b) => byBigInt(a.way, b.way) || a.segment - b.segment)
    const rows = members.map(piece => {
      const nodes = wayById.get(piece.way)!.nodes, distances = fixtureNodeDistances(nodes)
      const at = ([vertex, fraction]: [number, number]) => {
        const start = nodes[vertex][1]!, end = fraction > 0 ? nodes[vertex + 1][1]! : start
        return {
          cell: lonLatToGrid(normalizeLongitude(start[1] + normalizeLongitude(end[1] - start[1]) * fraction),
            start[0] + (end[0] - start[0]) * fraction),
          metres: distances && distances[vertex] + (fraction > 0 ? fraction * (distances[vertex + 1] - distances[vertex]) : 0),
        }
      }
      const start = at(piece.start), end = at(piece.end)
      const length = start.metres === null ? 0 : Math.round((end.metres! - start.metres) * 10) / 10
      return { piece, nodes, start, end, length, first: distances && distances[piece.start[0]] }
    })
    const columns: Array<[string, DataType, unknown[]]> = [
      ['way_id', new Int64(), rows.map(row => BigInt(row.piece.way))],
      ['segment_idx', new Int16(), rows.map(row => row.piece.segment)],
      ['start_vertex', new Uint16(), rows.map(row => row.piece.start[0])],
      ['start_fraction', new Float64(), rows.map(row => row.piece.start[1])],
      ['end_vertex', new Uint16(), rows.map(row => row.piece.end[0])],
      ['end_fraction', new Float64(), rows.map(row => row.piece.end[1])],
      ['start_node', new Int64(), rows.map(row => canonical(family, row.nodes[row.piece.start[0]][0]))],
      ['end_node', new Int64(), rows.map(row => canonical(family, row.nodes[row.piece.end[0]][0]))],
      ['start_gx', new Int32(), rows.map(row => row.start.cell[0])],
      ['start_gy', new Int32(), rows.map(row => row.start.cell[1])],
      ['end_gx', new Int32(), rows.map(row => row.end.cell[0])],
      ['end_gy', new Int32(), rows.map(row => row.end.cell[1])],
      ['length_m', new Float32(), rows.map(row => row.length)],
    ]
    // The clipped chain columns are read by the native finalizer only and are left out here.
    if (family === 'railways') {
      columns.push(['first_vertex_m', new Float64(), rows.map(row => row.first)],
        ['from_m', new Float64(), rows.map(row => row.start.metres)], ['to_m', new Float64(), rows.map(row => row.end.metres)])
    }
    writeArrow(sourcePiecesPath(prepared, square, family), 'transport_pieces_contract', { family }, columns)
  }

  const railways = ways.filter(way => (way.family ?? 'railways') === 'railways').sort((a, b) => byBigInt(a.id, b.id))
  const squareKey = (square: string): number => {
    const [, x, y] = square.split('/').map(Number)
    return y * 512 + x
  }
  writeArrow(railwayGlobalPath(prepared, 'railway-ways'), 'railway_ways_contract', {}, [
    ['way_id', new Int64(), railways.map(way => BigInt(way.id))],
    ['node_id', listOf(new Int64()), railways.map(way => way.nodes.map(node => canonical('railways', node[0])))],
    ['lat_e7', listOf(new Int32()), railways.map(way => way.nodes.map(node => node[1] && Math.round(node[1][0] * 1e7)))],
    ['lon_e7', listOf(new Int32()), railways.map(way => way.nodes.map(node => node[1] && Math.round(node[1][1] * 1e7)))],
    ['node_m', listOf(new Float64()), railways.map(way => fixtureNodeDistances(way.nodes))],
    ['square_key', listOf(new Uint32()), railways.map(way =>
      [...new Set(pieces.filter(piece => piece.way === way.id).map(piece => squareKey(piece.square)))].sort((a, b) => a - b))],
  ], 2) // The extractor streams size-bounded batches; two ways per batch makes fixtures cross a batch boundary.
  const routes = [...trainRoutes].sort((a, b) => byBigInt(a.id, b.id))
  writeArrow(railwayGlobalPath(prepared, 'train-routes'), 'train_routes_contract', {}, [
    ['osm_id', new Int64(), routes.map(route => BigInt(route.id))],
    ['member_kind', listOf(new Uint8()), routes.map(route => route.members.map(member => member[0].charCodeAt(0)))],
    ['member_id', listOf(new Int64()), routes.map(route => route.members.map(member => BigInt(member[1])))],
    ['member_role', listOf(new Utf8()), routes.map(route => route.members.map(member => member[2]))],
  ])
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
