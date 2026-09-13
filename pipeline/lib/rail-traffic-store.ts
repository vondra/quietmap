/** SQLite sidecar for clipped and whole-piece railway evidence (`rail_traffic_contract=1` input). */

import { basename, dirname, join, resolve } from 'node:path'
import { DatabaseSync, type StatementSync } from 'node:sqlite'
import type { RailServiceMatching, RailTrafficStatus } from './rail-passage.js'

export const RAIL_TRAFFIC_SIDECAR_VERSION = 1

export function railTrafficSidecarPath(preparedDirectory: string): string {
  const directory = resolve(preparedDirectory)
  return join(dirname(directory), `${basename(directory)}.rail-traffic.sqlite`)
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

const SCHEMA = `
CREATE TABLE IF NOT EXISTS rail_interval (
  square TEXT NOT NULL,
  osm_id INTEGER NOT NULL,
  segment_idx INTEGER NOT NULL,
  from_m REAL NOT NULL,
  to_m REAL NOT NULL,
  occurrence INTEGER NOT NULL,
  source_id INTEGER NOT NULL,
  country_iso TEXT NOT NULL,
  passenger REAL NOT NULL,
  freight REAL NOT NULL,
  passenger_status INTEGER NOT NULL,
  freight_status INTEGER NOT NULL,
  matching INTEGER NOT NULL,
  PRIMARY KEY (source_id, country_iso, square, osm_id, segment_idx, from_m, to_m, occurrence)
);
CREATE INDEX IF NOT EXISTS rail_interval_square ON rail_interval(square);
CREATE INDEX IF NOT EXISTS rail_interval_piece ON rail_interval(square, osm_id, segment_idx);
CREATE TABLE IF NOT EXISTS rail_quarantine (
  source_id INTEGER NOT NULL,
  country_iso TEXT NOT NULL,
  square TEXT NOT NULL,
  osm_id INTEGER NOT NULL,
  segment_idx INTEGER NOT NULL,
  PRIMARY KEY (source_id, country_iso, square, osm_id, segment_idx)
);
`

export interface RailIntervalRow {
  square: string
  osmId: number
  segmentIndex: number
  fromM: number
  toM: number
  /** Visit ordinal within a source snapshot, disjoint across its services. */
  occurrence: number
  sourceId: number
  countryIso: string
  passenger: number
  freight: number
  passengerStatus: number
  freightStatus: number
  matching: number
}

export function openRailTrafficSidecar(preparedDirectory: string): DatabaseSync {
  const path = railTrafficSidecarPath(preparedDirectory)
  const database = new DatabaseSync(path)
  database.exec('PRAGMA busy_timeout=60000')
  database.exec('PRAGMA journal_mode=WAL')
  const version = Number(database.prepare('PRAGMA user_version').get()?.user_version ?? 0)
  if (version === 0) {
    database.exec(SCHEMA)
    database.exec(`PRAGMA user_version = ${RAIL_TRAFFIC_SIDECAR_VERSION}`)
  } else if (version !== RAIL_TRAFFIC_SIDECAR_VERSION) {
    database.close()
    throw new Error(`rail-traffic sidecar schema ${version} is unsupported (want ${RAIL_TRAFFIC_SIDECAR_VERSION})`)
  }
  return database
}

const intervalInserts = new WeakMap<DatabaseSync, StatementSync>()

export function insertRailInterval(database: DatabaseSync, row: RailIntervalRow): void {
  const lower = Math.min(row.fromM, row.toM)
  const upper = Math.max(row.fromM, row.toM)
  if (!(upper > lower) || !Number.isFinite(lower) || !Number.isFinite(upper)) return
  if (row.passengerStatus === RAIL_STATUS.unknown && row.freightStatus === RAIL_STATUS.unknown) return
  let insert = intervalInserts.get(database)
  if (!insert) {
    insert = database.prepare(`
    INSERT OR REPLACE INTO rail_interval (
      square, osm_id, segment_idx, from_m, to_m, occurrence, source_id, country_iso,
      passenger, freight, passenger_status, freight_status, matching
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    `)
    intervalInserts.set(database, insert)
  }
  insert.run(
    row.square, row.osmId, row.segmentIndex, lower, upper, row.occurrence, row.sourceId, row.countryIso,
    row.passengerStatus === RAIL_STATUS.unknown ? 0 : row.passenger,
    row.freightStatus === RAIL_STATUS.unknown ? 0 : row.freight,
    row.passengerStatus, row.freightStatus, row.matching,
  )
}

export function retractRailSourceSquare(
  database: DatabaseSync,
  sourceIds: readonly number[],
  countryIso: string,
  square: string,
  piece?: { osmId: number; segmentIndex: number },
): number {
  if (sourceIds.length === 0) return 0
  const placeholders = sourceIds.map(() => '?').join(',')
  const pieceClause = piece ? 'AND osm_id = ? AND segment_idx = ?' : ''
  const result = database.prepare(`
    DELETE FROM rail_interval WHERE source_id IN (${placeholders}) AND country_iso = ? AND square = ?
      ${pieceClause}
      AND NOT EXISTS (
        SELECT 1 FROM rail_quarantine q
        WHERE q.source_id = rail_interval.source_id AND q.country_iso = rail_interval.country_iso
          AND q.square = rail_interval.square AND q.osm_id = rail_interval.osm_id
          AND q.segment_idx = rail_interval.segment_idx
      )
  `).run(...sourceIds, countryIso, square, ...(piece ? [piece.osmId, piece.segmentIndex] : []))
  return Number(result.changes ?? 0)
}

export function replaceRailQuarantine(
  database: DatabaseSync,
  sourceId: number,
  countryIso: string,
  square: string,
  pieces: ReadonlyArray<{ osmId: number; segmentIndex: number }>,
): void {
  database.prepare(
    'DELETE FROM rail_quarantine WHERE source_id = ? AND country_iso = ? AND square = ?',
  ).run(sourceId, countryIso, square)
  const insert = database.prepare(
    'INSERT INTO rail_quarantine (source_id, country_iso, square, osm_id, segment_idx) VALUES (?, ?, ?, ?, ?)',
  )
  for (const piece of pieces) insert.run(sourceId, countryIso, square, piece.osmId, piece.segmentIndex)
}

export function listRailIntervals(preparedDirectory: string, square?: string): RailIntervalRow[] {
  const database = openRailTrafficSidecar(preparedDirectory)
  try {
    const sql = square
      ? 'SELECT * FROM rail_interval WHERE square = ? ORDER BY osm_id, segment_idx, from_m, occurrence'
      : 'SELECT * FROM rail_interval ORDER BY square, osm_id, segment_idx, from_m, occurrence'
    const rows = square ? database.prepare(sql).all(square) : database.prepare(sql).all()
    return rows.map(row => ({
      square: String(row.square),
      osmId: Number(row.osm_id),
      segmentIndex: Number(row.segment_idx),
      fromM: Number(row.from_m),
      toM: Number(row.to_m),
      occurrence: Number(row.occurrence),
      sourceId: Number(row.source_id),
      countryIso: String(row.country_iso),
      passenger: Number(row.passenger),
      freight: Number(row.freight),
      passengerStatus: Number(row.passenger_status),
      freightStatus: Number(row.freight_status),
      matching: Number(row.matching),
    }))
  } finally {
    database.close()
  }
}
