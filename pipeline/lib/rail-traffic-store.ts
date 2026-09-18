/** Clipped and whole-piece railway evidence as per-square, per-country Arrow files (`rail_traffic_contract=1` input). */

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { Schema, Table, makeVector, tableFromIPC } from 'apache-arrow'
import { withFileLock } from './provenance.js'
import type { RailServiceMatching, RailTrafficStatus } from './rail-passage.js'
import { replaceArrowFileDurably } from './transport-parent.js'

const INTERVAL_FILE = /^rail-intervals\.([A-Z]{2})\.arrow$/
const railIntervalsPath = (squareDirectory: string, countryIso: string): string =>
  join(squareDirectory, `rail-intervals.${countryIso}.arrow`)
const railQuarantinePath = (squareDirectory: string, countryIso: string): string =>
  join(squareDirectory, `rail-quarantine.${countryIso}.arrow`)

/** Restoration asks only whether a square's evidence is present, never how much of it. */
export function squareHasRailIntervalFiles(squareDirectory: string): boolean {
  return readdirSync(squareDirectory).some(name => INTERVAL_FILE.test(name))
}

export const RAIL_STATUS = { unknown: 0, known: 1, estimated: 2 } as const

export function railStatusCode(status: RailTrafficStatus): number {
  return RAIL_STATUS[status]
}

export function railMatchingMask(matching: RailServiceMatching | undefined): number {
  if (matching === 'relation_estimated') return 1
  if (matching === 'graph_estimated') return 2
  return 0
}

/** Count>0 defaults to estimated; explicit zero without status is unknown, not known-zero. */
export function inferredRailStatus(count: number, explicit?: RailTrafficStatus): RailTrafficStatus {
  if (explicit) return explicit
  return count > 0 ? 'estimated' : 'unknown'
}

export interface RailPieceIdentity { osmId: number; segmentIndex: number }

export interface RailSquareInterval extends RailPieceIdentity {
  fromM: number
  toM: number
  /** Visit ordinal within a source snapshot, disjoint across its services. */
  occurrence: number
  sourceId: number
  passenger: number
  freight: number
  passengerStatus: number
  freightStatus: number
  matching: number
}

export interface RailIntervalRow extends RailSquareInterval {
  square: string
  countryIso: string
}

type ColumnValues = BigInt64Array | Int16Array | Float64Array | Uint16Array | Uint8Array
/** Each file schema has its own contract key, so a change of one is detected in that file. */
interface StoredSchema { contractKey: string; columns: Record<string, { new(length: number): ColumnValues }> }
const INTERVALS: StoredSchema = { contractKey: 'rail_intervals_contract', columns: {
  osm_id: BigInt64Array, segment_idx: Int16Array, from_m: Float64Array, to_m: Float64Array,
  occurrence: BigInt64Array, source_id: Uint16Array, passenger: Float64Array, freight: Float64Array,
  passenger_status: Uint8Array, freight_status: Uint8Array, matching: Uint8Array,
} }
const QUARANTINE: StoredSchema = { contractKey: 'rail_quarantine_contract',
  columns: { source_id: Uint16Array, osm_id: BigInt64Array, segment_idx: Int16Array } }

function readColumns(path: string, stored: StoredSchema): Record<string, ArrayLike<number | bigint>> | null {
  if (!existsSync(path)) return null
  const table = tableFromIPC(readFileSync(path))
  if (table.schema.metadata.get(stored.contractKey) !== '1') throw new Error(`${path}: unsupported ${stored.contractKey}`)
  return Object.fromEntries(Object.keys(stored.columns).map(name => [name, table.getChild(name)!.toArray()]))
}

function writeColumns(path: string, countryIso: string, stored: StoredSchema, rows: ReadonlyArray<Record<string, number>>): void {
  const columns = Object.entries(stored.columns).map(([name, Type]) => {
    const values = new Type(rows.length)
    rows.forEach((row, index) => { values[index] = (values instanceof BigInt64Array ? BigInt(row[name]) : row[name]) as never })
    return [name, makeVector(values)] as const
  })
  const bare = new Table(Object.fromEntries(columns))
  const schema = new Schema(bare.schema.fields, new Map([[stored.contractKey, '1'], ['country_iso', countryIso]]))
  replaceArrowFileDurably(path, new Table(schema, bare.batches))
}

