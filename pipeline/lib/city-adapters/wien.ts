/**
 * Wien — MA46 Dauerzählstellen (permanent traffic counters) adapter.
 *
 * Two files join on the station id (ZNR ↔ ZST_ID):
 *  - values: city CSV via the portal's stable go-link (today it redirects to
 *    wien.gv.at/data/ogd/ma46/dauerzaehlstellen.csv) — MONTHLY DTV per
 *    station × direction × vehicle type, 2016→current. DTVMS is the
 *    Mon–Sun all-days mean, i.e. MA46 already did the §2.6 day-type
 *    weighting; we only take the days-weighted mean of the 12 monthly
 *    values of the chosen year. `RINAME = "Gesamt"` rows are the
 *    bidirectional totals this pipeline stamps.
 *  - locations: OGD Wien WFS `ogdwien:DAUERZAEHLOGD`, point per station.
 *
 * Station names are landmarks ("Reichsbrücke", "Westbahnhof"), NOT street
 * names → records carry `line` (the station point) and the driver matches
 * by proximity only.
 *
 * Vehicle types: `Kfz` = ALL motor vehicles (verified: Kfz ≈ directions'
 * sum and ≈ published totals, so it includes trucks), `LkwÄ` =
 * "Lkw-ähnlich" (truck-like) → heavy. light = Kfz − LkwÄ; the 8+1
 * classifier folds buses and motorcycles into Kfz without a published
 * split → medium = moto = 0 (counted, not invented).
 *
 * STRTYP is only B (Hauptstraßen B) and G (Gemeindestraßen) — Vienna's
 * motorways are ASFINAG's, absent here (registry coverage excludes them).
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { execFileSync } from 'node:child_process'
import { replaceCacheFileAtomically } from '../atomic-cache.js'
import type { CityRecord } from './types.js'
import { DATA_YEAR as YEAR } from '../data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, '..', '..', '..', 'data', 'enrichment', YEAR, 'at', 'city-wien')
const VALUES_URL = 'https://www.wien.gv.at/gogv/l9ogddauerzaehlstellen'
const LOCATIONS_URL =
  'https://data.wien.gv.at/daten/geo?service=WFS&request=GetFeature&version=1.1.0' +
  '&typeName=ogdwien:DAUERZAEHLOGD&srsName=EPSG:4326&outputFormat=json'
const VALUES_PATH = resolve(CACHE_DIR, 'dauerzaehlstellen-werte.csv')
const LOCATIONS_PATH = resolve(CACHE_DIR, 'dauerzaehlstellen-standorte.json')
const PARSED_PATH = resolve(CACHE_DIR, 'dauerzaehlstellen-parsed-v1.json')

/** MONAT tokens as published (inconsistent abbreviation, verified 2026-06-12). */
const MONTH_INDEX: Record<string, number> = {
  'JAN.': 1, 'FEB.': 2, 'MÄRZ': 3, 'APRIL': 4, 'MAI': 5, 'JUNI': 6,
  'JULI': 7, 'AUG.': 8, 'SEP.': 9, 'OKT': 10, 'NOV': 11, 'DEZ.': 12,
}
/** Plan §2.6 rule 4: pandemic years never feed AADT. */
const EXCLUDED_YEARS = new Set([2020, 2021])
const MIN_MONTHS_PER_STATION = 10
const MIN_STATIONS = 50

function download(url: string, dest: string, enrichOnly: boolean): void {
  if (enrichOnly) throw new Error(`wien adapter: --enrich-only but no cache at ${dest}`)
  replaceCacheFileAtomically(dest, temporaryPath => {
    execFileSync('curl', ['-fsSL', '--max-time', '300', url, '-o', temporaryPath])
  })
}

