/** Flat column access and per-batch column rebuild for roads.arrow writers: no per-row `Vector.get`, no `makeTable`. */

import { DataType, Field, RecordBatch, Schema, Struct, Table, makeData, makeVector, type Data, type Vector } from 'apache-arrow'
import { bakedRoadCountryReader, segmentGeometryReader, type SegmentGeometry } from './prepared-grid.js'
import type { RoadRow } from './roads-arrow.js'

export type NumericColumn = Float64Array | Uint16Array | Uint8Array

/** A RoadRow over flat column arrays: a matcher pays only for the fields it reads. */
export class ColumnBackedRoadRow implements RoadRow {
  private decodedGeometry: SegmentGeometry | undefined

  constructor(private readonly columns: RoadRowColumns, private readonly index: number, public existingSourceId: number) {}

  private get geometry(): SegmentGeometry { return this.decodedGeometry ??= this.columns.geometry.row(this.index) }
  get startLat(): number { return this.geometry.startLat }
  get startLon(): number { return this.geometry.startLon }
  get endLat(): number { return this.geometry.endLat }
  get endLon(): number { return this.geometry.endLon }
  get midLat(): number { return this.geometry.midLat }
  get midLon(): number { return this.geometry.midLon }
  get ref(): string | null { return (this.columns.ref?.get(this.index) as string | null) ?? null }
  get name(): string | null { return (this.columns.name?.get(this.index) as string | null) ?? null }
  get osmId(): number | null { return this.columns.osmId ? Number(this.columns.osmId.get(this.index)) : null }
  get roadClass(): number { return this.columns.roadClass[this.index] }
  get countryCode(): number { return (this.columns.countries ??= bakedRoadCountryReader(this.columns.table)).codeAt(this.index) }
  get oneway(): number { return (this.columns.oneway?.get(this.index) as number) ?? 0 }
}

export interface RoadRowColumns {
  table: Table
  geometry: ReturnType<typeof segmentGeometryReader>
  ref: Vector | null
  name: Vector | null
  osmId: Vector | null
  oneway: Vector | null
  roadClass: Uint8Array
  countries: ReturnType<typeof bakedRoadCountryReader> | null
}

export function roadRowColumns(table: Table): RoadRowColumns {
  const roadClass = new Uint8Array(table.numRows)
  seedColumn(roadClass, table.getChild('road_class'), () => 5)
  return { table, geometry: segmentGeometryReader(table), ref: table.getChild('ref'), name: table.getChild('name'),
    osmId: table.getChild('osm_id'), oneway: table.getChild('oneway'), roadClass, countries: null }
}

/** Copy a stored column into a writable array; `Vector.get` searches the batch list on every read, so only null-bearing columns take it. */
export function seedColumn(target: NumericColumn, existing: Vector | null, fallbackAt: (index: number) => number): void {
  if (existing && existing.nullCount === 0) target.set(existing.toArray() as ArrayLike<number>)
  else for (let index = 0; index < target.length; index++) target[index] = (existing?.get(index) as number | null) ?? fallbackAt(index)
}

export type ColumnChunkOfBatch = (startRow: number, endRow: number, batch: RecordBatch) => Data

export const numericColumnChunk = (values: NumericColumn): ColumnChunkOfBatch =>
  (startRow, endRow) => makeVector(values.subarray(startRow, endRow)).data[0]

/**
 * The input table with the named columns rebuilt batch by batch and moved to the end, under new metadata.
 * `makeTable` spends seconds re-distributing the chunks of a 1024-batch square; it also ordered
 * rebuilt columns last and dropped empty batches, which the stored files already reflect.
 */
export function tableWithRebuiltColumns(
  table: Table, metadata: Map<string, string>, rebuiltColumns: ReadonlyMap<string, ColumnChunkOfBatch>,
): Table {
  const kept = table.schema.fields.flatMap((field, position) => (rebuiltColumns.has(field.name) ? [] : [position]))
  const batches = table.batches.filter(batch => batch.numRows > 0)
  let startRow = 0
  const children = batches.map(batch => {
    const endRow = startRow + batch.numRows
    const chunks = [...kept.map(position => batch.data.children[position]),
      ...[...rebuiltColumns.values()].map(chunkOf => chunkOf(startRow, endRow, batch))]
    startRow = endRow
    return chunks
  })
  const fields = [...kept.map(position => table.schema.fields[position]),
    // Flags of a column new to the file, as `makeTable` declared them and every stored file carries them: a built
    // string column nullable, a typed-array column not. `withArrowWrite` restores the stored flag of an existing column.
    ...[...rebuiltColumns.keys()].map((name, column) => {
      const { type } = children[0][kept.length + column]
      return new Field(name, type, DataType.isUtf8(type))
    })]
  const schema = new Schema(fields, metadata)
  return new Table(schema, batches.map((batch, position) => new RecordBatch(schema, makeData({
    type: new Struct(fields), length: batch.numRows, nullCount: 0, children: children[position] }))))
}
