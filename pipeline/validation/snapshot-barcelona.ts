/**
 * Leg A adapter: Barcelona Xarxa de Monitoratge del Soroll (Open Data BCN,
 * CC BY 4.0). The portal publishes RAW 1-minute LAeq per installation
 * (monthly ZIPs, ~280 MB / 2.1 GB CSV each) plus an installation registry
 * with coordinates and the dominant local source (Font). We stream each
 * month straight out of `unzip -p` and reduce to daily END-period energy
 * sums in SQLite — true annual Ld/Le/Ln/Lden per station computed from
 * primary data (no metric conversion), coverage tracked per minute counts.
 *
 * Run (annual, agent-run — no cron until 2 stable manual runs):
 *   npx tsx pipeline/validation/snapshot-barcelona.ts --year 2025
 * Re-runs reuse cached ZIPs under data/validation/barcelona/zips/. Partial
 * portal years work — stations below the rule-1 thresholds stay in SQLite
 * and out of the snapshot.
 */
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { createWriteStream, existsSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { replaceCacheFileAtomicallyAsync } from '../lib/atomic-cache.js'
import {
  boundedCoveragePercent,
  assertDir, ckanResources, endPeriodForLocalHour, energeticMeanDb, ldenFromPeriods,
  openValidationDb, PERIOD_MINUTES_PER_DAY, splitCsvLine, VALIDATION_DATA_DIR, writeSnapshot,
  type EndPeriod,
} from './lib.ts'

const NETWORK = 'barcelona-xarxa-soroll'
const PORTAL = 'https://opendata-ajuntament.barcelona.cat/data'
const DATA_PACKAGE = 'xarxasoroll-equipsmonitor-dades'
const STATIONS_PACKAGE = 'xarxasoroll-equipsmonitor-instal'
const ZIP_DIR = resolve(VALIDATION_DATA_DIR, 'barcelona/zips')
// A station-year is snapshot-worthy only with ≥9 covered months and ≥70 % of
// expected minutes in EVERY END period — plan rule 1 (annual averages; short
// coverage never anchors). Per-period, not total: 90 % day + 20 % night would
// pass a total gate while biasing Lden through an unrepresentative Ln.
// A month counts as covered only when it carries ≥50 % of its minutes.
const MIN_MONTHS = 9
const MIN_PERIOD_COVERAGE = 0.7
const MONTH_COVERED_FRACTION = 0.5
// Rows the header promised but the line didn't deliver: tolerate portal
// hiccups up to 1 %, refuse to ship a snapshot silently built on less.
const MAX_BAD_ROW_SHARE = 0.01

const year = Number(process.argv[process.argv.indexOf('--year') + 1] || NaN)
if (!Number.isInteger(year)) {
  console.error('usage: npx tsx pipeline/validation/snapshot-barcelona.ts --year <YYYY>')
  process.exit(2)
}

// ── 1. Resolve portal resources ─────────────────────────────────────────────

const dataResources = await ckanResources(PORTAL, DATA_PACKAGE)
const monthly = dataResources
  .filter((r) => r.name.startsWith(`${year}_`))
  .sort((a, b) => a.name.localeCompare(b.name))
if (monthly.length === 0) {
  console.error(`[barcelona] no monthly resources for ${year} on the portal — published years: ${[...new Set(dataResources.map((r) => r.name.slice(0, 4)))].join(', ')}`)
  process.exit(2)
}
console.error(`[barcelona] year ${year}: ${monthly.length} monthly resources`)

const stationResources = await ckanResources(PORTAL, STATIONS_PACKAGE)
const stationsCsvUrl = stationResources.find((r) => r.format.toUpperCase() === 'CSV')?.url
if (!stationsCsvUrl) throw new Error('[barcelona] installation registry CSV not found on the portal')

// ── 2. Installation registry → station table ───────────────────────────────

type Station = { id: string; name: string; lat: number; lng: number; font: string; district: string; installed: string; deinstalled: string }
// Only exact catalog equivalences are tagged. In particular, portal Font
// `TRÀNSIT` does not establish a road class and `OCI` is broader than
// nightlife, so neither is guessed into a narrower model factor.
const factorTagsForFont = (font: string): string[] =>
  font === 'ZONA DE VIANANTS' ? ['pedestrian_zone'] : []
const stations = new Map<string, Station>()
{
  const r = await fetch(stationsCsvUrl, { signal: AbortSignal.timeout(60000) })
  if (!r.ok) throw new Error(`[barcelona] stations CSV: HTTP ${r.status}`)
  const text = (await r.text()).replace(/^﻿/, '')
  const lines = text.split('\n').filter((l) => l.trim())
  const header = splitCsvLine(lines[0])
  const col = (name: string) => {
    const i = header.indexOf(name)
    if (i === -1) throw new Error(`[barcelona] stations CSV lost column ${name} — header: ${header.join(',')}`)
    return i
  }
  const [cId, cVia, cCarrer, cNum, cDistricte, cLat, cLng, cInst, cDeinst, cFont] = [
    col('Id_Instal'), col('Tipus_Via'), col('Nom_Carrer'), col('Num_Carrer'),
    col('Nom_Districte'), col('Latitud'), col('Longitud'),
    col('Data_Instalacio'), col('Data_DesInstalacio'), col('Font'),
  ]
  for (const line of lines.slice(1)) {
    const f = splitCsvLine(line)
    stations.set(f[cId], {
      id: f[cId],
      name: `${f[cVia]} ${f[cCarrer]} ${f[cNum]}`.trim(),
      lat: Number(f[cLat]),
      lng: Number(f[cLng]),
      font: f[cFont],
      district: f[cDistricte],
      installed: f[cInst],
      deinstalled: f[cDeinst],
    })
  }
  console.error(`[barcelona] installation registry: ${stations.size} installations`)
}

// ── 3. Stream monthly ZIPs → daily END-period energy sums ──────────────────

// acc: station → date → period → [energySum, minutes]
const acc = new Map<string, Map<string, Record<string, [number, number]>>>()
let badRows = 0
let totalRows = 0

async function downloadIfMissing(url: string, name: string): Promise<string> {
  const path = assertDir(resolve(ZIP_DIR, `${name}.zip`))
  if (existsSync(path) && statSync(path).size > 0) return path
  console.error(`[barcelona] downloading ${name}…`)
  const r = await fetch(url, { signal: AbortSignal.timeout(1800000) })
  if (!r.ok || !r.body) throw new Error(`[barcelona] ${name}: HTTP ${r.status}`)
  await replaceCacheFileAtomicallyAsync(path, async temporaryPath => {
    await pipeline(Readable.fromWeb(r.body as never), createWriteStream(temporaryPath))
  })
  return path
}

for (const res of monthly) {
  const zipPath = await downloadIfMissing(res.url, res.name)
  const t0 = performance.now()
  let rows = 0
  const unzip = spawn('unzip', ['-p', zipPath], { stdio: ['ignore', 'pipe', 'inherit'] })
  const rl = createInterface({ input: unzip.stdout, crlfDelay: Infinity })
  let header: string[] | null = null
  let cLocal = -1, cId = -1, cLevel = -1
  for await (const line of rl) {
    if (!header) {
      header = splitCsvLine(line.replace(/^﻿/, ''))
      cLocal = header.indexOf('Timestamp_local')
      cId = header.indexOf('Id_Instal')
      cLevel = header.findIndex((h) => h.startsWith('Nivell_LAeq'))
      if (cLocal === -1 || cId === -1 || cLevel === -1) throw new Error(`[barcelona] ${res.name}: unexpected header ${line}`)
      continue
    }
    // Timestamp_local: 2026-04-01T00:01:18+02:00 — date = chars 0..10, hour 11..13.
    const f = line.split(',')
    const ts = f[cLocal]
    const level = Number(f[cLevel])
    if (!ts || ts.length < 13 || !Number.isFinite(level)) { badRows++; continue }
    const date = ts.slice(0, 10)
    if (!date.startsWith(String(year))) continue // month files spill a few UTC-shifted rows into neighbours
    const period = endPeriodForLocalHour(Number(ts.slice(11, 13)))
    const id = f[cId]
    let byDate = acc.get(id)
    if (!byDate) acc.set(id, (byDate = new Map()))
    let byPeriod = byDate.get(date)
    if (!byPeriod) byDate.set(date, (byPeriod = { ld: [0, 0], le: [0, 0], ln: [0, 0] }))
    const slot = byPeriod[period]
    slot[0] += 10 ** (level / 10)
    slot[1] += 1
    rows++
  }
  const exit = await new Promise<number>((res2) => unzip.on('close', res2))
  if (exit !== 0) throw new Error(`[barcelona] unzip ${res.name} exited ${exit}`)
  if (rows === 0) throw new Error(`[barcelona] ${res.name}: 0 usable rows — portal layout drift, refusing a silently empty month`)
  totalRows += rows
  console.error(`[barcelona] ${res.name}: ${rows} rows in ${Math.round((performance.now() - t0) / 1000)} s`)
}
if (badRows > totalRows * MAX_BAD_ROW_SHARE) {
  throw new Error(`[barcelona] ${badRows}/${totalRows} malformed rows (> ${MAX_BAD_ROW_SHARE * 100} %) — portal format drift, refusing to ship`)
}
if (badRows) console.error(`[barcelona] skipped ${badRows} malformed rows`)

// ── 4. Persist daily layer + annual rollup ──────────────────────────────────

const db = openValidationDb()
db.exec('BEGIN')
// Scope-delete before import: INSERT OR REPLACE alone would keep rows the
// portal has since retracted (a re-run must converge to the portal's truth).
db.prepare('DELETE FROM daily_period WHERE network = ? AND date LIKE ?').run(NETWORK, `${year}-%`)
db.prepare('DELETE FROM annual_value WHERE network = ? AND year = ?').run(NETWORK, year)
const upStation = db.prepare('INSERT OR REPLACE INTO station (network, station_id, name, lat, lng, meta_json) VALUES (?,?,?,?,?,?)')
const upDaily = db.prepare('INSERT OR REPLACE INTO daily_period (network, station_id, date, period, energy_sum, minutes) VALUES (?,?,?,?,?,?)')
const upAnnual = db.prepare('INSERT OR REPLACE INTO annual_value (network, station_id, year, metric, value, meta_json) VALUES (?,?,?,?,?,?)')

const daysInYear = (year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0)) ? 366 : 365
const daysInMonth = (ym: string) => new Date(Number(ym.slice(0, 4)), Number(ym.slice(5, 7)), 0).getDate()

