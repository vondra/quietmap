/** Stream selected GTFS services into SQLite without losing ordered source observations. */

import { closeSync, existsSync, mkdtempSync, openSync, readSync, renameSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { DatabaseSync, type StatementSync } from 'node:sqlite'
import { readCsvRows, readGtfsStopTimes } from './gtfs-csv.js'
import { computeActiveTripFamiliesForFeed, loadStopsWithCoords, RAIL_TYPES,
  readGtfsTripDepartureMultipliers, resolveStopViaParent } from './gtfs-enrich-core.js'

export interface GtfsServiceOptions {
  familyOf?: (routeType: number) => 'rail' | null
  dateSelection?: (calendarRows: Record<string, string>[]) => string
  /** Identity of caller-supplied family/date policy, including captured values. */
  optionsKey?: string
  cachePath?: string
}

export interface GtfsServiceStop {
  sequence: number
  sourceStopId: string
  stopId: string
  lat: number
  lon: number
  name: string
  arrivalTime: string
  departureTime: string
  shapeDistance: number | null
}

interface GtfsShapePoint { sequence: number; lat: number; lon: number; shapeDistance: number | null }
export interface GtfsService {
  tripId: string
  routeId: string
  serviceId: string
  directionId: string
  shapeId: string
  departureMultiplier: number
  stops: GtfsServiceStop[]
  shape: GtfsShapePoint[]
}

interface ServiceProvenance {
  targetDate: string
  calendarPresent: boolean
  activeTripCount: number
  tripsWithStopTimes: number
  stopTimesLines: number
  tripsWithShape: number
  frequenciesPresent: boolean
  fromCache: boolean
}

const CACHE_VERSION = 3
const INPUT_FILES = ['routes.txt', 'trips.txt', 'stop_times.txt', 'stops.txt', 'shapes.txt',
  'calendar.txt', 'calendar_dates.txt', 'frequencies.txt', 'feed_info.txt'] as const

function inputIdentity(directory: string): string {
  return JSON.stringify([directory, ...INPUT_FILES.map(name => {
    const path = join(directory, name)
    if (!existsSync(path)) return [name, null]
    const stat = statSync(path)
    return [name, stat.size, stat.mtimeMs]
  })])
}

export class GtfsServiceStore implements Disposable {
  private readonly serviceQuery: StatementSync
  private readonly stopQuery: StatementSync
  private readonly shapeQuery: StatementSync
  private lastShapeId = ''
  private lastShape: GtfsShapePoint[] = []

  constructor(
    readonly sourceDirectory: string,
    readonly provenance: ServiceProvenance,
    private readonly database: DatabaseSync,
    private readonly temporaryDirectory?: string,
  ) {
    this.serviceQuery = database.prepare(`SELECT trip_id AS tripId, route_id AS routeId,
      service_id AS serviceId, direction_id AS directionId, shape_id AS shapeId,
      departure_multiplier AS departureMultiplier FROM services WHERE trip_id = ?`)
    this.stopQuery = database.prepare(`SELECT t.sequence, t.source_stop_id AS sourceStopId,
      s.stop_id AS stopId, s.lat, s.lon, s.name, t.arrival_time AS arrivalTime,
      t.departure_time AS departureTime, t.shape_distance AS shapeDistance
      FROM stop_times t JOIN stops s ON s.source_stop_id = t.source_stop_id
      WHERE t.trip_id = ? ORDER BY t.sequence`)
    this.shapeQuery = database.prepare('SELECT points FROM shapes WHERE shape_id = ?')
  }

  service(tripId: string): GtfsService | undefined {
    const row = this.serviceQuery.get(tripId) as unknown as Omit<GtfsService, 'stops' | 'shape'> | undefined
    if (!row) return undefined
    if (row.shapeId !== this.lastShapeId) {
      this.lastShapeId = row.shapeId
      this.lastShape = row.shapeId ? JSON.parse(this.shapeQuery.get(row.shapeId)!.points as string) : []
    }
    return { ...row, stops: this.stopQuery.all(tripId) as unknown as GtfsServiceStop[], shape: this.lastShape }
  }

  *services(): Generator<GtfsService> {
    for (const row of this.database.prepare('SELECT trip_id FROM services ORDER BY shape_id, trip_id').iterate()) {
      yield this.service(row.trip_id as string)!
    }
  }

  [Symbol.dispose](): void {
    try { this.database.close() }
    finally { if (this.temporaryDirectory) rmSync(this.temporaryDirectory, { recursive: true, force: true }) }
  }
}

function cachedServices(path: string, directory: string, inputs: string, options: string): GtfsServiceStore | undefined {
  if (!existsSync(path)) return undefined
  const fd = openSync(path, 'r'), header = Buffer.alloc(16)
  try { readSync(fd, header, 0, 16, 0) } finally { closeSync(fd) }
  if (header.toString() !== 'SQLite format 3\0') return undefined
  const database = new DatabaseSync(path, { readOnly: true })
  try {
    if (database.prepare('PRAGMA user_version').get()?.user_version === CACHE_VERSION) {
      const saved = database.prepare('SELECT inputs, options, provenance FROM identity').get()
      if (saved?.inputs === inputs && saved.options === options) {
        return new GtfsServiceStore(directory, { ...JSON.parse(saved.provenance as string), fromCache: true }, database)
      }
    }
  } catch (error) { database.close(); throw error }
  database.close()
  return undefined
}

function sequenceNumber(value: string, field: string): number {
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value))) throw new Error(`invalid ${field}: '${value}'`)
  return Number(value)
}

