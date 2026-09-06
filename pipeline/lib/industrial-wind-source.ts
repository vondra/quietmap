/** Admit the seven original turbine registers once without modifying source caches. */

import { createHash } from 'node:crypto'
import { readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { parse } from 'csv-parse/sync'
import { readSheet } from 'read-excel-file/node'
import proj4 from 'proj4'
import shp from 'shpjs'
import type { PreparedBbox } from './prepared-grid.js'
import { inBbox } from './spatial.js'

export interface WindObservation { latitude: number; longitude: number; hub: number; power: number }
export interface WindRegister { country: string; bbox: PreparedBbox; radiusM: number; observations: WindObservation[] }
// Original national target scopes and strict matching radii; ES covers only CLM.
export const WIND_COUNTRIES = [
  { country: 'CA', bbox: [41.5, -141, 84, -52], radiusM: 500 },
  { country: 'DE', bbox: [46, 4, 56, 16], radiusM: 200 },
  { country: 'DK', bbox: [54.5, 8, 57.8, 13], radiusM: 200 },
  { country: 'ES', bbox: [37, -6, 42, 0], radiusM: 200 },
  { country: 'NO', bbox: [57.9, 4.5, 71.2, 31.2], radiusM: 500 },
  { country: 'SE', bbox: [55.3, 10.9, 69.1, 24.2], radiusM: 200 },
  { country: 'US', bbox: [17.5, -180, 71.5, -65], radiusM: 500 },
] as const

function number(value: unknown): number {
  if (value == null || value === '') return 0
  const parsed = typeof value === 'number' ? value : Number.parseFloat(String(value).replace(',', '.'))
  if (!Number.isFinite(parsed)) throw new Error(`invalid turbine measurement: ${String(value)}`)
  return parsed
}
function observation(lat: number, lon: number, hub: number, power: number): WindObservation {
  if (![lat, lon, hub, power].every(Number.isFinite) || Math.abs(lat) > 90 || Math.abs(lon) > 180 || hub < 0 || power < 0) {
    throw new Error('invalid admitted turbine coordinates or measurements')
  }
  return { latitude: lat, longitude: lon, hub, power }
}
function csv(bytes: Buffer, required: string[]): Record<string, string>[] {
  const rows = parse(bytes, { columns: true, skip_empty_lines: true, bom: true }) as Record<string, string>[]
  if (!rows.length || required.some(name => !(name in rows[0]))) throw new Error(`turbine CSV missing records/columns: ${required.join(',')}`)
  return rows
}
function array(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value) || !value.length || value.some(r => !r || typeof r !== 'object' || Array.isArray(r))) throw new Error('turbine source must contain records')
  return value as Record<string, unknown>[]
}
function project(zone: number, x: number, y: number) {
  return proj4(`+proj=utm +zone=${zone} +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs`, 'WGS84', [x, y])
}

export function parseWindWorkbook(country: 'CA' | 'DK', rows: readonly (readonly unknown[])[]): WindObservation[] {
  const observations: WindObservation[] = []
  let start = 1
  if (country === 'CA') {
    if (rows[0]?.[6] !== 'Turbine Rated Capacity (kW)' || rows[0]?.[12] !== 'Latitude' || rows[0]?.[13] !== 'Longitude') throw new Error('NRCan WTD columns changed')
  } else {
    if (!rows.slice(0, 30).some(r => r[0] === 'Møllenummer (GSRN)' && r[3] === 'Kapacitet (kW)' && r[5] === 'Navhøjde (m)')) throw new Error('ENS turbine columns changed')
    start = rows.slice(0, 30).findIndex(r => typeof r[0] === 'number' || typeof r[0] === 'string' && /^\d/.test(r[0]))
    if (start < 0) throw new Error('ENS turbine records missing')
  }
  if (rows.length <= start) throw new Error('turbine workbook has no records')
  for (const row of rows.slice(start)) {
    if (country === 'CA') {
      const lat = number(row[12]), lon = number(row[13]), power = number(row[6])
      if (!lat || !lon || power <= 0 || !inBbox(lat, lon, WIND_COUNTRIES[0].bbox)) continue
      observations.push(observation(lat, lon, number(row[8]), power))
    } else {
      if (!row[0] || row[2] != null && row[2] !== '' && row[2] !== 0) continue
      const power = number(row[3]), hub = number(row[5]), x = number(row[12]), y = number(row[13])
      if (!x || !y || x < 100000 || power <= 0 && hub <= 0) continue
      const [lon, lat] = project(32, x, y)
      if (inBbox(lat, lon, [54, 7, 58, 15])) observations.push(observation(lat, lon, hub, power))
    }
  }
  return observations
}

