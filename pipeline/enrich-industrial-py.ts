/**
 * Enrich PY industrial with GEM (Paraguay-flagged + cross-border hydro).
 *
 * Paraguay is **99% hydropower-generated** (~30 TWh/year) but generates
 * roughly 20-25× its own consumption — massive electricity exporter. All
 * the heavy lifting is done by two binational dams:
 *
 *   - **Itaipú** (14 GW, Paraguay/Brazil, on Río Paraná at Ciudad del Este
 *     / Foz do Iguaçu) — world's 2nd largest hydro plant. Flagged as
 *     Country_area='Brazil' in GEM but the dam straddles the border.
 *     Paraguay owns 50% of the power; consumes ~10%, exports ~90% to Brazil.
 *
 *   - **Yacyretá** (3.1 GW, Paraguay/Argentina, on Río Paraná at Encarnación
 *     / Posadas). Flagged as Country_area='Argentina' in GEM. Paraguay owns
 *     50%; consumes small share, exports most to Argentina.
 *
 *   - **Acaray** (210 MW + 48 MW expansion, Alto Paraná, 100% Paraguay)
 *
 * Other GEM Paraguay entries are tiny (Frigorífico Guaraní 1 MW, Filadelfia
 * 1 MW, Paracel 220 MW pre-construction, PASH/ERIH solar announced).
 *
 * Non-power industrial is minimal:
 *   - INC Vallemí cement (Concepción)
 *   - Cervepar Pilsen brewery (Itauguá)
 *   - Cargill/ADM soy processors (Asunción, Minga Guazú, Villeta)
 *   - No refinery (PETROPAR imports via pipeline from Brazil)
 *   - No steel, mining, chemicals
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-py.ts
 */

import { readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { DEFAULT_FUEL_TO_NACE, NATIONAL_MIX, stampOneWinner } from './lib/enrich-industrial-gem.js'
import type { MatchFacility } from './lib/facility-match.js'
import { inBbox } from './lib/spatial.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/py`)

// Paraguay bbox
const PY_BBOX: [number, number, number, number] = [-27.7, -62.7, -19.3, -54.2]

// Don't over-exclude neighbours since Paraguay is narrow and the two
// mega-dams are exactly on the borders
const EXCLUDE_ZONES: Array<[number, number, number, number]> = [
  // Brazil (NE) — east of -55 and north of -22
  [-22.0, -54.5, -19.3, -53.0],
  // Argentina (SW) — south of -27.5 west of -58
  [-28.0, -62.7, -27.5, -57.0],
  // Bolivia (NW Chaco) — north of -21
  [-21.0, -62.7, -19.3, -57.5],
]

// Expand bbox slightly to include Itaipú + Yacyretá border plants
function inPyOrBorder(lat: number, lon: number): boolean {
  if (lat < PY_BBOX[0] || lat > PY_BBOX[2] || lon < PY_BBOX[1] || lon > PY_BBOX[3]) return false
  // Only exclude strict exclusion zones, not Paraguay mainland
  for (const b of EXCLUDE_ZONES) {
    if (lat >= b[0] && lat <= b[2] && lon >= b[1] && lon <= b[3]) return false
  }
  return true
}

interface IndSite { lat: number; lon: number; name: string; fuel: string }

function loadPlants(file: string): { sites: IndSite[]; parsedInArea: number } {
  const path = resolve(CACHE_DIR, file)
  if (!existsSync(path)) return { sites: [], parsedInArea: 0 }
  const fc = JSON.parse(readFileSync(path, 'utf-8'))
  const out: IndSite[] = []
  let parsedInArea = 0 // pre-status parse count (coords valid) — feeds datasetNonEmpty
  for (const f of fc.features || []) {
    const g = f.geometry
    if (!g || g.type !== 'Point') continue
    const [lon, lat] = g.coordinates || []
    if (lat == null || lon == null) continue
    parsedInArea++
    const p = f.properties || {}
    const status = (p.Status || '').toString().toLowerCase()
    if (!status.includes('operating')) continue
    const name = (p.Plant___Project_name || 'PY plant').toString()
    // For border file, explicitly include Itaipú and Yacyretá
    const isCrossBorder = /itaip|yacyret|acaray/i.test(name)
    if (!inPyOrBorder(lat, lon) && !isCrossBorder) continue
    // For false positives (Finland Savitaipale), skip
    if (!/itaip|yacyret|acaray|paraguay|asunci|paraná|paran|ciudad del este|encarnaci|alto paraná/i.test(name) && !inPyOrBorder(lat, lon)) continue
    out.push({
      lat, lon,
      name,
      fuel: (p.Type || 'unknown').toString().toLowerCase(),
    })
  }
  return { sites: out, parsedInArea }
}

async function main() {
  console.log(`=== PY Industrial Enrichment — GEM + cross-border hydro (${YEAR}) ===\n`)

  const pyLoad = loadPlants('power-plants-gem-py.geojson')
  const borderLoad = loadPlants('power-plants-gem-border.geojson')
  // BOTH files are load-bearing: with the border file missing, the domestic
  // facilities still trigger the country-wide reset and the old Itaipú /
  // Yacyretá stamps could never be restated (round-3 Codex). Same fatal
  // pattern as CO: refuse to reset what this run cannot replace.
  for (const [file, load] of [['power-plants-gem-py.geojson', pyLoad], ['power-plants-gem-border.geojson', borderLoad]] as const) {
    if (load.parsedInArea === 0) {
      console.error(`FATAL: ${file} missing/empty — refusing to reset stamps this run cannot replace`)
      process.exit(1)
    }
  }
  const pyPlants = pyLoad.sites
  const borderPlants = borderLoad.sites

  // Merge by coordinate (Acaray may be in both)
  const seen = new Set<string>()
  const allPlants: IndSite[] = []
  for (const s of [...pyPlants, ...borderPlants]) {
    const key = `${s.lat.toFixed(4)}_${s.lon.toFixed(4)}`
    if (seen.has(key)) continue
    seen.add(key)
    allPlants.push(s)
  }
  console.log(`  GEM operating plants (PY + cross-border): ${allPlants.length}`)
  for (const p of allPlants) {
    console.log(`    ${p.fuel.padEnd(12)} ${p.name.substring(0, 50)} @ (${p.lat.toFixed(2)}, ${p.lon.toFixed(2)})`)
  }

  const facilities: MatchFacility[] = []
  for (const p of allPlants) {
    const nace4 = DEFAULT_FUEL_TO_NACE(p.fuel) // wind/blank → null → skip
    if (nace4 == null) continue
    facilities.push({ lat: p.lat, lon: p.lon, nace4, ...NATIONAL_MIX })
  }
  // The old carpet loop had NO per-row country gate — every polygon in the PY bbox
  // (including Brazilian/Argentine ones across the Paraná) could inherit a stamp.
  // The core gates candidates AND the reset via isInside = inPyOrBorder: that is
  // the intended fix, not an omission.
  await stampOneWinner({
    facilities,
    isInside: inPyOrBorder,
    // hexGate: plain bbox — a border hex centred in an EXCLUDE_ZONE still holds in-PY rows whose old stamps must be swept
    hexGate: (la, lo) => inBbox(la, lo, PY_BBOX),
    searchRadiusM: 3000,
    resetSourceIds: [NATIONAL_MIX.id],
    countryGate: makeCountryGate('PY'),
    datasetNonEmpty: true, // both files hard-gated non-empty above
    label: 'PY',
    h3r4Dir: H3R4_DIR,
  })
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