function shapeDistance(value: string): number | null {
  if (value === '') return null
  const distance = Number(value)
  if (!Number.isFinite(distance) || distance < 0) throw new Error(`invalid shape_dist_traveled: '${value}'`)
  return distance
}

async function importServices(database: DatabaseSync, directory: string, options: GtfsServiceOptions): Promise<ServiceProvenance> {
  const selected = await computeActiveTripFamiliesForFeed(directory,
    options.familyOf ?? (routeType => RAIL_TYPES.has(routeType) ? 'rail' : null), options.dateSelection)
  const provenance: ServiceProvenance = { targetDate: selected.targetDate, calendarPresent: selected.calendarPresent,
    activeTripCount: selected.tripFam.size, tripsWithStopTimes: 0, stopTimesLines: 0, tripsWithShape: 0,
    frequenciesPresent: existsSync(join(directory, 'frequencies.txt')), fromCache: false }
  if (!selected.tripFam.size) return provenance
  const multipliers = await readGtfsTripDepartureMultipliers(directory, new Set(selected.tripFam.keys()))
  const requiredShapes = new Set<string>()
  const insertService = database.prepare('INSERT INTO services VALUES (?, ?, ?, ?, ?, ?)')
  await readCsvRows(join(directory, 'trips.txt'), row => {
    if (!selected.tripFam.has(row.trip_id)) return
    const shapeId = row.shape_id || ''
    if (shapeId) { requiredShapes.add(shapeId); provenance.tripsWithShape++ }
    insertService.run(row.trip_id, row.route_id, row.service_id, row.direction_id || '', shapeId,
      multipliers.get(row.trip_id) ?? 1)
  })

  if (requiredShapes.size) {
    database.exec(`CREATE TEMP TABLE shape_points (shape_id TEXT, sequence INTEGER, lat REAL, lon REAL,
      shape_distance REAL, PRIMARY KEY(shape_id, sequence)) WITHOUT ROWID`)
    const insertPoint = database.prepare('INSERT INTO shape_points VALUES (?, ?, ?, ?, ?)')
    await readCsvRows(join(directory, 'shapes.txt'), row => {
      if (!requiredShapes.has(row.shape_id)) return
      const lat = row.shape_pt_lat === '' ? NaN : Number(row.shape_pt_lat)
      const lon = row.shape_pt_lon === '' ? NaN : Number(row.shape_pt_lon)
      if (!Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180) {
        throw new Error(`invalid coordinates in shape '${row.shape_id}'`)
      }
      insertPoint.run(row.shape_id, sequenceNumber(row.shape_pt_sequence, 'shape_pt_sequence'), lat, lon,
        shapeDistance(row.shape_dist_traveled || ''))
    })
    const points = database.prepare(`SELECT sequence, lat, lon, shape_distance AS shapeDistance
      FROM shape_points WHERE shape_id = ? ORDER BY sequence`)
    const insertShape = database.prepare('INSERT INTO shapes VALUES (?, ?)')
    for (const shapeId of requiredShapes) {
      const shape = points.all(shapeId)
      if (!shape.length) throw new Error(`referenced shape '${shapeId}' has no vertices`)
      insertShape.run(shapeId, JSON.stringify(shape))
    }
    database.exec('DROP TABLE shape_points')
  }

  const sourceStops = await loadStopsWithCoords(directory)
  const insertedStops = new Set<string>()
  const insertStop = database.prepare('INSERT INTO stops VALUES (?, ?, ?, ?, ?)')
  const insertTime = database.prepare('INSERT INTO stop_times VALUES (?, ?, ?, ?, ?, ?)')
  provenance.stopTimesLines = await readGtfsStopTimes(directory, selected.tripFam, (headers, tripIdIndex) => {
    const stopIndex = headers.indexOf('stop_id'), sequenceIndex = headers.indexOf('stop_sequence')
    if (stopIndex < 0 || sequenceIndex < 0) throw new Error('stop_times.txt missing stop_id/stop_sequence')
    const arrivalIndex = headers.indexOf('arrival_time'), departureIndex = headers.indexOf('departure_time')
    const distanceIndex = headers.indexOf('shape_dist_traveled')
    return fields => {
      const tripId = fields[tripIdIndex], sourceStopId = fields[stopIndex]
      if (!insertedStops.has(sourceStopId)) {
        const { stop } = resolveStopViaParent(sourceStops, sourceStopId)
        if (!stop) throw new Error(`${directory}: active rail trip '${tripId}' has unresolved stop '${sourceStopId}'`)
        insertStop.run(sourceStopId, stop.stop_id, stop.lat, stop.lon, stop.name)
        insertedStops.add(sourceStopId)
      }
      insertTime.run(tripId, sequenceNumber(fields[sequenceIndex], 'stop_sequence'), sourceStopId,
        fields[arrivalIndex] || '', fields[departureIndex] || '', shapeDistance(fields[distanceIndex] || ''))
    }
  })
  const missing = database.prepare(`SELECT trip_id FROM services s WHERE NOT EXISTS
    (SELECT 1 FROM stop_times t WHERE t.trip_id = s.trip_id) LIMIT 1`).get()
  if (missing) throw new Error(`${directory}: active rail trip '${missing.trip_id}' has no stop_times`)
  provenance.tripsWithStopTimes = selected.tripFam.size
  return provenance
}

