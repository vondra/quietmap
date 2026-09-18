/** `<year>.railway-ways.arrow` (an Arrow IPC stream of bounded batches) and `<year>.train-routes.arrow`, read in bounded memory. */

import fs, { readFileSync } from 'node:fs'
import { basename, dirname, join, resolve } from 'node:path'
import { RecordBatchReader, tableFromIPC, type RecordBatch, type Table } from 'apache-arrow'

export const railwayGlobalPath = (preparedDirectory: string, kind: 'railway-ways' | 'train-routes'): string => {
  const directory = resolve(preparedDirectory)
  return join(dirname(directory), `${basename(directory)}.${kind}.arrow`)
}

export type SourceNode = [id: string, coordinates: [number, number] | null]

const RAILWAY_WAYS_READ_BYTES = 16 << 20

/** Record batches of an Arrow IPC stream through sequential reads of at most `readBytes`, one batch alive
 *  at a time: Node cannot hold the world railway-ways file (over 2 GiB) as one buffer. */
export function* arrowStreamBatches(path: string, contractKey: string, readBytes = RAILWAY_WAYS_READ_BYTES): Generator<RecordBatch> {
  const descriptor = fs.openSync(path, 'r')
  try {
    const reader = RecordBatchReader.from(function* (): Generator<Uint8Array> {
      for (;;) {
        // A fresh buffer per read: decoded batches are views into the chunks they came from.
        const chunk = Buffer.allocUnsafe(readBytes), read = fs.readSync(descriptor, chunk, 0, readBytes, null)
        if (read === 0) return
        yield chunk.subarray(0, read)
      }
    }()).open()
    if (reader.schema.metadata.get(contractKey) !== '1') throw new Error(`${path}: unsupported ${contractKey}`)
    yield* reader
  } finally { fs.closeSync(descriptor) }
}

export function contractTable(path: string, contractKey: string, bytes: Buffer): Table {
  const table = tableFromIPC(bytes)
  if (table.schema.metadata.get(contractKey) !== '1') throw new Error(`${path}: unsupported ${contractKey}`)
  if (table.batches.length > 1) throw new Error(`${path}: expected one record batch`)
  return table
}

export function firstRowAtOrAfter(ids: BigInt64Array, id: bigint): number {
  let low = 0, high = ids.length
  while (low < high) {
    const middle = (low + high) >>> 1
    if (ids[middle] < id) low = middle + 1
    else high = middle
  }
  return low
}

type NumericColumn = BigInt64Array | Int32Array | Uint32Array | Float64Array
interface ListColumn<T> { offsets: Int32Array; values: T; valid(row: number): boolean }

function listColumn<T extends NumericColumn>(batch: RecordBatch, name: string): ListColumn<T> {
  const data = batch.getChild(name)!.data[0]
  return { offsets: data.valueOffsets as Int32Array, values: data.children[0].values as T, valid: row => data.getValid(row) }
}

/** Owned copies of the listed rows' values; `keptOffsets` holds an empty range for every other row. */
function copiedListValues<T extends NumericColumn>(list: ListColumn<T>, keptOffsets: Uint32Array): T {
  const copy = new (list.values.constructor as new (length: number) => T)(keptOffsets.at(-1)!)
  for (let row = 0; row + 1 < keptOffsets.length; row++) {
    if (keptOffsets[row + 1] === keptOffsets[row]) continue
    copy.set(list.values.subarray(list.offsets[row], list.offsets[row + 1]) as never, keptOffsets[row])
  }
  return copy
}

/** What one record batch of `railway-ways` leaves in memory: every way's id, length and squares, and the
 *  node chain of complete train-route member ways only (the only chains any caller reads). */
class RailwayWaysBatch {
  readonly wayIds: BigInt64Array
  private readonly lengthM: Float64Array
  private readonly squareOffsets: Uint32Array
  private readonly squareKeys: Uint32Array
  private readonly chainOffsets: Uint32Array
  private readonly nodeIds: BigInt64Array
  private readonly latitudesE7: Int32Array
  private readonly longitudesE7: Int32Array
  private readonly nodeM: Float64Array

