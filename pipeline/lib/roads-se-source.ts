/** Parse admitted Trafikverket NVDB Trafik link-part measurements. */

import { DatabaseSync, type SQLInputValue } from 'node:sqlite'
import { resolve } from 'node:path'
import proj4 from 'proj4'
import { withholdsCountLine } from './count-holdout.js'
import { roadObservation, type RoadObservation } from './road-observation.js'
import type { RoadLoaderArguments } from './road-loader-cli.js'
import { readPinnedRoadSource } from './pinned-road-source.js'

const SOURCE_PATH = 'se/nvdb-trafik-2026.gpkg'
const SOURCE_SHA256 = '9c2bff552a9a051f8e9d06a50750ac6b8b8619fbaf891053035d94a29d6950c8'
const SWEDEN_BBOX = [55.3, 10.9, 69.1, 24.2] as const
const SWEREF99_TM_SRID = 3006

// SWEREF 99 TM is UTM 33 on GRS 80; ETRS 89 and WGS 84 agree far below the 50 m match radius.
proj4.defs(
  'EPSG:3006',
  '+proj=utm +zone=33 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs',
)

/** Sample and full-year loop measurements count; assessed (Bedömt) flows are modelled, not counted. */
const MEASURED_METHODS = new Set(['Stickprovsmätning', 'Helårsmätning'])
/** Sibling (Syskon) carriageway links carry one direction's flow; Normal links the two-way total. */
const DIRECTIONAL_ROLES = new Set(['Syskon fram', 'Syskon bak'])

export interface SwedishNvdbObservation extends RoadObservation {
  /** WGS 84 link-part line in [longitude, latitude] order. */
  line: ReadonlyArray<readonly [number, number]>
  rank: null
  isRamp: false
  light: number
  medium: number
  heavy: number
  moto: number
}

export interface SwedishNvdbSource {
  observations: SwedishNvdbObservation[]
  sourceRows: number
  twoWayObservations: number
  directionalObservations: number
  assessedSkipped: number
  incompleteSplitsSkipped: number
  zeroTrafficSkipped: number
  inconsistentClassesSkipped: number
  invalidGeometrySkipped: number
}

/** One GeoPackage feature row as the loader selects it; the test builds these by hand. */
export interface SwedishNvdbRecord {
  id: unknown
  role: unknown
  method: unknown
  total: unknown
  lightPeriods: readonly unknown[]
  mediumPeriods: readonly unknown[]
  heavyPeriods: readonly unknown[]
  geometry: unknown
}

const isCount = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0

const GPKG_MAGIC = 0x4750
const GPKG_ENVELOPE_BYTES = [0, 32, 48, 48, 64] as const
const WKB_LINE_STRING = 2
const WKB_LINE_STRING_Z = 1002

/** Read the single line of a GeoPackage geometry blob, in its stored coordinates. NVDB
 *  writes Z lines; the heights carry no traffic meaning and are dropped. */
export function decodeGpkgLineString(blob: Uint8Array): { srsId: number; line: [number, number][] } {
  const view = new DataView(blob.buffer, blob.byteOffset, blob.byteLength)
  if (blob.byteLength < 8 || view.getUint16(0, false) !== GPKG_MAGIC) {
    throw new Error('Swedish NVDB geometry is not a GeoPackage blob')
  }
  const envelope = GPKG_ENVELOPE_BYTES[(view.getUint8(3) >> 1) & 0x07] ?? null
  if (envelope === null) throw new Error('Swedish NVDB geometry has an unknown envelope')
  const srsId = view.getInt32(4, true)
  let offset = 8 + envelope
  const wkb = (bytes: number): DataView => {
    if (offset + bytes > blob.byteLength) throw new Error('Swedish NVDB geometry ends early')
    const slice = new DataView(blob.buffer, blob.byteOffset + offset, bytes)
    offset += bytes
    return slice
  }
  const littleEndian = wkb(1).getUint8(0) === 1
  if (!littleEndian) throw new Error('Swedish NVDB geometry is not little-endian WKB')
  const wkbType = wkb(4).getUint32(0, true)
  const stride = wkbType === WKB_LINE_STRING ? 16 : wkbType === WKB_LINE_STRING_Z ? 24 : 0
  if (stride === 0) throw new Error('Swedish NVDB geometry is not a line')
  const points = wkb(4).getUint32(0, true)
  const line: [number, number][] = []
  for (let index = 0; index < points; index++) {
    const pair = wkb(stride)
    line.push([pair.getFloat64(0, true), pair.getFloat64(8, true)])
  }
  return { srsId, line }
}