export function parseWindCsv(country: 'DE' | 'ES' | 'US', bytes: Buffer): WindObservation[] {
  const required = country === 'DE' ? ['lon', 'lat', 'rated_power_kw', 'hub_height_m']
    : country === 'ES' ? ['UTM_X', 'UTM_Y', 'POTENCIA_UNI', 'ALTURA_BUJE'] : ['ylat', 'xlong', 't_cap', 't_hh']
  const observations: WindObservation[] = []
  for (const row of csv(bytes, required)) {
    if (country === 'DE') {
      const lat = number(row.lat), lon = number(row.lon), power = number(row.rated_power_kw)
      if (lat > 47 && lat < 56 && lon > 5 && lon < 16 && power > 0) observations.push(observation(lat, lon, number(row.hub_height_m), power))
    } else if (country === 'ES') {
      const x = number(row.UTM_X), y = number(row.UTM_Y), power = number(row.POTENCIA_UNI)
      if (!x || !y || !power) continue
      const [lon, lat] = project(30, x, y)
      if (inBbox(lat, lon, [35, -10, 44, 5])) observations.push(observation(lat, lon, number(row.ALTURA_BUJE), power))
    } else {
      const lat = number(row.ylat), lon = number(row.xlong), hub = number(row.t_hh), power = number(row.t_cap)
      // The preceding global pass also admits hub-only USWTDB observations.
      if (lat && lon && inBbox(lat, lon, [13, -180, 72, -60]) && (hub > 0 || power > 0)) observations.push(observation(lat, lon, hub, power))
    }
  }
  return observations
}

export function parseNorwegianWind(parksValue: unknown, turbinesValue: unknown): WindObservation[] {
  const parks = array(parksValue), turbines = array(turbinesValue), powerByPark = new Map<unknown, number>()
  for (const park of parks) {
    const p = park.properties as Record<string, unknown> | undefined
    if (!p || !('status' in p) || !('anleggsNr' in p)) throw new Error('invalid NVE park record')
    if (!p.anleggsNr || p.status !== 'D') continue
    const power = number(p.effekt_MW_idrift || p.effekt_MW), count = number(p.antallTurbiner)
    if (power > 0 && count > 0) {
      if (!Number.isInteger(count)) throw new Error('invalid NVE turbine count')
      powerByPark.set(p.anleggsNr, power / count * 1000)
    }
  }
  const observations: WindObservation[] = []
  for (const turbine of turbines) {
    const p = turbine.properties as Record<string, unknown> | undefined
    if (!p || !('status' in p) || !('anleggsNr' in p)) throw new Error('invalid NVE turbine record')
    if (p.status !== 'D' || !p.anleggsNr) continue
    const g = turbine.geometry as { type: string; coordinates: unknown } | null
    if (g === null) continue
    if (g?.type !== 'Point' || !Array.isArray(g.coordinates) || g.coordinates.length < 2) throw new Error('invalid NVE turbine geometry')
    const [lon, lat] = g.coordinates.map(number), power = powerByPark.get(p.anleggsNr)
    if (lat && lon && power !== undefined) observations.push(observation(lat, lon, 0, power))
  }
  return observations
}

export async function parseSwedishWind(bytes: Buffer): Promise<WindObservation[]> {
  const decoded = await shp(bytes), collections = Array.isArray(decoded) ? decoded : [decoded]
  if (collections.length !== 1 || collections[0].type !== 'FeatureCollection') throw new Error('Vindbrukskollen must contain one turbine layer')
  const features = array(collections[0].features), observations: WindObservation[] = []
  for (const feature of features) {
    const p = feature.properties as Record<string, unknown> | undefined
    if (!p || !('STATUS' in p) || !('NAVHOJD' in p) || !('MAXEFFEKT' in p)) throw new Error('invalid Vindbrukskollen turbine columns')
    if (p.STATUS !== 'Uppfört') continue
    const g = feature.geometry as { type: string; coordinates: unknown } | null
    if (g === null) continue
    if (g?.type !== 'Point' || !Array.isArray(g.coordinates) || g.coordinates.length < 2) throw new Error('invalid Vindbrukskollen geometry')
    const [lon, lat] = g.coordinates.map(number), hub = number(p.NAVHOJD || 0), power = number(p.MAXEFFEKT || 0) * 1000
    if (lat && lon && (hub > 0 || power > 0)) observations.push(observation(lat, lon, hub, power))
  }
  return observations
}

export async function loadWindRegisters(enrichmentDirectory: string) {
  const receipts: Array<{ path: string; bytes: number; sha256: string }> = []
  const read = (file: string) => {
    const path = resolve(enrichmentDirectory, file), before = statSync(path, { bigint: true }), bytes = readFileSync(path), after = statSync(path, { bigint: true })
    if (!before.isFile() || ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs'].some(k => before[k as keyof typeof before] !== after[k as keyof typeof after])) throw new Error(`wind source changed while read: ${path}`)
    receipts.push({ path, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') })
    return bytes
  }
  const registers: WindRegister[] = []
  for (const policy of WIND_COUNTRIES) {
    const c = policy.country
    const observations = c === 'CA' ? parseWindWorkbook(c, await readSheet(read('ca/wind-turbines-en.xlsx'), 'WTD'))
      : c === 'DK' ? parseWindWorkbook(c, await readSheet(read('dk/ens-windturbines.xlsx'), 'Vindmølledata'))
        : c === 'DE' ? parseWindCsv(c, read('de/mastr-wind.csv'))
          : c === 'ES' ? parseWindCsv(c, read('es/clm-aerogeneradores.csv'))
            : c === 'NO' ? parseNorwegianWind(JSON.parse(read('no/nve-vindkraftverk.json').toString()), JSON.parse(read('no/nve-vindturbiner.json').toString()))
              : c === 'SE' ? await parseSwedishWind(read('se/vindbrukskollen.zip'))
                : parseWindCsv(c, read('../global/uswtdb.csv'))
    registers.push({ ...policy, observations })
  }
  return { registers, receipts }
}