export async function loadWien(opts: { enrichOnly: boolean; forceDownload: boolean }): Promise<CityRecord[]> {
  mkdirSync(CACHE_DIR, { recursive: true })
  if (existsSync(PARSED_PATH) && !opts.forceDownload) {
    // §2.6 rule 5: the year the AADT means come from is stamped in the cache.
    const cached: { year: number; records: CityRecord[] } = JSON.parse(readFileSync(PARSED_PATH, 'utf8'))
    console.log(`[wien] cache: year ${cached.year}, ${cached.records.length} stations`)
    return cached.records
  }
  if (!existsSync(VALUES_PATH) || opts.forceDownload) download(VALUES_URL, VALUES_PATH, opts.enrichOnly)
  if (!existsSync(LOCATIONS_PATH) || opts.forceDownload) download(LOCATIONS_URL, LOCATIONS_PATH, opts.enrichOnly)

  const wfs = JSON.parse(readFileSync(LOCATIONS_PATH, 'utf8'))
  const stationPoint = new Map<number, readonly [number, number]>()
  for (const f of wfs.features ?? []) {
    const id = f.properties?.ZST_ID
    const coords = f.geometry?.coordinates
    if (typeof id === 'number' && Array.isArray(coords)) stationPoint.set(id, [coords[0], coords[1]])
  }
  if (stationPoint.size < 60) {
    throw new Error(`wien adapter: only ${stationPoint.size} WFS stations (expected ~87) — service drift?`)
  }

  // values CSV is ISO-8859-1 with ';' delimiter; columns located by header
  // name so a column insertion can't silently shift the parse.
  const lines = readFileSync(VALUES_PATH).toString('latin1').split(/\r?\n/)
  const header = lines[0].split(';').map((h) => h.trim())
  const col = (name: string): number => {
    const i = header.indexOf(name)
    if (i < 0) throw new Error(`wien adapter: column "${name}" missing (header: ${header.join(',')})`)
    return i
  }
  const cJahr = col('JAHR'), cMonat = col('MONAT'), cZnr = col('ZNR'), cZname = col('ZNAME')
  const cRiname = col('RINAME'), cFztyp = col('FZTYP'), cDtvms = col('DTVMS')

  // znr → year → month → { kfz, lkw } (Gesamt = bidirectional total rows only)
  const byStation = new Map<number, Map<number, Map<number, { kfz?: number; lkw?: number }>>>()
  const stationName = new Map<number, string>()
  for (let i = 1; i < lines.length; i++) {
    const row = lines[i].split(';')
    if (row.length <= cDtvms || row[cRiname] !== 'Gesamt') continue
    const year = Number(row[cJahr])
    const month = MONTH_INDEX[row[cMonat].trim()]
    const znr = Number(row[cZnr])
    const dtvms = Number(row[cDtvms])
    if (!Number.isInteger(year) || !Number.isInteger(znr)) continue
    if (month === undefined) throw new Error(`wien adapter: unknown MONAT "${row[cMonat]}" — format drift?`)
    if (!Number.isFinite(dtvms) || dtvms <= 0) continue
    stationName.set(znr, row[cZname].trim())
    const years = byStation.get(znr) ?? new Map()
    byStation.set(znr, years)
    const months = years.get(year) ?? new Map()
    years.set(year, months)
    const cell = months.get(month) ?? {}
    months.set(month, cell)
    if (row[cFztyp] === 'Kfz') cell.kfz = dtvms
    else if (row[cFztyp] === 'LkwÄ') cell.lkw = dtvms
  }

  // Latest non-pandemic year where enough stations have ≥10 valid months
  // (§2.6 rules 2–5). A station's month is valid when its Kfz total parsed.
  const allYears = [...new Set([...byStation.values()].flatMap((y) => [...y.keys()]))]
    .filter((y) => !EXCLUDED_YEARS.has(y))
    .sort((a, b) => b - a)
  const stationsWithEnough = (year: number): number[] =>
    [...byStation.entries()]
      .filter(([, years]) => {
        const months = years.get(year)
        return months !== undefined && [...months.values()].filter((c) => c.kfz !== undefined).length >= MIN_MONTHS_PER_STATION
      })
      .map(([znr]) => znr)
  const usedYear = allYears.find((y) => stationsWithEnough(y).length >= MIN_STATIONS)
  if (usedYear === undefined) {
    throw new Error(`wien adapter: no year has ≥${MIN_STATIONS} stations with ≥${MIN_MONTHS_PER_STATION} months (years: ${allYears.join(',')})`)
  }

  const daysInMonth = (m: number): number => new Date(usedYear, m, 0).getDate()
  let noGeometry = 0
  const records: CityRecord[] = []
  for (const znr of stationsWithEnough(usedYear)) {
    const point = stationPoint.get(znr)
    if (!point) {
      noGeometry++
      continue
    }
    let days = 0, kfzSum = 0, lkwSum = 0
    for (const [month, cell] of byStation.get(znr)!.get(usedYear)!) {
      if (cell.kfz === undefined) continue
      const d = daysInMonth(month)
      days += d
      kfzSum += cell.kfz * d
      lkwSum += (cell.lkw ?? 0) * d
    }
    const kfz = kfzSum / days
    const lkw = Math.min(lkwSum / days, kfz) // LkwÄ ⊂ Kfz; clamp guards a data glitch
    records.push({
      street: stationName.get(znr) ?? `ZNR ${znr}`,
      aadtLight: Math.round(kfz - lkw),
      aadtMedium: 0,
      aadtHeavy: Math.round(lkw),
      aadtMoto: 0,
      line: [point],
    })
  }
  if (records.length < MIN_STATIONS) {
    throw new Error(`wien adapter: only ${records.length} stations survived the geometry join — WFS/CSV drift?`)
  }

  replaceCacheFileAtomically(PARSED_PATH, temporaryPath => {
    writeFileSync(temporaryPath, JSON.stringify({ year: usedYear, records }))
  })
  console.log(
    `[wien] year ${usedYear}: ${records.length} stations (${noGeometry} without geometry dropped; cache ${PARSED_PATH})`,
  )
  return records
}