function projectLine(line: [number, number][]): (readonly [number, number])[] | null {
  const projected: (readonly [number, number])[] = []
  for (const [east, north] of line) {
    if (!Number.isFinite(east) || !Number.isFinite(north)) return null
    const [longitude, latitude] = proj4('EPSG:3006', 'WGS84', [east, north])
    if (
      longitude < SWEDEN_BBOX[1] ||
      longitude > SWEDEN_BBOX[3] ||
      latitude < SWEDEN_BBOX[0] ||
      latitude > SWEDEN_BBOX[2]
    )
      return null
    projected.push([longitude, latitude])
  }
  return projected.length >= 2 ? projected : null
}

export function parseSwedishNvdbSource(records: readonly SwedishNvdbRecord[]): SwedishNvdbSource {
  const result: SwedishNvdbSource = {
    observations: [],
    sourceRows: records.length,
    twoWayObservations: 0,
    directionalObservations: 0,
    assessedSkipped: 0,
    incompleteSplitsSkipped: 0,
    zeroTrafficSkipped: 0,
    inconsistentClassesSkipped: 0,
    invalidGeometrySkipped: 0,
  }
  const seen = new Set<number>()
  for (const record of records) {
    if (!isCount(record.id) || seen.has(record.id)) {
      throw new Error('Swedish NVDB row has invalid section identity')
    }
    seen.add(record.id)
    if (record.method !== null && typeof record.method !== 'string') {
      throw new Error(`Swedish NVDB row ${record.id} has invalid method`)
    }
    if (record.method === null || !MEASURED_METHODS.has(record.method)) {
      result.assessedSkipped++
      continue
    }
    const periods = [...record.lightPeriods, ...record.mediumPeriods, ...record.heavyPeriods]
    if (
      record.lightPeriods.length !== 3 ||
      record.mediumPeriods.length !== 3 ||
      record.heavyPeriods.length !== 3 ||
      !isCount(record.total) ||
      periods.some(value => !isCount(value))
    ) {
      result.incompleteSplitsSkipped++
      continue
    }
    if (record.total === 0) {
      result.zeroTrafficSkipped++
      continue
    }
    const geometry = record.geometry instanceof Uint8Array ? record.geometry : null
    if (geometry === null) {
      result.invalidGeometrySkipped++
      continue
    }
    const decoded = decodeGpkgLineString(geometry)
    if (decoded.srsId !== SWEREF99_TM_SRID) {
      throw new Error(`Swedish NVDB row ${record.id} is not in SWEREF 99 TM`)
    }
    const line = projectLine(decoded.line)
    if (line === null) {
      result.invalidGeometrySkipped++
      continue
    }
    // Adt_tunga_fordon sums medium and heavy, so the classes come from the day-period
    // splits instead; loops cannot see motorcycles, so 1 % of the total rides as moto
    // exactly as on the Danish, Finnish and Dutch censuses, keeping the total exact.
    const [lightDay, lightEvening, lightNight] = record.lightPeriods as [number, number, number]
    const [mediumDay, mediumEvening, mediumNight] = record.mediumPeriods as [number, number, number]
    const [heavyDay, heavyEvening, heavyNight] = record.heavyPeriods as [number, number, number]
    const medium = mediumDay + mediumEvening + mediumNight
    const heavy = heavyDay + heavyEvening + heavyNight
    // Nine independently rounded day-period splits and one separately rounded total
    // differ by less than (9+1)/2; anything beyond is a misread schema, not rounding.
    if (Math.abs(record.total - (lightDay + lightEvening + lightNight + medium + heavy)) > 4) {
      result.inconsistentClassesSkipped++
      continue
    }
    const moto = Math.round(record.total * 0.01)
    const light = record.total - medium - heavy - moto
    if (light < 0) {
      result.inconsistentClassesSkipped++
      continue
    }
    if (withholdsCountLine(line)) continue
    const directional = typeof record.role === 'string' && DIRECTIONAL_ROLES.has(record.role)
    if (directional) result.directionalObservations++
    else result.twoWayObservations++
    result.observations.push({
      ...roadObservation(`nvdb2026:${record.id}`, directional ? 'directional' : 'both-directions'),
      line,
      rank: null,
      isRamp: false,
      light,
      medium,
      heavy,
      moto,
    })
  }
  if (result.observations.length === 0) throw new Error('Swedish NVDB source has no usable measurements')
  return result
}

