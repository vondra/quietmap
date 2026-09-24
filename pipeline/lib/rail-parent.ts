/** Restore original railway parents before repeat enrichment; the square's interval files remain authoritative. */

import { readFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { Float32, Int32, RecordBatch, Schema, Table, Uint16, tableFromIPC, vectorFromArray, type Vector } from 'apache-arrow'
import { squareHasRailIntervalFiles } from './rail-traffic-store.js'
import { finalizedChildrenBySourcePiece, replaceArrowFileDurably, sourceParentCoveredByChildren } from './transport-parent.js'
import type { SourceTransportTopology } from './transport-topology.js'

const GEOMETRY = new Set(['start_gx', 'start_gy', 'end_gx', 'end_gy', 'length_m'])
const TRAFFIC = new Set([
  'trains_passenger_day', 'trains_passenger_evening', 'trains_passenger_night',
  'trains_freight_day', 'trains_freight_evening', 'trains_freight_night',
  'passenger_status', 'freight_status', 'passenger_source_id', 'freight_source_id',
  'passenger_matching', 'freight_matching', 'source_id',
])

/** Caller holds the existing railway enrichment lock; a raw file is deliberately unpublishable until finalization. */
export function restoreRailwayParentsForEnrichment(
  path: string, square: string, topology: SourceTransportTopology,
): Table {
  const table = tableFromIPC(readFileSync(path))
  if (table.schema.metadata.get('rail_traffic_contract') !== '1') return table
  // Never discard finalized evidence whose interval files are gone: the final rows would be its only copy.
  // A square that only ever received class priors (source 0) has no files and nothing to lose.
  // Presence only, not one file per consumed country: nothing deletes a single country's file, and a
  // partial copy of a prepared tree is the copier's checksummed inventory to catch, not a restore gate.
  const evidenced = ['passenger_source_id', 'freight_source_id'].some(name =>
    (table.getChild(name)!.toArray() as Uint16Array).some(source => source !== 0))
  if (evidenced && !squareHasRailIntervalFiles(dirname(path))) {
    throw new Error(`railway parent restoration requires the retained interval files of ${square}`)
  }
  const fields = table.schema.fields.filter(field => !TRAFFIC.has(field.name) && !GEOMETRY.has(field.name))
  const childRows = finalizedChildrenBySourcePiece(table, fields.map(field => field.name), 'railway', square)
  const attributes = fields.map(field => table.getChild(field.name)!)
  const ids = table.getChild('osm_id')!
  const selected = Array.from(childRows.values(), children => children[0])
  const pieces = topology.squarePieces(square)
  const columns: Record<string, Vector> = {}
  fields.forEach((field, index) => {
    columns[field.name] = vectorFromArray(selected.map(row => attributes[index].get(row)), field.type)
  })
  const starts: Array<[number, number]> = [], ends: Array<[number, number]> = [], lengths: number[] = []
  for (const [key, children] of childRows) {
    const sourceRow = pieces.row(String(ids.get(children[0])), Number(key.split(':')[1]))
    if (sourceRow < 0) throw new Error(`source railway parent missing ${key} in ${square}`)
    const covered = sourceParentCoveredByChildren(table, children, pieces.geometry(sourceRow), 'railway', `${key} in ${square}`)
    starts.push(covered.start)
    ends.push(covered.end)
    lengths.push(covered.lengthM)
  }
  columns.start_gx = vectorFromArray(starts.map(point => point[0]), new Int32())
  columns.start_gy = vectorFromArray(starts.map(point => point[1]), new Int32())
  columns.end_gx = vectorFromArray(ends.map(point => point[0]), new Int32())
  columns.end_gy = vectorFromArray(ends.map(point => point[1]), new Int32())
  columns.length_m = vectorFromArray(lengths, new Float32())
  columns.source_id = vectorFromArray(selected.map(() => 0), new Uint16())
  const ordered = Object.fromEntries(table.schema.fields.filter(field => !TRAFFIC.has(field.name))
    .map(field => [field.name, columns[field.name]]))
  const bare = new Table({ ...ordered, source_id: columns.source_id })
  const metadata = new Map(table.schema.metadata)
  metadata.delete('rail_traffic_contract')
  metadata.delete('qm_blocks')
  const schema = new Schema(bare.schema.fields.map(field =>
    table.schema.fields.find(original => original.name === field.name) ?? field), metadata)
  const restored = new Table(schema, bare.batches.map(batch => new RecordBatch(schema, batch.data)))
  replaceArrowFileDurably(path, restored)
  return restored
}
