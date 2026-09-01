/**
 * Enrich UZ roads.arrow with Uzbekistan-tuned CNOSSOS class defaults.
 *
 * No open per-segment AADT exists for Uzbekistan. Uzavtoyul (state road agency)
 * and the Ministry of Transport publish no traffic-count datasets; the national
 * open-data portal (data.gov.uz) carries registries, not counts. So — like the
 * neighbours RU/KZ and like CN/IN/IR — we apply country-tuned class defaults over
 * OSM geometry rather than a per-segment join, gated by makeCoastalCountryGate('UZ').
 *
 * ## AADT by OSM class (two-way veh/day), rural baseline
 *
 * | class            | rural  | Tashkent ×2.0 | tier-2 ×1.4 | anchor                              |
 * |------------------|-------:|--------------:|------------:|-------------------------------------|
 * | 0 motorway       | 25,000 |        50,000 |      35,000 | M-39 Tashkent↔Samarkand upgrade     |
 * | 1 trunk          | 12,000 |        24,000 |      16,800 | A-class intercity (M-37/M-34)       |
 * | 2 primary        |  6,000 |        12,000 |       8,400 | regional arterials                  |
 * | 3 secondary      |  3,000 |         6,000 |       4,200 | step-down                           |
 * | 4 tertiary       |  1,400 |         2,800 |       1,960 | step-down                           |
 * | 5 residential    |    600 |         1,200 |         840 | step-down                           |
 *
 * Links (10/11/12) get ~half the parent-class flow (ramp traffic).
 *
 * ## City tiers (AADT multiplier vs rural baseline)
 *   Tashkent ×2.0 — Central Asia's largest city (~2.9 M, wide Soviet boulevards).
 *   10 tier-2 regional capitals ×1.4 (Samarkand, Namangan, Andijan, Bukhara,
 *   Fergana, Nukus, Qarshi, Jizzakh, Navoiy, Urgench). No published per-city
 *   scalar exists; calibrated by eye to the population/role gradient.
 *
 * ## Vehicle split — heavy share is the single most acoustically important input
 *   We use a CLASS-graded heavy share (trucks concentrate on the high-order
 *   network), NOT the per-tier blanket used by some older country notes — a
 *   blanket "35% HGV everywhere" would wrongly stamp a third of a village street
 *   as trucks. Uzbekistan is freight-reliant and a low-passenger-car economy, so
 *   the high-order share is set above RU's 27%: the M-39 Silk Road / China-bound
 *   transit corridors run very HGV-heavy. The M-39's documented ~38% is folded
 *   into the motorway/trunk default rather than built as a separate corridor
 *   geometry (Occam — an 8 pp bump on one corridor isn't worth the indirection).
 *     motorway/trunk (+links): heavy 0.30
 *     primary/secondary (+link): heavy 0.16
 *     tertiary                 : heavy 0.08
 *     residential              : heavy 0.05
 *   Medium (LCV) 0.06, moto 0.04 across all classes; light = remainder. The
 *   passenger fleet is a near-total **Chevrolet/Daewoo monoculture** (UzAuto Asaka,
 *   ~300k cars/yr; extreme import tariffs) — a homogeneous light-vehicle category,
 *   so no special light-vehicle correction is warranted. Moto share is modest
 *   (Damas-minivan culture, not a SE-Asia two-wheeler economy) — highMoto unset.
 *
 * Sources: Uzbekistan road network role (M-39/M-37/M-34 A-roads), UzAuto fleet
 * composition, regional-capital populations. Full list + URLs in
 * data/enrichment/2026/uz/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-uz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_UZ_NATIONAL_ROADS } from './lib/source-ids.generated.js'
import { inBbox } from './lib/spatial.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCoastalCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_UZ_NATIONAL_ROADS


// UZ coarse bbox [minLat,minLon,maxLat,maxLon] — cheap pre-scan fed to
// iterateCountryHexes and re-checked per midpoint. The makeCoastalCountryGate('UZ')
// polygon is the real filter; this box only skips the rest of the planet. UZ has
// 5 land neighbours (KZ/TM/TJ/KG/AF), so the polygon gate prevents stamping Uzbek
// AADT onto their roads (the doubly-landlocked interior is well inside the box).
const UZ_HEX_BBOX: [number, number, number, number] = [37.2, 55.9, 45.6, 73.2]

// ── City tiers (AADT multiplier vs rural baseline) ──
// half = bbox half-extent in degrees around the centroid (~city + ring extent).
interface City { name: string; lat: number; lon: number; mult: number; half: number }

const CITIES: City[] = [
  // Tier 1 — Tashkent (~2.9 M)
  { name: 'Tashkent',  lat: 41.31, lon: 69.24, mult: 2.0, half: 0.30 },
  // Tier 2 — regional capitals (×1.4)
  { name: 'Samarkand', lat: 39.65, lon: 66.96, mult: 1.4, half: 0.16 },
  { name: 'Namangan',  lat: 40.99, lon: 71.67, mult: 1.4, half: 0.16 },
  { name: 'Andijan',   lat: 40.78, lon: 72.34, mult: 1.4, half: 0.16 },
  { name: 'Bukhara',   lat: 39.77, lon: 64.42, mult: 1.4, half: 0.16 },
  { name: 'Fergana',   lat: 40.39, lon: 71.78, mult: 1.4, half: 0.16 },
  { name: 'Nukus',     lat: 42.46, lon: 59.61, mult: 1.4, half: 0.16 },
  { name: 'Qarshi',    lat: 38.86, lon: 65.79, mult: 1.4, half: 0.16 },
  { name: 'Jizzakh',   lat: 40.12, lon: 67.84, mult: 1.4, half: 0.16 },
  { name: 'Navoiy',    lat: 40.10, lon: 65.38, mult: 1.4, half: 0.16 },
  { name: 'Urgench',   lat: 41.55, lon: 60.63, mult: 1.4, half: 0.16 },
]

function cityMultiplier(lat: number, lon: number): number {
  for (const c of CITIES) {
    if (lat >= c.lat - c.half && lat <= c.lat + c.half &&
        lon >= c.lon - c.half && lon <= c.lon + c.half) return c.mult
  }
  return 1.0
}

// ── AADT defaults by OSM road_class (engine inputs.rs codes) ──
const BASE_AADT: Record<number, number> = {
  0: 25000, 10: 12500, // motorway + motorway_link
  1: 12000, 11: 6000,  // trunk + trunk_link
  2: 6000,  12: 3000,  // primary + primary_link
  3: 3000,             // secondary
  4: 1400,             // tertiary
  5: 600,              // residential
}
const UZ_COVERAGE: ReadonlySet<number> = new Set(Object.keys(BASE_AADT).map(Number))

/** Heavy-vehicle share by road class — trucks concentrate on the high-order
 *  network; UZ's freight reliance + M-39 transit lift the top tier above RU's. */