const TRAFFIC_COLUMNS = [
  'id',
  'ROLE',
  'Matmetod',
  'Adt_samtliga_fordon',
  'Adt_latta_fordon_06_18',
  'Adt_latta_fordon_18_22',
  'Adt_latta_fordon_22_06',
  'Adt_medeltunga_fordon_06_18',
  'Adt_medeltunga_fordon_18_22',
  'Adt_medeltunga_fordon_22_06',
  'Adt_tunga_fordon_06_18',
  'Adt_tunga_fordon_18_22',
  'Adt_tunga_fordon_22_06',
  'geom',
] as const

export function loadSwedishNvdbSource(options: RoadLoaderArguments): SwedishNvdbSource {
  readPinnedRoadSource(options, SOURCE_PATH, SOURCE_SHA256)
  const database = new DatabaseSync(resolve(options.enrichmentDirectory, SOURCE_PATH), { readOnly: true })
  try {
    const tables = database
      .prepare("SELECT table_name AS name FROM gpkg_contents WHERE data_type = 'features'")
      .all() as Record<string, SQLInputValue>[]
    if (tables.length !== 1 || typeof tables[0].name !== 'string') {
      throw new Error('Swedish NVDB source must hold one feature table')
    }
    const table = tables[0].name
    const srs = database
      .prepare('SELECT srs_id AS srs FROM gpkg_geometry_columns WHERE table_name = ?')
      .get(table) as Record<string, SQLInputValue> | undefined
    if (srs?.srs !== SWEREF99_TM_SRID) throw new Error('Swedish NVDB source is not in SWEREF 99 TM')
    const rows = database
      .prepare(`SELECT ${TRAFFIC_COLUMNS.map(name => `"${name}"`).join(', ')} FROM "${table}" ORDER BY id`)
      .all() as Record<string, SQLInputValue>[]
    return parseSwedishNvdbSource(
      rows.map(row => ({
        id: row.id,
        role: row.ROLE,
        method: row.Matmetod,
        total: row.Adt_samtliga_fordon,
        lightPeriods: [
          row.Adt_latta_fordon_06_18,
          row.Adt_latta_fordon_18_22,
          row.Adt_latta_fordon_22_06,
        ],
        mediumPeriods: [
          row.Adt_medeltunga_fordon_06_18,
          row.Adt_medeltunga_fordon_18_22,
          row.Adt_medeltunga_fordon_22_06,
        ],
        heavyPeriods: [
          row.Adt_tunga_fordon_06_18,
          row.Adt_tunga_fordon_18_22,
          row.Adt_tunga_fordon_22_06,
        ],
        geometry: row.geom instanceof Uint8Array ? row.geom : null,
      })),
    )
  } finally {
    database.close()
  }
}
