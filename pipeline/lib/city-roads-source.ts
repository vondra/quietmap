/** Admit original Praha, Wien and Brno traffic observations without writing normalized source caches. */

import { createHash } from 'node:crypto'
import { readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { parse } from 'csv-parse/sync'
import { readSheet } from 'read-excel-file/node'
import { DATASETS } from './enrichment-datasets.js'
import { SOURCE_ID_CITY_PRAHA_TSK, SOURCE_ID_CITY_WIEN_DAUERZAEHLSTELLEN, SOURCE_ID_CITY_BRNO_DETECTORS } from './sources.js'
import { municipalityFromGeoJson, type CityCoordinate } from './city-polygon.js'

export interface CityRoadRecord {
  street: string
  light: number
  medium: number
  heavy: number
  moto: number
  line?: readonly CityCoordinate[]
}
export const MUNICIPAL_ROAD_SOURCES = [
  { slug: 'praha', country: 'cz', iso3: 'CZE', municipality: 'Prague', sourceId: SOURCE_ID_CITY_PRAHA_TSK },
  { slug: 'wien', country: 'at', iso3: 'AUT', municipality: 'Wien(Stadt)', sourceId: SOURCE_ID_CITY_WIEN_DAUERZAEHLSTELLEN },
  { slug: 'brno', country: 'cz', iso3: 'CZE', municipality: 'Brno-City', sourceId: SOURCE_ID_CITY_BRNO_DETECTORS },
] as const
const EXCLUDED_YEARS = new Set([2020, 2021]) // Original city admission excludes pandemic editions.
const PRAHA_SHEET = 'Úseky - po profilech - dle uzlů'
const PRAHA_STREET_NAMES: Readonly<Record<string, string>> = {
  'BARRAND.MOST': 'Barrandovský most', 'DEJVICKÝ T.': 'Dejvický tunel', 'BRUSNICKÝ T.': 'Brusnický tunel',
  'STRAH.TUNEL': 'Strahovský tunel', 'BUBENEČ.TUN.': 'Bubenečský tunel', 'TUN.MRÁZOVKA': 'Tunel Mrázovka',
  'TĚŠNOVSKÝ T.': 'Těšnovský tunel', 'V HOLEŠOVIČ.': 'V Holešovičkách', '5.KVĚTNA': '5. května',
  'ŠTĚRB.SPOJKA': 'Štěrboholská spojka', 'NUSEL.MOST': 'Nuselský most', 'MOST BARIK.': 'most Barikádníků',
  'ROZVAD.SPOJ.': 'Rozvadovská spojka', 'JIRÁSK.MOST': 'Jiráskův most', 'ROHAN.NÁBŘ.': 'Rohanské nábřeží',
  'NÁB.K.JAROŠE': 'nábřeží Kapitána Jaroše', 'NÁB.L.SVOB.': 'nábřeží Ludvíka Svobody', 'BUBEN.NÁBŘ.': 'Bubenské nábřeží',
  'POD KREJCÁR.': 'Pod Krejcárkem', 'BĚLOCERKEVS.': 'Bělocerkevská', 'J.ŽELIVSKÉHO': 'Jana Želivského',
  'ČERNOKOSTEL.': 'Černokostelecká', 'N.POVLTAVSKÁ': 'Nová Povltavská',
}
function count(value: unknown, label: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) throw new Error(`invalid ${label}: ${String(value)}`)
  return value
}
function coordinate(value: unknown): CityCoordinate {
  if (!Array.isArray(value) || value.length !== 2 || value.some(v => typeof v !== 'number' || !Number.isFinite(v)) ||
      Math.abs(value[0]) > 180 || Math.abs(value[1]) > 90) throw new Error('invalid municipal traffic coordinate')
  return value as unknown as CityCoordinate
}
export function parsePrahaRows(rows: readonly (readonly unknown[])[]) {
  const streets = new Map<string, { length: number; light: number; medium: number; heavy: number }>()
  let sections = 0
  for (const row of rows) {
    const [start, end, street, , , length, cars, slow, , buses] = row
    if (typeof start !== 'number' || typeof end !== 'number') continue
    if (!Number.isFinite(start) || !Number.isFinite(end) || typeof street !== 'string' || !street.trim()) throw new Error('invalid Praha section identity')
    const len = count(length, 'Praha section length')
    if (len === 0) throw new Error('zero Praha section length')
    const light = count(cars, 'Praha cars'), heavy = count(slow, 'Praha slow vehicles'), medium = count(buses, 'Praha buses')
    if (light + heavy + medium === 0) continue
    const key = street.trim(), acc = streets.get(key) ?? { length: 0, light: 0, medium: 0, heavy: 0 }
    acc.length += len; acc.light += light * len; acc.medium += medium * len; acc.heavy += heavy * len
    streets.set(key, acc); sections++
  }
  if (sections < 500) throw new Error(`Praha source has only ${sections} positive sections; expected the monitored network`)
  return { sections, records: [...streets].map(([street, a]): CityRoadRecord => ({
    street: PRAHA_STREET_NAMES[street] ?? street, light: Math.round(a.light / a.length),
    medium: Math.round(a.medium / a.length), heavy: Math.round(a.heavy / a.length), moto: 0,
  })) }
}
export function parseBrno(text: string) {
  const json = JSON.parse(text)
  if (json?.type !== 'FeatureCollection' || !Array.isArray(json.features) || json.exceededTransferLimit === true || json.properties?.exceededTransferLimit === true) throw new Error('Brno source is not a complete FeatureCollection')
  const features = json.features as Array<{ properties: Record<string, unknown>; geometry: { type: string; coordinates: unknown } }>
  if (features.length < 500 || features.length > 1200 || features.some(f => !f?.properties || !f.geometry)) throw new Error('Brno source section count or feature shape drift')
  const years = [...new Set(features.flatMap(f => Object.keys(f.properties).flatMap(key => /^car_(\d{4})$/.exec(key)?.slice(1) ?? [])))]
    .map(Number).filter(year => !EXCLUDED_YEARS.has(year)).sort((a, b) => b - a)
  const year = years.find(y => features.filter(f => {
    const value = f.properties[`car_${y}`]
    return value != null && count(value, 'Brno car thousands') > 0
  }).length >= features.length * 0.9)
  if (year === undefined) throw new Error('Brno has no non-pandemic edition with at least90% populated sections')
  const records: CityRoadRecord[] = []
  for (const f of features) {
    const cars = f.properties[`car_${year}`]
    if (cars == null || count(cars, 'Brno car thousands') === 0) continue
    const total = (cars as number) * 1000, percent = count(f.properties[`truc_${year}`] ?? 0, 'Brno truck percentage')
    if (total > 150000 || percent > 100) throw new Error('Brno section exceeds original traffic/share bounds')
    const parts = f.geometry.type === 'LineString' ? [f.geometry.coordinates] : f.geometry.type === 'MultiLineString' ? f.geometry.coordinates : null
    if (!Array.isArray(parts) || !parts.length) throw new Error('Brno section has invalid line geometry')
    const heavy = Math.round(total * percent / 100)
    for (const part of parts) {
      if (!Array.isArray(part) || part.length < 2) throw new Error('Brno section has an empty line')
      records.push({ street: `BKOM section ${f.properties.id ?? f.properties.ObjectId}`,
        light: total - heavy, medium: 0, heavy, moto: 0, line: part.map(coordinate) })
    }
  }
  if (records.length < 500) throw new Error('Brno has fewer than500 usable sections')
  return { year, records }
}
const MONTH_INDEX: Readonly<Record<string, number>> = {
  'JAN.': 1, 'FEB.': 2, 'MÄRZ': 3, 'APRIL': 4, 'MAI': 5, 'JUNI': 6,
  'JULI': 7, 'AUG.': 8, 'SEP.': 9, 'OKT': 10, 'NOV': 11, 'DEZ.': 12,
}
export function parseWien(values: Buffer, locations: string) {
  const json = JSON.parse(locations), points = new Map<number, CityCoordinate>()
  if (json?.type !== 'FeatureCollection' || !Array.isArray(json.features)) throw new Error('invalid Wien station FeatureCollection')
  for (const feature of json.features) {
    const id = feature?.properties?.ZST_ID
    if (!Number.isInteger(id) || feature?.geometry?.type !== 'Point') throw new Error('invalid Wien station identity')
    if (points.has(id)) throw new Error(`duplicate Wien station ${id}`)
    points.set(id, coordinate(feature.geometry.coordinates))
  }
  if (points.size < 60) throw new Error('Wien source has fewer than60 station locations')
  const rows = parse(values.toString('latin1'), { delimiter: ';', columns: true, skip_empty_lines: true }) as Record<string, string>[]
  const required = ['JAHR', 'MONAT', 'ZNR', 'ZNAME', 'RINAME', 'FZTYP', 'DTVMS']
  if (!rows.length || required.some(name => !(name in rows[0]))) throw new Error('Wien CSV columns or records missing')
  type Month = { kfz?: number; lkw?: number }
  const stations = new Map<number, Map<number, Map<number, Month>>>(), names = new Map<number, string>()
  let invalidValuesSkipped = 0
  for (const row of rows) {
    if (row.RINAME !== 'Gesamt') continue
    const year = Number(row.JAHR), id = Number(row.ZNR), month = MONTH_INDEX[row.MONAT.trim()]
    if (!Number.isInteger(year) || !Number.isInteger(id) || month === undefined) throw new Error('invalid Wien observation identity/month')
    if (!row.DTVMS.trim()) continue
    const value = Number(row.DTVMS)
    if (!Number.isFinite(value) || value <= 0) { invalidValuesSkipped++; continue }
    names.set(id, row.ZNAME.trim())
    const years = stations.get(id) ?? new Map<number, Map<number, Month>>(); stations.set(id, years)
    const months = years.get(year) ?? new Map<number, Month>(); years.set(year, months)
    const cell = months.get(month) ?? {}; months.set(month, cell)
    if (row.FZTYP === 'Kfz') cell.kfz = value
    else if (row.FZTYP === 'LkwÄ') cell.lkw = value
  }
  const eligible = (year: number) => [...stations].filter(([, years]) => [...(years.get(year)?.values() ?? [])].filter(month => month.kfz !== undefined).length >= 10)
  const years = [...new Set([...stations.values()].flatMap(years => [...years.keys()]))].filter(year => !EXCLUDED_YEARS.has(year)).sort((a, b) => b - a)
  const year = years.find(year => eligible(year).length >= 50)
  if (year === undefined) throw new Error('Wien has no non-pandemic year with50 stations and10 monthly totals')
  let noGeometry = 0
  const records: CityRoadRecord[] = []
  for (const [id, years] of eligible(year)) {
    const point = points.get(id)
    if (!point) { noGeometry++; continue }
    let days = 0, totalSum = 0, heavySum = 0
    for (const [month, cell] of years.get(year)!) {
      if (cell.kfz === undefined) continue
      const monthDays = new Date(Date.UTC(year, month, 0)).getUTCDate()
      days += monthDays; totalSum += cell.kfz * monthDays; heavySum += (cell.lkw ?? 0) * monthDays
    }
    const total = totalSum / days, heavy = heavySum / days
    if (heavy > total) throw new Error(`Wien station ${id}: truck-like total exceeds all vehicles`)
    records.push({ street: names.get(id) ?? `ZNR ${id}`, light: Math.round(total - heavy), medium: 0,
      heavy: Math.round(heavy), moto: 0, line: [point] })
  }
  if (records.length < 50) throw new Error('Wien has fewer than50 stations after geometry join')
  return { year, noGeometry, invalidValuesSkipped, records }
}