const snapshotStations = []
for (const [id, byDate] of acc) {
  const st = stations.get(id)
  if (!st) { console.error(`[barcelona] data for unknown installation ${id} — registry drift, skipped`); continue }
  const total: Record<EndPeriod, [number, number]> = { ld: [0, 0], le: [0, 0], ln: [0, 0] }
  const monthMinutes = new Map<string, number>()
  for (const [date, byPeriod] of byDate) {
    for (const period of ['ld', 'le', 'ln'] as const) {
      const [e, n] = byPeriod[period]
      if (n > 0) upDaily.run(NETWORK, id, date, period, e, n)
      total[period][0] += e
      total[period][1] += n
      monthMinutes.set(date.slice(0, 7), (monthMinutes.get(date.slice(0, 7)) ?? 0) + n)
    }
  }
  const monthsCovered = [...monthMinutes.entries()]
    .filter(([ym, min]) => min >= daysInMonth(ym) * 1440 * MONTH_COVERED_FRACTION).length
  const periodCoverage = (['ld', 'le', 'ln'] as const)
    .map((p) => total[p][1] / (PERIOD_MINUTES_PER_DAY[p] * daysInYear))
  for (const c of periodCoverage) {
    if (c > 1.005) throw new Error(`[barcelona] ${id}: period coverage ${(c * 100).toFixed(1)} % > 100 % — duplicate rows across month files, refusing to ship`)
  }
  const minCoverage = Math.min(...periodCoverage)
  const ld = energeticMeanDb(...total.ld)
  const le = energeticMeanDb(...total.le)
  const ln = energeticMeanDb(...total.ln)
  const lden = ldenFromPeriods(ld, le, ln)
  upStation.run(NETWORK, id, st.name, st.lat, st.lng, JSON.stringify({ font: st.font, district: st.district, installed: st.installed, deinstalled: st.deinstalled }))
  const coveragePct = boundedCoveragePercent(minCoverage)
  const meta = JSON.stringify({ min_period_coverage_pct: coveragePct, months_covered: monthsCovered })
  for (const [metric, value] of Object.entries({ ld, le, ln, lden })) {
    if (value != null) upAnnual.run(NETWORK, id, year, metric, +value.toFixed(2), meta)
  }
  if (lden == null || monthsCovered < MIN_MONTHS || minCoverage < MIN_PERIOD_COVERAGE) {
    console.error(`[barcelona] ${id} (${st.name}): ${monthsCovered} covered mo, min period coverage ${(minCoverage * 100).toFixed(0)} % — kept in SQLite, EXCLUDED from snapshot (rule 1)`)
    continue
  }
  snapshotStations.push({
    station_id: id, name: st.name, lat: st.lat, lng: st.lng,
    tags: factorTagsForFont(st.font),
    font: st.font, district: st.district,
    ld: +ld!.toFixed(1), le: +le!.toFixed(1), ln: +ln!.toFixed(1), lden: +lden.toFixed(1),
    months_covered: monthsCovered, coverage_pct: coveragePct,
  })
}
if (snapshotStations.length === 0) throw new Error('[barcelona] 0 stations pass the coverage gate — refusing a silently empty snapshot')
db.exec('COMMIT')
db.close()

