/** Restore original railway parents before repeat enrichment; sidecar evidence remains authoritative. */

import { closeSync, fsyncSync, openSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { isDeepStrictEqual } from 'node:util'
import { Float32, Int32, RecordBatch, Schema, Table, Uint16, tableFromIPC, tableToIPC, vectorFromArray, type Vector } from 'apache-arrow'
import { lonLatToGrid } from './prepared-grid.js'
import { railTrafficSidecarPath, RAIL_TRAFFIC_SIDECAR_VERSION } from './rail-traffic-store.js'
import { SourceTransportTopology, transportPieceKey } from './transport-topology.js'

const GEOMETRY = new Set(['start_gx', 'start_gy', 'end_gx', 'end_gy', 'length_m'])
const TRAFFIC = new Set([
  'trains_passenger_day', 'trains_passenger_evening', 'trains_passenger_night',
  'trains_freight_day', 'trains_freight_evening', 'trains_freight_night',
  'passenger_status', 'freight_status', 'passenger_source_id', 'freight_source_id',
  'passenger_matching', 'freight_matching', 'source_id',
  'trains_passenger', 'trains_freight', 'parallel_divisor',
])

/** Caller holds the existing railway enrichment lock; a raw file is deliberately unpublishable until finalization. */
export function restoreRailwayParentsForEnrichment(
  path: string, prepared: string, square: string, topology: SourceTransportTopology,
): Table {
  const table = tableFromIPC(readFileSync(path))
  if (table.schema.metadata.get('rail_traffic_contract') !== '1') return table
  // Never discard finalized evidence if its authoritative preparation input is absent.
  using sidecar = new DatabaseSync(railTrafficSidecarPath(prepared), { readOnly: true })
  if (sidecar.prepare('PRAGMA user_version').get()?.user_version !== RAIL_TRAFFIC_SIDECAR_VERSION) {
    throw new Error('railway parent restoration requires the retained traffic sidecar')
  }
  const ids = table.getChild('osm_id')!, indices = table.getChild('segment_idx')!
  if (!ids || !indices) throw new Error('railway parent restoration requires source identity columns')
  const fields = table.schema.fields.filter(field => !TRAFFIC.has(field.name) && !GEOMETRY.has(field.name))
  const attributes = fields.map(field => table.getChild(field.name)!)
  const parents = new Map<string, number>()
  const childRows = new Map<string, number[]>()
  const rows = table.numRows
  for (let row = 0; row < rows; row++) {
    const key = transportPieceKey(String(ids.get(row)), indices.get(row) as number)
    const children = childRows.get(key) ?? []
    children.push(row)
    childRows.set(key, children)
    const first = parents.get(key)
    if (first === undefined) parents.set(key, row)
    else for (let column = 0; column < attributes.length; column++) {
      if (!isDeepStrictEqual(attributes[column].get(first), attributes[column].get(row))) {
        throw new Error(`railway children disagree on ${fields[column].name} for ${key} in ${square}`)
      }
    }
  }
  const geometry = topology.squareParentGeometries(square, Array.from(parents.values(), row => String(ids.get(row))))
  const selected = [...parents.values()]
  const columns: Record<string, Vector> = {}
  fields.forEach((field, index) => {
    columns[field.name] = vectorFromArray(selected.map(row => attributes[index].get(row)), field.type)
  })
  const starts: Array<[number, number]> = [], ends: Array<[number, number]> = [], lengths: number[] = []
  for (const key of parents.keys()) {
    const parent = geometry.get(key)
    if (!parent) throw new Error(`source railway parent missing ${key} in ${square}`)
    const start = lonLatToGrid(parent.start[1], parent.start[0])
    const end = lonLatToGrid(parent.end[1], parent.end[0])
    const children = childRows.get(key)!
    const links = new Map<string, string>()
    for (const child of children) {
      const from = `${table.getChild('start_gx')!.get(child)},${table.getChild('start_gy')!.get(child)}`
      const to = `${table.getChild('end_gx')!.get(child)},${table.getChild('end_gy')!.get(child)}`
      if (links.has(from)) throw new Error(`overlapping railway children for ${key} in ${square}`)
      links.set(from, to)
    }
    let endpoint: string | undefined = start.join(',')
    for (let visit = 0; visit < children.length; visit++) {
      const next: string | undefined = links.get(endpoint!)
      links.delete(endpoint!)
      endpoint = next
    }
    if (endpoint !== end.join(',') || links.size !== 0) {
      throw new Error(`railway children do not cover source parent ${key} in ${square}`)
    }
    starts.push(start)
    ends.push(end)
    // Unsplit rows retain the exact extractor value. Merged parents recover the
    // Float32 then one-decimal ties-to-even encoding in osm-extract/src/spill.rs.
    const tenths = Math.fround(parent.lengthM) * 10
    const lower = Math.floor(tenths), fraction = tenths - lower
    const rounded = lower + Number(fraction > 0.5 || (fraction === 0.5 && lower % 2 !== 0))
    lengths.push(children.length === 1
      ? table.getChild('length_m')!.get(parents.get(key)!) as number : rounded / 10)
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
  const temporary = `${path}.tmp`
  const fd = openSync(temporary, 'w')
  try {
    writeFileSync(fd, Buffer.from(tableToIPC(restored, 'file')))
    fsyncSync(fd)
  } finally { closeSync(fd) }
  renameSync(temporary, path)
  const directory = openSync(dirname(path), 'r')
  try { fsyncSync(directory) } finally { closeSync(directory) }
  return restored
}