export async function loadMunicipalRoadSources(enrichmentDirectory: string, boundaryDirectory: string) {
  const receipts: Array<{ path: string; bytes: number; sha256: string }> = []
  const read = (path: string): Buffer => {
    const before = statSync(path, { bigint: true }), bytes = readFileSync(path), after = statSync(path, { bigint: true })
    if (['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs'].some(key => before[key as keyof typeof before] !== after[key as keyof typeof after])) throw new Error(`municipal source changed while read: ${path}`)
    receipts.push({ path, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') })
    return bytes
  }
  const boundaries = new Map<string, string>()
  const cities = []
  for (const city of MUNICIPAL_ROAD_SOURCES) {
    const dataset = DATASETS.find(d => d.id === city.sourceId)!
    if (!dataset.roadCoverage?.length || dataset.year === null) throw new Error(`municipal registry policy missing: ${city.slug}`)
    const directory = resolve(enrichmentDirectory, city.country, `city-${city.slug}`)
    const parsed = city.slug === 'praha'
      ? { year: dataset.year, ...parsePrahaRows(await readSheet(read(resolve(directory, `intenzity-${dataset.year}.xlsx`)), PRAHA_SHEET)) }
      : city.slug === 'brno' ? parseBrno(read(resolve(directory, 'pentlogram-lines.geojson')).toString('utf8'))
        : parseWien(read(resolve(directory, 'dauerzaehlstellen-werte.csv')), read(resolve(directory, 'dauerzaehlstellen-standorte.json')).toString('utf8'))
    if (parsed.year !== dataset.year) throw new Error(`${city.slug}: selected year ${parsed.year} differs from registered source year ${dataset.year}`)
    const records = parsed.records.filter(record => {
      const values = [record.light, record.medium, record.heavy, record.moto]
      if (values.some(v => !Number.isSafeInteger(v) || v < 0 || v > 2147483647)) throw new Error(`${city.slug}: invalid rounded vehicle split`)
      return values.some(v => v > 0)
    })
    if (!records.length) throw new Error(`${city.slug}: no positive traffic observations`)
    let boundary = boundaries.get(city.iso3)
    if (!boundary) { boundary = read(resolve(boundaryDirectory, `geoBoundaries-${city.iso3}-ADM2.geojson`)).toString('utf8'); boundaries.set(city.iso3, boundary) }
    const { records: _records, ...admission } = parsed
    cities.push({ ...city, year: parsed.year, admission, coverage: new Set(dataset.roadCoverage), records,
      zeroSplitSkipped: parsed.records.length - records.length, municipality: municipalityFromGeoJson(boundary, city.municipality) })
  }
  return { cities, receipts }
}