const pieceKey = (piece: RailPieceIdentity): string => `${piece.osmId}:${piece.segmentIndex}`
const intervalKey = (row: RailSquareInterval): string =>
  `${pieceKey(row)}:${row.sourceId}:${row.fromM}:${row.toM}:${row.occurrence}`

/** All mutations of one country in one square: loaded once, written once. */
export class RailTrafficSquareSession {
  /** piece key → interval key → row; a per-piece retraction must not scan a 300 k-row square. */
  private readonly intervals = new Map<string, Map<string, RailSquareInterval>>()
  /** source id → piece key → quarantined piece. */
  private readonly quarantine = new Map<number, Map<string, RailPieceIdentity>>()
  private intervalsChanged = false
  private quarantineChanged = false

  constructor(private readonly squareDirectory: string, private readonly countryIso: string) {
    const stored = readColumns(railIntervalsPath(squareDirectory, countryIso), INTERVALS)
    for (let row = 0; stored && row < stored.osm_id.length; row++) {
      const interval: RailSquareInterval = {
        osmId: Number(stored.osm_id[row]), segmentIndex: Number(stored.segment_idx[row]),
        fromM: Number(stored.from_m[row]), toM: Number(stored.to_m[row]), occurrence: Number(stored.occurrence[row]),
        sourceId: Number(stored.source_id[row]), passenger: Number(stored.passenger[row]), freight: Number(stored.freight[row]),
        passengerStatus: Number(stored.passenger_status[row]), freightStatus: Number(stored.freight_status[row]),
        matching: Number(stored.matching[row]),
      }
      this.store(interval)
    }
    const quarantined = readColumns(railQuarantinePath(squareDirectory, countryIso), QUARANTINE)
    for (let row = 0; quarantined && row < quarantined.osm_id.length; row++) {
      const piece = { osmId: Number(quarantined.osm_id[row]), segmentIndex: Number(quarantined.segment_idx[row]) }
      this.quarantinedPieces(Number(quarantined.source_id[row])).set(pieceKey(piece), piece)
    }
  }

  private quarantinedPieces(sourceId: number): Map<string, RailPieceIdentity> {
    let pieces = this.quarantine.get(sourceId)
    if (!pieces) this.quarantine.set(sourceId, pieces = new Map())
    return pieces
  }

  private store(row: RailSquareInterval): void {
    let piece = this.intervals.get(pieceKey(row))
    if (!piece) this.intervals.set(pieceKey(row), piece = new Map())
    piece.set(intervalKey(row), row)
  }

  rows(): RailSquareInterval[] { return [...this.intervals.values()].flatMap(piece => [...piece.values()]) }

  replaceQuarantine(sourceId: number, pieces: readonly RailPieceIdentity[]): void {
    const replacement = new Map(pieces.map(piece => [pieceKey(piece), piece])), current = this.quarantinedPieces(sourceId)
    if (replacement.size === current.size && [...replacement.keys()].every(key => current.has(key))) return
    this.quarantine.set(sourceId, replacement)
    this.quarantineChanged = true
  }

  /** Drop own rows of the square or of one piece. A quarantined piece keeps its rows as the fallback
   *  veto unless `includingQuarantined`: a current accepted snapshot replaces its old visits. */
  retract(sourceIds: readonly number[], piece?: RailPieceIdentity, includingQuarantined = false): number {
    let retracted = 0
    const pieces = piece ? [this.intervals.get(pieceKey(piece)) ?? new Map<string, RailSquareInterval>()] : this.intervals.values()
    for (const rows of pieces) {
      for (const [key, row] of rows) {
        if (!sourceIds.includes(row.sourceId)) continue
        if (!includingQuarantined && this.quarantine.get(row.sourceId)?.has(pieceKey(row))) continue
        rows.delete(key)
        retracted++
      }
    }
    this.intervalsChanged ||= retracted > 0
    return retracted
  }