  constructor(batch: RecordBatch, trainRouteMemberWays: ReadonlySet<bigint>) {
    this.wayIds = (batch.getChild('way_id')!.toArray() as BigInt64Array).slice()
    const metres = listColumn<Float64Array>(batch, 'node_m'), squares = listColumn<Uint32Array>(batch, 'square_key')
    this.lengthM = new Float64Array(batch.numRows).fill(Number.NaN)
    this.squareOffsets = new Uint32Array(batch.numRows + 1)
    this.chainOffsets = new Uint32Array(batch.numRows + 1)
    for (let row = 0; row < batch.numRows; row++) {
      const nodes = metres.valid(row) ? metres.offsets[row + 1] - metres.offsets[row] : 0
      if (nodes) this.lengthM[row] = metres.values[metres.offsets[row + 1] - 1]
      this.squareOffsets[row + 1] = this.squareOffsets[row] + squares.offsets[row + 1] - squares.offsets[row]
      this.chainOffsets[row + 1] = this.chainOffsets[row] + (trainRouteMemberWays.has(this.wayIds[row]) ? nodes : 0)
    }
    this.squareKeys = copiedListValues(squares, this.squareOffsets)
    this.nodeM = copiedListValues(metres, this.chainOffsets)
    this.nodeIds = copiedListValues(listColumn<BigInt64Array>(batch, 'node_id'), this.chainOffsets)
    this.latitudesE7 = copiedListValues(listColumn<Int32Array>(batch, 'lat_e7'), this.chainOffsets)
    this.longitudesE7 = copiedListValues(listColumn<Int32Array>(batch, 'lon_e7'), this.chainOffsets)
  }

  /** NaN when the way is incomplete. */
  wayLengthM(row: number): number { return this.lengthM[row] }

  squares(row: number): string[] {
    return Array.from(this.squareKeys.subarray(this.squareOffsets[row], this.squareOffsets[row + 1]),
      key => `z9/${key % 512}/${Math.floor(key / 512)}`)
  }

  /** Null when the way is incomplete (a complete way has every coordinate). */
  trainRouteMemberChain(row: number): { nodes: SourceNode[]; distances: number[] } | null {
    const [first, end] = [this.chainOffsets[row], this.chainOffsets[row + 1]]
    if (first === end) return null
    const nodes: SourceNode[] = []
    for (let node = first; node < end; node++) {
      nodes.push([String(this.nodeIds[node]), [this.latitudesE7[node] / 1e7, this.longitudesE7[node] / 1e7]])
    }
    return { nodes, distances: Array.from(this.nodeM.subarray(first, end)) }
  }
}

const WAY_MEMBER_KIND = 'w'.charCodeAt(0)

class RailwayGlobals {
  private readonly wayBatches: RailwayWaysBatch[] = []
  readonly routes: Table

  constructor(preparedDirectory: string) {
    const routesPath = railwayGlobalPath(preparedDirectory, 'train-routes')
    this.routes = contractTable(routesPath, 'train_routes_contract', readFileSync(routesPath))
    const trainRouteMemberWays = new Set<bigint>()
    const memberIds = this.routes.getChild('member_id')!.data[0]?.children[0].values as BigInt64Array | undefined
    const memberKinds = this.routes.getChild('member_kind')!.data[0]?.children[0].values as Uint8Array | undefined
    memberIds?.forEach((id, member) => { if (memberKinds![member] === WAY_MEMBER_KIND) trainRouteMemberWays.add(id) })
    for (const batch of arrowStreamBatches(railwayGlobalPath(preparedDirectory, 'railway-ways'), 'railway_ways_contract')) {
      if (batch.numRows) this.wayBatches.push(new RailwayWaysBatch(batch, trainRouteMemberWays))
    }
  }

  /** Batches are ascending by way id, as are the ways inside each. */
  way(wayId: string): { batch: RailwayWaysBatch; row: number } | null {
    const id = BigInt(wayId)
    const batch = this.wayBatches.find(candidate => candidate.wayIds.at(-1)! >= id)
    const row = batch ? firstRowAtOrAfter(batch.wayIds, id) : -1
    return batch && batch.wayIds[row] === id ? { batch, row } : null
  }
}

// Several topologies of one process (walk, passage writer) share the globals.
const railwayGlobalsCache: { prepared?: string; globals?: RailwayGlobals } = {}

/** Loaded on first use, and only by processes that route services. */
export function railwayGlobals(preparedDirectory: string): RailwayGlobals {
  if (railwayGlobalsCache.prepared !== preparedDirectory || !railwayGlobalsCache.globals) {
    railwayGlobalsCache.globals = new RailwayGlobals(preparedDirectory)
    railwayGlobalsCache.prepared = preparedDirectory
  }
  return railwayGlobalsCache.globals
}
