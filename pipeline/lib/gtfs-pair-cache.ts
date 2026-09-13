/** Cache exact GTFS pair inputs and accounting with one SQLite row per shape. */

import { closeSync, existsSync, mkdtempSync, openSync, readSync, renameSync, rmSync } from 'node:fs'
import { basename, dirname, join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { RailShapeIndex } from './rail-pair-searches.js'
import type { RailStationPairCount } from './rail-graph.js'
import type { StopPairFrequenciesResult } from './gtfs-stop-pairs.js'

const CACHE_VERSION = 1
interface CacheIdentity { options: string; inputs: string }

export function readGtfsPairCache(path: string, identity: CacheIdentity): StopPairFrequenciesResult | null {
  if (!existsSync(path)) return null
  const fd = openSync(path, 'r'), header = Buffer.alloc(16)
  try { readSync(fd, header, 0, 16, 0) } finally { closeSync(fd) }
  if (header.toString() !== 'SQLite format 3\0') return null
  const database = new DatabaseSync(path, { readOnly: true })
  try {
    if (database.prepare('PRAGMA user_version').get()?.user_version !== CACHE_VERSION) return null
    const saved = database.prepare('SELECT options, inputs, provenance FROM identity').get()
    if (!saved || saved.options !== identity.options || saved.inputs !== identity.inputs) return null
    const shapes = new Map<number, Array<[number, number]>>()
    for (const row of database.prepare('SELECT id, geometry FROM shapes').iterate()) {
      shapes.set(row.id as number, JSON.parse(row.geometry as string))
    }
    const pairs: RailStationPairCount[] = []
    for (const raw of database.prepare(`
      SELECT from_lat AS fromLat, from_lon AS fromLon, to_lat AS toLat, to_lon AS toLon,
             pax, frt, shape_id FROM pairs ORDER BY id`).iterate()) {
      const { shape_id, ...pair } = raw as unknown as RailStationPairCount & { shape_id: number | null }
      if (shape_id !== null) {
        const shape = shapes.get(shape_id)
        if (!shape) throw new Error(`GTFS pair cache references missing shape ${shape_id}`)
        pair.shapePolyline = shape
      }
      pairs.push(pair)
    }
    return { pairs, provenance: { ...JSON.parse(saved.provenance as string), fromCache: true } }
  } finally { database.close() }
}

export function writeGtfsPairCache(path: string, identity: CacheIdentity, result: StopPairFrequenciesResult): void {
  const temporaryDirectory = mkdtempSync(join(dirname(path), `.${basename(path)}-`))
  const temporary = join(temporaryDirectory, 'pairs.sqlite')
  try {
    const database = new DatabaseSync(temporary)
    try {
      database.exec(`
        PRAGMA foreign_keys = ON;
        CREATE TABLE identity (options TEXT NOT NULL, inputs TEXT NOT NULL, provenance TEXT NOT NULL);
        CREATE TABLE shapes (id INTEGER PRIMARY KEY, geometry TEXT NOT NULL);
        CREATE TABLE pairs (
          id INTEGER PRIMARY KEY, from_lat REAL NOT NULL, from_lon REAL NOT NULL,
          to_lat REAL NOT NULL, to_lon REAL NOT NULL, pax REAL NOT NULL, frt REAL NOT NULL,
          shape_id INTEGER REFERENCES shapes(id) DEFERRABLE INITIALLY DEFERRED);
        BEGIN;`)
      const shapes = new RailShapeIndex()
      const writePair = database.prepare('INSERT INTO pairs VALUES (?, ?, ?, ?, ?, ?, ?, ?)')
      result.pairs.forEach((p, i) => writePair.run(i, p.fromLat, p.fromLon, p.toLat, p.toLon, p.pax, p.frt,
        p.shapePolyline ? shapes.idFor(p.shapePolyline) : null))
      const writeShape = database.prepare('INSERT INTO shapes VALUES (?, ?)')
      for (const [geometry, id] of shapes.entries()) writeShape.run(id, geometry)
      database.prepare('INSERT INTO identity VALUES (?, ?, ?)').run(identity.options, identity.inputs, JSON.stringify(result.provenance))
      database.exec(`PRAGMA user_version = ${CACHE_VERSION}; COMMIT;`)
    } finally { database.close() }
    renameSync(temporary, path)
  } finally { rmSync(temporaryDirectory, { recursive: true, force: true }) }
}