snapshotStations.sort((a, b) => a.station_id.localeCompare(b.station_id, undefined, { numeric: true }))
const path = writeSnapshot({
  schema_version: 2,
  network: NETWORK,
  country_code: 'ES',
  year,
  license: 'CC BY 4.0 — Ajuntament de Barcelona, Open Data BCN',
  source: [`${PORTAL}/dataset/${DATA_PACKAGE}`, `${PORTAL}/dataset/${STATIONS_PACKAGE}`],
  fetched_at: new Date().toISOString(),
  mode: 'total',
  anchor_type: 'measurement',
  regime: 'mixed',
  tags: ['dense_urban'],
  comparison_mode: 'upper_bound',
  comparison_tolerance_db: 2,
  comparison_tolerance_basis: 'Project diagnostic +2 dB upper allowance for annual aggregation and receiver siting; not a measurement confidence interval.',
  measured_metric_field: 'lden',
  model_metric_field: 'lden',
  commensurability: {
    metric_variant: 'period_split',
    dominance: 'total_ambient',
    receiver_convention: 'nmt_pole',
    coord_uncertainty_m: 15,
    note: 'True END Ld/Le/Ln computed from portal 1-minute LAeq over local time; Lden per END weighting. Street mics hear everything — asymmetric rule: model_total ≤ measured + U only.',
  },
  method: 'stream unzip -p over monthly 1-min CSVs → daily END-period energy sums (SQLite daily_period) → annual energetic means',
  stations: snapshotStations,
})
console.error(`[barcelona] snapshot: ${snapshotStations.length} stations → ${path}`)
