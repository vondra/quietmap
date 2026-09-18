/** Restore raw road parents from finalized carriageway children so the enrichment chain can run again. */

import { readFileSync } from 'node:fs'
import { DataType, RecordBatch, Schema, Table, makeVector, tableFromIPC, vectorFromArray, type Vector } from 'apache-arrow'
import { readArrowFileSchemaMetadata, ROADS_TIME_PROFILES_METADATA_KEY } from './roads-arrow.js'
import { finalizedChildrenBySourcePiece, replaceArrowFileDurably, sourceParentCoveredByChildren } from './transport-parent.js'
import type { SourceTransportTopology } from './transport-topology.js'

const GEOMETRY = ['start_gx', 'start_gy', 'end_gx', 'end_gy', 'length_m']
const REBUILT = [...GEOMETRY, 'segment_idx']
/** Every enrichment column a finalized file carries; a fresh extract has none of them. */
const ENRICHED_TRAFFIC = new Set(['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'traffic_estimated',
  'speed_taper', 'traffic_profile_id'])

type NumericColumn = Int32Array | Float32Array | Int16Array
const gatherRows = <T extends NumericColumn>(values: T, rows: Int32Array): T =>
  (values.constructor as unknown as { from(rows: Int32Array, pick: (row: number) => T[number]): T }).from(rows, row => values[row])

// The extractor stores one decimal, so any other value is the survivor of a parent whose subpixel sibling was dropped.
// Only its length can differ from the parent: a dropped sibling rounds both ends into one z30 cell, which is the
// parent's own end cell, so the lone survivor already spans the parent's exact endpoints.
const isExtractedLength = (length: number): boolean => Math.fround(Math.round(length * 10) / 10) === length

/** Caller holds the road enrichment lock; a raw file and a repeated call are left byte-identical. */
export function restoreRoadParentsForEnrichment(
  path: string, square: string, topology: SourceTransportTopology,
): { restored: false } | { rows: number; parents: number; restored: true } {
  // A fresh build sweeps every raw square through this step; the footer alone says there is nothing to do.
  if (readArrowFileSchemaMetadata(path).get('road_traffic_contract') !== '1') return { restored: false }
  const table = tableFromIPC(readFileSync(path))
  const ids = table.getChild('osm_id')!
  const kept = table.schema.fields.filter(field => !ENRICHED_TRAFFIC.has(field.name))
  const children = finalizedChildrenBySourcePiece(table, kept.map(field => field.name)
    .filter(name => !GEOMETRY.includes(name) && name !== 'source_id'), 'road', square)
  const firstRows = new Map(Array.from(children, ([key, rows]) => [key, rows[0]]))
  // Finalization drops a parent whose whole extent is one z30 cell; continuity still needs its incidence,
  // so it returns with the way-level attributes of the nearest surviving piece of the same way.
  const pieces = topology.squarePieces(square)
  const dropped = Array.from({ length: pieces.count }, (_, row) => pieces.key(row)).filter(key => !firstRows.has(key))
  const donors = new Map<string, Array<[segment: number, row: number]>>()
  if (dropped.length) for (const [key, row] of firstRows) {
    const [way, segment] = key.split(':')
    const pieces = donors.get(way)
    if (pieces) pieces.push([Number(segment), row])
    else donors.set(way, [[Number(segment), row]])
  }
  for (const key of dropped) {
    const [way, segment] = key.split(':')
    const donor = donors.get(way)?.reduce((best, next) =>
      Math.abs(next[0] - Number(segment)) < Math.abs(best[0] - Number(segment)) ? next : best)
    if (!donor) throw new Error(`no finalized piece of way ${way} can restore dropped parent ${key} in ${square}`)
    firstRows.set(key, donor[1])
  }
  const selected = Int32Array.from(firstRows.values())
  const geometry = Object.fromEntries(REBUILT.map(name => {
    return [name, gatherRows(table.getChild(name)!.toArray() as NumericColumn, selected)]
  }))
  const cut = [...firstRows].flatMap(([key, row], parent) =>
    children.get(key)?.length !== 1 || !isExtractedLength(geometry.length_m[parent]) ? [{ key, row, parent }] : [])
  for (const { key, row, parent } of cut) {
    const segment = Number(key.split(':')[1]), sourceRow = pieces.row(String(ids.get(row)), segment)
    if (sourceRow < 0) throw new Error(`source road parent missing ${key} in ${square}`)
    const covered = sourceParentCoveredByChildren(table, children.get(key) ?? [],
      pieces.geometry(sourceRow), 'road', `${key} in ${square}`)
    geometry.segment_idx[parent] = segment
    geometry.start_gx[parent] = covered.start[0]
    geometry.start_gy[parent] = covered.start[1]
    geometry.end_gx[parent] = covered.end[0]
    geometry.end_gy[parent] = covered.end[1]
    geometry.length_m[parent] = covered.lengthM
  }
  const columns: Record<string, Vector> = {}
  for (const field of kept) {
    const vector = table.getChild(field.name)!
    if (field.name in geometry) columns[field.name] = makeVector(geometry[field.name])
    else if (field.name === 'source_id') columns.source_id = makeVector(new Uint16Array(selected.length))
    else if ((DataType.isInt(field.type) || DataType.isFloat(field.type)) && vector.nullCount === 0) {
      columns[field.name] = makeVector(gatherRows(vector.toArray() as NumericColumn, selected))
    } else columns[field.name] = vectorFromArray(Array.from(selected, row => vector.get(row)), field.type)
  }
  const bare = new Table(columns)
  const metadata = new Map(table.schema.metadata)
  for (const key of ['road_traffic_contract', 'qm_blocks', ROADS_TIME_PROFILES_METADATA_KEY]) metadata.delete(key)
  const schema = new Schema(kept, metadata)
  replaceArrowFileDurably(path, new Table(schema, bare.batches.map(batch => new RecordBatch(schema, batch.data))))
  return { rows: table.numRows, parents: selected.length, restored: true }
}
