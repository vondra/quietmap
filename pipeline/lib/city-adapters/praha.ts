/**
 * Praha — TSK-ÚDI "Intenzity dopravy v Praze" adapter.
 *
 * Source: direct XLSX from TSK's CDN (no auth), the city's official
 * monitored-network counts: 954 bidirectional section profiles on 415
 * streets, WHOLE administrative Prague, average WORKING DAY 0–24 h
 * (title row says "pracovní den, 0-24 h" — the older 06–22 h caveat in the
 * audit referred to the data.praha.eu cordon dashboard, a different
 * series). Mix of permanent loops, mobile counters and manual surveys —
 * the official measurement program with no modeled gap-fill rows (recon
 * 2026-06-12, Legerova section verified at 45,545 total / 45,500 excl.
 * PID buses = the audit figure).
 *
 * No geometry in the file (node ids + street + cross-street names only) →
 * the driver matches by normalized street NAME inside the Prague polygon.
 * Prague street names are unique citywide (municipal decree), so a name
 * join inside the gate is exact; streets with multiple sections get a
 * length-weighted mean (one value per street — section placement without
 * geometry would be guesswork).
 *
 * CNOSSOS bucket mapping: cars → light; "pomalá vozidla" (HGV + coaches +
 * tractors) → heavy; PID buses → medium (urban buses ≈ CNOSSOS cat 2);
 * moto not counted by TSK → 0. Working-day total is kept as the AADT
 * proxy UNSCALED (TSK's TP-189 weekday→annual factors are not published
 * with the file; inventing one would fabricate data) — which is exactly
 * why the registry marks this dataset `measurement: 'derived'`, keeping
 * it out of counted-only consumers while the popup still shows it as the
 * city-measured source it is.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { execFileSync } from 'node:child_process'
import * as XLSX from 'xlsx'
import { replaceCacheFileAtomically } from '../atomic-cache.js'
import type { CityRecord } from './types.js'
import { DATA_YEAR as YEAR } from '../data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, '..', '..', '..', 'data', 'enrichment', YEAR, 'cz', 'city-praha')
const XLSX_URL = 'https://cdn.tsk-praha.cz/portal/2026/05/Intenzity-dopravy-v-Praze_2025.xlsx'
const XLSX_PATH = resolve(CACHE_DIR, 'intenzity-2025.xlsx')
const PARSED_PATH = resolve(CACHE_DIR, 'intenzity-2025-parsed-v1.json')
const SHEET = 'Úseky - po profilech - dle uzlů'

export async function loadPraha(opts: { enrichOnly: boolean; forceDownload: boolean }): Promise<CityRecord[]> {
  mkdirSync(CACHE_DIR, { recursive: true })
  if (existsSync(PARSED_PATH) && !opts.forceDownload) {
    return JSON.parse(readFileSync(PARSED_PATH, 'utf8'))
  }
  if (!existsSync(XLSX_PATH) || opts.forceDownload) {
    if (opts.enrichOnly) {
      throw new Error(`praha adapter: --enrich-only but no cache at ${XLSX_PATH}`)
    }
    replaceCacheFileAtomically(XLSX_PATH, temporaryPath => {
      execFileSync('curl', ['-fsSL', '--max-time', '300', XLSX_URL, '-o', temporaryPath])
    })
  }

  const wb = XLSX.read(readFileSync(XLSX_PATH), { type: 'buffer' })
  const ws = wb.Sheets[SHEET]
  if (!ws) throw new Error(`praha adapter: sheet "${SHEET}" missing (got: ${wb.SheetNames.join(', ')})`)
  const rows: unknown[][] = XLSX.utils.sheet_to_json(ws, { header: 1 })

  // Aggregate sections per street: length-weighted mean of each bucket.
  type Acc = { len: number; light: number; medium: number; heavy: number }
  const byStreet = new Map<string, Acc>()
  let sections = 0
  for (const r of rows) {
    // Data rows have numeric node ids in A+B; everything else is headers.
    const [u1, u2, street, , , lenM, cars, slow, , busPid] = r
    if (typeof u1 !== 'number' || typeof u2 !== 'number' || typeof street !== 'string') continue
    const len = typeof lenM === 'number' && lenM > 0 ? lenM : 1
    const carsN = typeof cars === 'number' ? cars : 0
    const slowN = typeof slow === 'number' ? slow : 0
    const busN = typeof busPid === 'number' ? busPid : 0
    if (carsN + slowN + busN <= 0) continue
    sections++
    const key = street.trim()
    const acc = byStreet.get(key) ?? { len: 0, light: 0, medium: 0, heavy: 0 }
    acc.len += len
    acc.light += carsN * len
    acc.medium += busN * len
    acc.heavy += slowN * len
    byStreet.set(key, acc)
  }
  if (sections < 500) {
    // 954 expected; a format change must fail loud, not stamp a remnant.
    throw new Error(`praha adapter: only ${sections} sections parsed (expected ~954) — format drift?`)
  }

  // TSK truncates long names to a fixed-width column — expand the loud ones
  // to their OSM names (hand-verified; unmapped truncations stay unmatched
  // and are listed by the driver's match-rate log; motorways are deliberately
  // ABSENT — D-roads keep the ŘSD national census, see registry coverage).
  const OSM_NAME: Record<string, string> = {
    'BARRAND.MOST': 'Barrandovský most',
    'DEJVICKÝ T.': 'Dejvický tunel',
    'BRUSNICKÝ T.': 'Brusnický tunel',
    'STRAH.TUNEL': 'Strahovský tunel',
    'BUBENEČ.TUN.': 'Bubenečský tunel',
    'TUN.MRÁZOVKA': 'Tunel Mrázovka',
    'TĚŠNOVSKÝ T.': 'Těšnovský tunel',
    'V HOLEŠOVIČ.': 'V Holešovičkách',
    '5.KVĚTNA': '5. května',
    'ŠTĚRB.SPOJKA': 'Štěrboholská spojka',
    'NUSEL.MOST': 'Nuselský most',
    'MOST BARIK.': 'most Barikádníků',
    'ROZVAD.SPOJ.': 'Rozvadovská spojka',
    'JIRÁSK.MOST': 'Jiráskův most',
    'ROHAN.NÁBŘ.': 'Rohanské nábřeží',
    'NÁB.K.JAROŠE': 'nábřeží Kapitána Jaroše',
    'NÁB.L.SVOB.': 'nábřeží Ludvíka Svobody',
    'BUBEN.NÁBŘ.': 'Bubenské nábřeží',
    'POD KREJCÁR.': 'Pod Krejcárkem',
    'BĚLOCERKEVS.': 'Bělocerkevská',
    'J.ŽELIVSKÉHO': 'Jana Želivského',
    'ČERNOKOSTEL.': 'Černokostelecká',
    'N.POVLTAVSKÁ': 'Nová Povltavská',
  }
  const records: CityRecord[] = [...byStreet.entries()].map(([street, a]) => ({
    street: OSM_NAME[street] ?? street,
    aadtLight: Math.round(a.light / a.len),
    aadtMedium: Math.round(a.medium / a.len),
    aadtHeavy: Math.round(a.heavy / a.len),
    aadtMoto: 0,
  }))
  replaceCacheFileAtomically(PARSED_PATH, temporaryPath => {
    writeFileSync(temporaryPath, JSON.stringify(records))
  })
  console.log(`[praha] parsed ${sections} sections → ${records.length} streets (cache ${PARSED_PATH})`)
  return records
}