  insert(row: RailSquareInterval): void {
    const lower = Math.min(row.fromM, row.toM), upper = Math.max(row.fromM, row.toM)
    if (!(upper > lower) || !Number.isFinite(lower) || !Number.isFinite(upper)) return
    if (row.passengerStatus === RAIL_STATUS.unknown && row.freightStatus === RAIL_STATUS.unknown) return
    const stored = { ...row, fromM: lower, toM: upper,
      passenger: row.passengerStatus === RAIL_STATUS.unknown ? 0 : row.passenger,
      freight: row.freightStatus === RAIL_STATUS.unknown ? 0 : row.freight }
    this.store(stored)
    this.intervalsChanged = true
  }

  /** Quarantine first: a crash between the two renames leaves rows protected, and the country's rerun repairs both. */
  write(): void {
    if (this.quarantineChanged) {
      const rows = [...this.quarantine].flatMap(([sourceId, pieces]) => Array.from(pieces.values(),
        piece => ({ source_id: sourceId, osm_id: piece.osmId, segment_idx: piece.segmentIndex })))
      rows.sort((a, b) => a.source_id - b.source_id || a.osm_id - b.osm_id || a.segment_idx - b.segment_idx)
      writeColumns(railQuarantinePath(this.squareDirectory, this.countryIso), this.countryIso, QUARANTINE, rows)
    }
    if (!this.intervalsChanged) return
    const rows = this.rows().sort((a, b) => a.osmId - b.osmId || a.segmentIndex - b.segmentIndex ||
      a.sourceId - b.sourceId || a.fromM - b.fromM || a.toM - b.toM || a.occurrence - b.occurrence)
    writeColumns(railIntervalsPath(this.squareDirectory, this.countryIso), this.countryIso, INTERVALS, rows.map(row => ({
      osm_id: row.osmId, segment_idx: row.segmentIndex, from_m: row.fromM, to_m: row.toM, occurrence: row.occurrence,
      source_id: row.sourceId, passenger: row.passenger, freight: row.freight,
      passenger_status: row.passengerStatus, freight_status: row.freightStatus, matching: row.matching,
    })))
  }
}

/** The country is in the file name, so border squares never share a file; two processes of one
 *  country serialize on the interval file's lock. Nothing is written when `mutate` throws. */
export function withRailTrafficSquare<T>(
  preparedDirectory: string, square: string, countryIso: string,
  mutate: (session: RailTrafficSquareSession) => T | Promise<T>,
): Promise<T> {
  const squareDirectory = resolve(preparedDirectory, square)
  return withFileLock(railIntervalsPath(squareDirectory, countryIso), async () => {
    const session = new RailTrafficSquareSession(squareDirectory, countryIso)
    const result = await mutate(session)
    session.write()
    return result
  })
}

/** Every stored interval of one square or of the whole year, for inspection and tests. */
export function listRailIntervals(preparedDirectory: string, square?: string): RailIntervalRow[] {
  const prepared = resolve(preparedDirectory)
  const squares = square ? [square] : readdirSync(join(prepared, 'z9')).flatMap(x =>
    readdirSync(join(prepared, 'z9', x)).map(y => `z9/${x}/${y}`))
  const rows: RailIntervalRow[] = []
  for (const name of squares) {
    const directory = join(prepared, name)
    if (!existsSync(directory)) continue
    for (const file of readdirSync(directory)) {
      const countryIso = INTERVAL_FILE.exec(file)?.[1]
      if (!countryIso) continue
      for (const row of new RailTrafficSquareSession(directory, countryIso).rows()) rows.push({ ...row, square: name, countryIso })
    }
  }
  return rows.sort((a, b) => (a.square < b.square ? -1 : Number(a.square > b.square)) || a.osmId - b.osmId ||
    a.segmentIndex - b.segmentIndex || a.fromM - b.fromM || a.occurrence - b.occurrence || a.sourceId - b.sourceId)
}
