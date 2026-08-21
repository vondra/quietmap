/** LKPR TANOS monthly aircraft-noise adapter; fetches explicit public PDF URLs only. */
import { spawnSync } from 'node:child_process'
import { createWriteStream, existsSync, readFileSync, renameSync, rmSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { assertDir, openValidationDb, VALIDATION_DATA_DIR, writeSnapshot } from './lib.ts'

const NETWORK = 'lkpr-tanos'
const MONTHS: Record<string, number> = { leden: 1, unor: 2, brezen: 3, duben: 4, kveten: 5, cerven: 6, cervenec: 7, srpen: 8, zari: 9, rijen: 10, listopad: 11, prosinec: 12 }
const STATIONS = [
  ['MP01', 'Jeneč', 50.0889, 14.2103], ['MP02', 'Červený Újezd', 50.071, 14.1646],
  ['MP03', 'Unhošť', 50.0815, 14.1292], ['MP04', 'Pavlov', 50.0952, 14.1646],
  ['MP05', 'Hostivice', 50.0814, 14.2347], ['MP06', 'Dobrovíz', 50.1144, 14.2207],
  ['MP07', 'Kněževes', 50.1199, 14.2584], ['MP08', 'Horoměřice střed', 50.14, 14.3341],
  ['MP09', 'Praha - Přední Kopanina', 50.1172, 14.3014], ['MP10', 'Horoměřice', 50.1299, 14.3439],
  ['MP11', 'Roztoky', 50.1589, 14.3946], ['MP12', 'Praha 17 - Řepy', 50.0711, 14.3208],
  ['MP13', 'Suchdol', 50.1456, 14.3787], ['MP14', 'Malé Kyšice', 50.0588, 14.082],
] as const
type ParsedStation = { station_id: string; name: string; month: string; laeq_aircraft_day_0622: number; laeq_aircraft_night_2206: number | null }

/** Parse pdftotext -layout output, retaining the published aircraft-only monthly means. */
export function parseLkprMonthlyReport(text: string): ParsedStation[] {
  const blocks = text.split(/(?=^MP\d{2}\s)/m).slice(1)
  const parsed = blocks.map(block => {
    const header = /^MP(\d{2})\s+(.+)\n([^\n]+)$/m.exec(block)
    const average = /^\s*Měsíční průměr\s+(.+)$/m.exec(block)
    if (!header || !average) throw new Error('[lkpr] report layout changed: station header or monthly average missing')
    const [monthName, yearText] = header[3].trim().toLowerCase().normalize('NFD').replace(/\p{Diacritic}/gu, '').split(/\s+/)
    const monthNumber = MONTHS[monthName]
    const year = Number(yearText)
    if (!monthNumber || !Number.isInteger(year)) throw new Error(`[lkpr] unrecognised report month ${JSON.stringify(header[3])}`)
    const values = average[1].trim().split(/\s+/)
    if (values.length !== 6 || values.some(value => value !== '*' && !/^\d+,\d$/.test(value))) throw new Error(`[lkpr] MP${header[1]}: expected six one-decimal total/aircraft/non-aircraft values`)
    const numeric = (value: string) => value === '*' ? null : Number(value.replace(',', '.'))
    const day = numeric(values[2])
    if (day == null) throw new Error(`[lkpr] MP${header[1]}: aircraft day monthly mean is unavailable`)
    return { station_id: `mp${header[1]}`, name: header[2].trim(), month: `${year}-${String(monthNumber).padStart(2, '0')}`, laeq_aircraft_day_0622: day, laeq_aircraft_night_2206: numeric(values[3]) }
  })
  if (parsed.length !== STATIONS.length) throw new Error(`[lkpr] expected ${STATIONS.length} fixed stations, parsed ${parsed.length}`)
  for (const [index, station] of parsed.entries()) {
    if (station.station_id !== `mp${String(index + 1).padStart(2, '0')}` || station.name !== STATIONS[index][1]) throw new Error(`[lkpr] station order/name drift at ${station.station_id}: ${station.name}`)
  }
  return parsed
}

if (process.argv[1] && resolve(process.argv[1]) === import.meta.filename) {
const args = process.argv.slice(2)
const valueFor = (name: string) => {
  const index = args.indexOf(name)
  if (index === -1) return undefined
  if (!args[index + 1] || args[index + 1].startsWith('--')) throw new Error(`[lkpr] ${name} requires a value`)
  return args[index + 1]
}
const inputText = valueFor('--input-text')
const url = valueFor('--url')
const sourceUrl = valueFor('--source-url')
if (!inputText && !url) {
  console.error('usage: npx tsx pipeline/validation/snapshot-lkpr.ts --url <published monthly PDF> | --input-text <offline pdftotext fixture>')
  process.exit(2)
}
let text: string
if (inputText) text = readFileSync(resolve(inputText), 'utf8')
else {
  const pdfPath = assertDir(resolve(VALIDATION_DATA_DIR, 'lkpr', `${safeUrlFilename(url!)}.pdf`))
  if (!existsSync(pdfPath) || statSync(pdfPath).size === 0) {
    const response = await fetch(url!, { signal: AbortSignal.timeout(120000) })
    if (!response.ok || !response.body) throw new Error(`[lkpr] ${url}: HTTP ${response.status}`)
    const temporary = `${pdfPath}.tmp-${process.pid}`
    try { await pipeline(Readable.fromWeb(response.body as never), createWriteStream(temporary, { flags: 'wx' })); renameSync(temporary, pdfPath) }
    finally { rmSync(temporary, { force: true }) }
  }
  const extract = spawnSync('pdftotext', ['-layout', pdfPath, '-'], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
  if (extract.status !== 0) throw new Error(`[lkpr] pdftotext failed: ${extract.stderr}`)
  text = extract.stdout
}
function safeUrlFilename(value: string) { return Buffer.from(value).toString('base64url').slice(0, 80) }
const rows = parseLkprMonthlyReport(text)
const [year, month] = rows[0].month.split('-').map(Number)
if (rows.some(row => row.month !== rows[0].month)) throw new Error('[lkpr] report contains mixed months')
const db = openValidationDb()
db.exec('CREATE TABLE IF NOT EXISTS monthly_value (network TEXT NOT NULL, station_id TEXT NOT NULL, month TEXT NOT NULL, metric TEXT NOT NULL, value REAL, meta_json TEXT, PRIMARY KEY (network, station_id, month, metric))')
const stationUpsert = db.prepare('INSERT OR REPLACE INTO station (network, station_id, name, lat, lng, meta_json) VALUES (?,?,?,?,?,?)')
const monthlyUpsert = db.prepare('INSERT OR REPLACE INTO monthly_value (network, station_id, month, metric, value, meta_json) VALUES (?,?,?,?,?,?)')
db.exec('BEGIN')
for (const [index, row] of rows.entries()) {
  const [, name, lat, lng] = STATIONS[index]
  stationUpsert.run(NETWORK, row.station_id, name, lat, lng, JSON.stringify({ siting: 'official NMT map georeference; approximately 150–300 m uncertainty' }))
  for (const [metric, value] of Object.entries({ laeq_aircraft_day_0622: row.laeq_aircraft_day_0622, laeq_aircraft_night_2206: row.laeq_aircraft_night_2206 })) monthlyUpsert.run(NETWORK, row.station_id, row.month, metric, value, JSON.stringify({ report: url ?? inputText }))
}
db.exec('COMMIT'); db.close()
const path = writeSnapshot({ schema_version: 2, network: NETWORK, country_code: 'CZ', year, license: 'Letiště Praha public TANOS monthly report; cite the report URL', source: [url ?? sourceUrl ?? `offline fixture ${inputText}`], fetched_at: new Date().toISOString(), mode: 'source:aircraft', anchor_type: 'measurement', regime: 'aircraft', tags: [], comparison_mode: 'trend_only', comparison_tolerance_db: null, comparison_tolerance_basis: null, measured_metric_field: 'laeq_aircraft_day_0622', model_metric_field: 'lden', commensurability: { metric_variant: 'laeq_windows', dominance: 'event_classified', receiver_convention: 'nmt_pole', coord_uncertainty_m: 300, note: 'Published monthly aircraft-only LAeq,T day 06–22; monthly report explicitly says it is not comparable to statutory characteristic-flight-day limits. Model Lden Δ is descriptive only.' }, method: 'pdftotext -layout; strict MP01–MP14 header and six-column monthly-average parse; stores aircraft-only day/night metrics without conversion.', stations: rows.map((row, index) => ({ ...row, lat: STATIONS[index][2], lng: STATIONS[index][3], report_month: month })) })
console.error(`[lkpr] snapshot: ${rows.length}/14 stations × ${rows[0].month} → ${path}`)
}