function heavyShare(cls: number): number {
  if (cls === 0 || cls === 1 || cls === 10 || cls === 11) return 0.30 // motorway/trunk + links
  if (cls === 2 || cls === 3 || cls === 12) return 0.16               // primary/secondary + primary_link
  if (cls === 4) return 0.08                                          // tertiary
  return 0.05                                                         // residential
}

function splitVehicles(aadt: number, cls: number) {
  const heavy = heavyShare(cls)
  const medium = 0.06 // LCV
  const moto = 0.04
  const light = 1 - heavy - medium - moto
  return {
    light: Math.round(aadt * light),
    medium: Math.round(aadt * medium),
    heavy: Math.round(aadt * heavy),
    moto: Math.round(aadt * moto),
  }
}

async function main() {
  console.log(`=== UZ Roads Enrichment — Uzbekistan-tuned CNOSSOS class defaults (${YEAR}) ===\n`)

  // Created here (not module scope): makeCoastalCountryGate may download+convert the CGAZ
  // boundary file on a fresh host. UZ borders 5 countries; a rectangular bbox alone
  // would stamp Uzbek AADT onto Kazakh/Turkmen/Tajik/Kyrgyz/Afghan roads that lack
  // their own national enrichment — the polygon gate prevents it.
  const inUZ = makeCoastalCountryGate('UZ')

  const hexDirs = iterateCountryHexes(H3R4_DIR, UZ_HEX_BBOX)
  console.log(`  UZ-bbox hexes with roads.arrow: ${hexDirs.length}\n`)

  let totalRoads = 0, enriched = 0, hexesUpdated = 0, preserved = 0, outsideUZ = 0
  const byClass: Record<number, number> = {}
  const byMult: Record<string, number> = {}
  let sumHeavy = 0, sumAll = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRoadAadt(
      resolve(H3R4_DIR, hex, 'roads.arrow'),
      (row) => {
        // Cheap gates first; the polygon gate (inUZ) is the expensive call — last.
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        const base = BASE_AADT[row.roadClass]
        if (base === undefined) return null
        if (!inBbox(row.midLat, row.midLon, UZ_HEX_BBOX)) return null
        if (!inUZ(row.midLat, row.midLon)) { outsideUZ++; return null }

        const mult = cityMultiplier(row.midLat, row.midLon)
        const aadt = base * mult
        const s = splitVehicles(aadt, row.roadClass)
        return { ...s, sourceId: MY_SOURCE_ID }
      },
      (row, _i, applied) => {
        enriched++
        byClass[row.roadClass] = (byClass[row.roadClass] || 0) + 1
        const m = cityMultiplier(row.midLat, row.midLon)
        const key = m === 2.0 ? 'Tashkent ×2.0' : m === 1.4 ? 'tier-2 ×1.4' : 'rural ×1.0'
        byMult[key] = (byMult[key] || 0) + 1
        sumHeavy += applied.heavy
        sumAll += applied.light + applied.medium + applied.heavy
      },
      UZ_COVERAGE,
    )
    totalRoads += r.rows
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 100 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${enriched.toLocaleString()} enriched`)
    }
  }

  const CLASS_NAME: Record<number, string> = {
    0: 'motorway', 1: 'trunk', 2: 'primary', 3: 'secondary', 4: 'tertiary',
    5: 'residential', 10: 'motorway_link', 11: 'trunk_link', 12: 'primary_link',
  }
  console.log(`\n=== Results ===`)
  console.log(`  Total roads scanned:     ${totalRoads.toLocaleString()}`)
  console.log(`  Preserved (higher prio): ${preserved.toLocaleString()}`)
  console.log(`  Outside UZ polygon:      ${outsideUZ.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()} (${(100 * enriched / Math.max(totalRoads, 1)).toFixed(1)}%)`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By road class:`)
  for (const cls of Object.keys(byClass).map(Number).sort((a, b) => a - b)) {
    console.log(`    ${CLASS_NAME[cls] ?? cls}: ${byClass[cls].toLocaleString()}`)
  }
  console.log(`\n  By city tier:`)
  for (const [k, v] of Object.entries(byMult).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${v.toLocaleString()}`)
  }
  console.log(`\n  Mean heavy share (light+med+heavy): ${(100 * sumHeavy / Math.max(sumAll, 1)).toFixed(1)}%  (expect ~5-30% by class mix)`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
