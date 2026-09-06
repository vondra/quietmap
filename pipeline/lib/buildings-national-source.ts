/** Immutable national building cache admission and a disposable SQLite point index. */

import { createHash } from 'node:crypto'
import { createReadStream, readFileSync, statSync } from 'node:fs'
import { createInterface } from 'node:readline'
import { DatabaseSync } from 'node:sqlite'
import { flatDist, pointSearchReach } from './spatial.js'
import { SOURCE_ID_CZ_RUIAN_VFR, SOURCE_ID_ES_CATASTRO } from './source-ids.generated.js'
import type { PreparedBbox } from './prepared-grid.js'

export const NATIONAL_BUILDING_SOURCES = [
  { country: 'CZ', file: 'cz/ruian-buildings.json', sourceId: SOURCE_ID_CZ_RUIAN_VFR,
    bbox: [48.2, 11.7, 51.4, 19.2] as PreparedBbox },
  { country: 'ES', file: 'es/catastro-buildings.json', sourceId: SOURCE_ID_ES_CATASTRO,
    bbox: [27, -19, 44, 5] as PreparedBbox },
] as const
export type NationalBuildingSource = typeof NATIONAL_BUILDING_SOURCES[number]
export interface NationalBuildingPoint { lat: number; lon: number; floors: number; buildingType: number | null }

// RÚIAN ZpusobVyuzitiKod mapping retained from the national VFR adapter.
const RUIAN_BUILDING_TYPES: Readonly<Record<number, number>> = {
  2: 8, 6: 0, 7: 0, 8: 0, 9: 9, 10: 1, 11: 6, 12: 2, 13: 8,
  14: 1, 15: 9, 16: 2, 17: 7, 18: 7, 19: 0, 20: 1, 21: 8,
}

function identity(path: string) {
  const stat = statSync(path, { bigint: true })
  if (!stat.isFile()) throw new Error(`${path}: national building cache is not a file`)
  return { path, device: String(stat.dev), inode: String(stat.ino), bytes: String(stat.size),
    mtimeNs: String(stat.mtimeNs), ctimeNs: String(stat.ctimeNs) }
}

function admit(value: unknown, source: NationalBuildingSource, path: string, row: number): NationalBuildingPoint {
  const point = value as Record<string, unknown> | null
  const bad = () => new Error(`${path}: invalid national building observation ${row}`)
  if (!point || typeof point !== 'object' || Array.isArray(point)) throw bad()
  const { lat, lon, floors, useCode } = point
  if (typeof lat !== 'number' || typeof lon !== 'number' || typeof floors !== 'number' ||
      !Number.isFinite(lat) || !Number.isFinite(lon) || !Number.isSafeInteger(floors) || floors < 0 ||
      lat < source.bbox[0] || lat > source.bbox[2] || lon < source.bbox[1] || lon > source.bbox[3]) throw bad()
  if (source.country === 'ES' && (floors === 0 || floors > 255)) throw bad()
  if (source.country === 'CZ' && (typeof useCode !== 'number' || !Number.isSafeInteger(useCode) || useCode < 0)) throw bad()
  return { lat, lon, floors: Math.min(floors, 255),
    buildingType: source.country === 'CZ' ? RUIAN_BUILDING_TYPES[useCode as number] ?? null : null }
}

export async function indexNationalBuildings(path: string, databasePath: string, source: NationalBuildingSource) {
  const before = identity(path)
  const hash = createHash('sha256')
  const database = new DatabaseSync(databasePath)
  try {
    // This index is disposable, never source authority; a failed admission discards it.
    database.exec(`PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-32768;
      CREATE TABLE points (y INTEGER, x INTEGER, seq INTEGER, lat REAL, lon REAL, floors INTEGER, buildingType INTEGER,
        PRIMARY KEY(y,x,seq)) WITHOUT ROWID; BEGIN`)
    const insert = database.prepare('INSERT INTO points VALUES (?,?,?,?,?,?,?)')
    let records = 0
    function add(value: unknown) {
      const point = admit(value, source, path, records)
      insert.run(Math.floor(point.lat * 100), Math.floor(point.lon * 100), records++,
        point.lat, point.lon, point.floors, point.buildingType)
    }
    if (source.country === 'CZ') {
      const bytes = readFileSync(path)
      hash.update(bytes)
      const values: unknown = JSON.parse(bytes.toString('utf8'))
      if (!Array.isArray(values)) throw new Error(`${path}: RÚIAN cache must be a JSON array`)
      for (const value of values) add(value)
    } else {
      const stream = createReadStream(path)
      stream.on('data', chunk => { hash.update(chunk) })
      const lines = createInterface({ input: stream, crlfDelay: Infinity })
      try {
        for await (const line of lines) {
          if (line.trim()) add(JSON.parse(line))
        }
      } finally { lines.close(); stream.destroy() }
    }
    if (!records) throw new Error(`${path}: national building cache is empty`)
    if (JSON.stringify(identity(path)) !== JSON.stringify(before)) throw new Error(`${path}: source changed during admission`)
    database.exec('COMMIT')
    const candidates = database.prepare(`SELECT lat,lon,floors,buildingType FROM points
      WHERE y=? AND x=? AND lat BETWEEN ? AND ? AND lon BETWEEN ? AND ? ORDER BY seq`)
    return {
      source,
      receipt: { ...before, sha256: hash.digest('hex'), records,
        coverage: 'retained observations; municipality completeness is not established' },
      nearest(lat: number, lon: number): NationalBuildingPoint | null {
        const [dy, dx] = pointSearchReach(lat, 30)
        let closest: NationalBuildingPoint | null = null
        let distance = 30
        // Ascending spatial cells, then original record order, retain dev1 equal-distance ties.
        for (let y = Math.floor((lat - dy) * 100); y <= Math.floor((lat + dy) * 100); y++) {
          for (let x = Math.floor((lon - dx) * 100); x <= Math.floor((lon + dx) * 100); x++) {
            for (const record of candidates.iterate(y, x, lat - dy, lat + dy, lon - dx, lon + dx)) {
              const point = record as unknown as NationalBuildingPoint
              const d = flatDist(lat, lon, point.lat, point.lon)
              if (d < distance) { closest = point; distance = d }
            }
          }
        }
        return closest
      },
      close() { database.close() },
    }
  } catch (error) { database.close(); throw error }
}

export type NationalBuildingIndex = Awaited<ReturnType<typeof indexNationalBuildings>>