export async function openGtfsServices(extractDir: string, options: GtfsServiceOptions = {}): Promise<GtfsServiceStore> {
  const directory = resolve(extractDir), inputs = inputIdentity(directory), policy = options.optionsKey ?? 'default'
  const cachePath = options.cachePath ? resolve(options.cachePath) : undefined
  if (cachePath) {
    const cached = cachedServices(cachePath, directory, inputs, policy)
    if (cached) return cached
  }
  const temporaryDirectory = mkdtempSync(join(cachePath ? dirname(cachePath) : tmpdir(), '.gtfs-services-'))
  const temporaryPath = join(temporaryDirectory, 'services.sqlite')
  try {
    const database = new DatabaseSync(temporaryPath)
    let provenance: ServiceProvenance
    try {
      database.exec(`PRAGMA foreign_keys = ON; PRAGMA temp_store = FILE;
        CREATE TABLE identity (inputs TEXT NOT NULL, options TEXT NOT NULL, provenance TEXT NOT NULL);
        CREATE TABLE services (trip_id TEXT PRIMARY KEY, route_id TEXT NOT NULL, service_id TEXT NOT NULL,
          direction_id TEXT NOT NULL, shape_id TEXT NOT NULL, departure_multiplier REAL NOT NULL);
        CREATE INDEX services_shape ON services(shape_id, trip_id);
        CREATE TABLE shapes (shape_id TEXT PRIMARY KEY, points TEXT NOT NULL);
        CREATE TABLE stops (source_stop_id TEXT PRIMARY KEY, stop_id TEXT NOT NULL,
          lat REAL NOT NULL, lon REAL NOT NULL, name TEXT NOT NULL);
        CREATE TABLE stop_times (trip_id TEXT REFERENCES services(trip_id), sequence INTEGER NOT NULL,
          source_stop_id TEXT NOT NULL REFERENCES stops(source_stop_id), arrival_time TEXT NOT NULL,
          departure_time TEXT NOT NULL, shape_distance REAL, PRIMARY KEY(trip_id, sequence)) WITHOUT ROWID;
        BEGIN`)
      provenance = await importServices(database, directory, options)
      if (inputIdentity(directory) !== inputs) throw new Error('GTFS source changed during import; cache not published')
      database.prepare('INSERT INTO identity VALUES (?, ?, ?)').run(inputs, policy, JSON.stringify(provenance))
      database.exec(`PRAGMA user_version = ${CACHE_VERSION}; COMMIT`)
    } finally { database.close() }
    if (cachePath && provenance.activeTripCount > 0) {
      renameSync(temporaryPath, cachePath)
      rmSync(temporaryDirectory, { recursive: true, force: true })
      return new GtfsServiceStore(directory, provenance, new DatabaseSync(cachePath, { readOnly: true }))
    }
    return new GtfsServiceStore(directory, provenance, new DatabaseSync(temporaryPath, { readOnly: true }), temporaryDirectory)
  } catch (error) {
    rmSync(temporaryDirectory, { recursive: true, force: true })
    throw error
  }
}
