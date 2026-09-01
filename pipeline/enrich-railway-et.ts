/**
 * Enrich ET railways.arrow with EDR / AKR / Addis-LRT corridor-tier defaults.
 *
 * Ethiopian Railways Corporation, the binational EDR operator, and ERC publish no
 * GIS/GTFS — not even for the flagship electrified SGR (confirmed June 2026 against
 * ERC, EDR, mobilitydatabase.org, transit.land). So, like the road layer, we apply
 * corridor-tier defaults over the OSM rail geometry, gated to Ethiopian soil by the
 * country polygon. Counts are trains/day in BOTH directions (engine convention) and
 * are CALIBRATED to published secondary traffic statistics (see tier table).
 *
 * Ethiopia's rail context (confirmed against the OSM geometry in our hexes):
 *   - **Addis–Djibouti Railway (EDR)** — 752 km *electrified* standard gauge
 *       (25 kV AC), opened Jan 2018. Africa's first electrified cross-border line.
 *       Addis(Sebeta) ↔ Adama ↔ Awash ↔ Dire Dawa ↔ Dewele ↔ Djibouti port.
 *       Built primarily for FREIGHT — landlocked Ethiopia runs ~95% of its trade
 *       through Djibouti — with a lighter passenger service. Freight-dominant.
 *   - **Awash–Weldiya / Hara Gebeya Railway (AKR)** — ~390 km standard gauge,
 *       opened 2022 but NOT in regular public operation (ERC). Branches NORTH off the
 *       EDR at Awash to Kombolcha / Dessie / Weldiya. Minimal intermittent traffic.
 *   - **Addis Ababa Light Rail (AALRT)** — Africa's first light-rail metro, 2015.
 *       Two lines (EW "Green" + NS "Blue"), ~32 km, in central Addis. Disc-braked
 *       electric multiple units, NO freight, ~35 km/h.
 *   - **No operational metre gauge** — the colonial Ethio-Djibouti (CDE) line is
 *       dismantled. OSM's `narrow_gauge` inside this bbox is all *cross-border
 *       Sudan Railways*, which the ET country gate excludes.
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                                  | pax | frt | basis |
 * |---------------------------------------|----:|----:|-------|
 * | EDR (Addis↔Dire Dawa↔Djibouti, 2018)  |   4 |  12 | 84k pax 2019 → ~daily each way; 3.2 Mt freight 2025 ÷ ~1.7 kt/train ≈ ~10-12/day |
 * | AKR (Awash↔Weldiya, 2022)             |   1 |   4 | not in regular operation; minimal intermittent freight |
 * | Addis Ababa LRT (rail_type tram/LRT)  | 150 |   0 | ~15-min headway × ~17 h ≈ ~136/day (2025); ~187 rotations/day design (2016) |
 * | funicular / incline                   |   2 |   0 | the 1 stray rail_type=4 segment |
 * | defunct metre gauge (guard only)      |   0 |   1 | rail_type=3 (none survives inside ET) |
 *
 * EDR and AKR run as named OSM lines ("Addis Ababa – Djibouti Railway" vs
 * "Awash–Weldiya Railway"), so each standard-gauge segment is split by its
 * authoritative line NAME (91% of rows are named); the unnamed minority falls back to
 * the nearer of two coarse corridor polylines. Either way no segment is left at the
 * catastrophic engine default (80 pax + 20 frt). Removing that default's phantom
 * block-braked freight from the Addis tram lines is the single biggest ET rail
 * correction (+10 dB error on every LRT segment). Counts are coarse (±30-40%) but the
 * freight/passenger split and the freight-vs-LRT distinction are what move Lden, and
 * those are right. Full derivation in data/enrichment/2026/et/provenance.md.
 *
 * No external download — pure corridor-tier defaults — so the script is inherently
 * --enrich-only; re-run it verbatim after any OSM re-extract.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-et.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_ET_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_ET_NATIONAL_RAILWAY


// Encloses Ethiopia; the country polygon refines (excludes cross-border Sudan rail).
const ET_HEX_BBOX: [number, number, number, number] = [3.3, 32.9, 15.0, 48.1]

// ── EDR corridor [lon, lat] — Addis(Sebeta) → Adama → Awash → Dire Dawa → Dewele ──
// Electrified SGR to Djibouti port. Waypoints traced to hug the OSM alignment (verified
// against the per-0.5°-cell density of rail_type=0 rows inside the ET polygon).
const EDR_CORRIDOR: [number, number][] = [
  [38.60, 8.92],  // Addis Ababa / Sebeta (Lebu terminus)
  [38.80, 8.86],  // Akaki / Indode freight yard
  [38.99, 8.74],  // Bishoftu (Debre Zeit)
  [39.11, 8.59],  // Mojo
  [39.27, 8.54],  // Adama (Nazret)
  [39.55, 8.62],
  [39.92, 8.90],  // Metehara
  [40.17, 8.99],  // Awash (junction with AKR)
  [40.45, 9.10],
  [40.75, 9.24],  // Mieso
  [41.20, 9.45],
  [41.70, 9.55],
  [41.86, 9.59],  // Dire Dawa
  [42.00, 9.95],
  [42.20, 10.45],
  [42.50, 11.00],
  [42.65, 11.15], // Dewele (Djibouti border)
]

// ── AKR corridor [lon, lat] — Awash → Kombolcha → Dessie → Weldiya/Hara Gebeya ──
// Branches north off the EDR at Awash through the Afar lowlands.
const AKR_CORRIDOR: [number, number][] = [
  [40.17, 8.99],  // Awash (junction)
  [40.05, 9.55],
  [40.00, 10.05],
  [39.95, 10.55],
  [39.85, 10.90],
  [39.74, 11.08], // Kombolcha
  [39.63, 11.13], // Dessie
  [39.62, 11.45],
  [39.60, 11.72], // Weldiya / Hara Gebeya
]

const EDR_TIER = { pax: 4, frt: 12 }
const AKR_TIER = { pax: 1, frt: 4 }

/** Classify one rail row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, name: string): { pax: number; frt: number } | null {
  // Urban light rail / tram = Addis Ababa LRT (the only one in ET). Disc-braked EMUs,
  // no freight — strip the engine default's phantom 20 block-braked freight trains.
  if (railType === 1 || railType === 2) return { pax: 150, frt: 0 }

  // Funicular / incline — the single stray rail_type=4 segment; near-silent, no freight.
  if (railType === 4) return { pax: 2, frt: 0 }

  // Defunct colonial metre gauge (Ethio-Djibouti CDE). None survives inside the ET
  // polygon today (OSM's narrow_gauge here is all cross-border Sudan Railways, gated
  // out), but guard the default in case a re-extract re-adds a stub. pax floored at
  // 1, not 0: normalize_rail reads pax=0 as "use def_pax" (10/day for narrow gauge).
  if (railType === 3) return { pax: 1, frt: 1 }

  // Anything else (monorail / future unmapped type) is NOT Ethiopian standard gauge —
  // leave it at the engine default rather than mislabelling it EDR/AKR below.
  if (railType !== 0) return null

  // Standard gauge (railType 0) — only EDR + AKR operate. Split by the authoritative
  // OSM line name (91% of rows carry it); checking AKR first since "Awash–Weldiya"
  // never contains a Djibouti token, and EDR names never contain "weldiya".
  const lc = name.toLowerCase()
  if (lc.includes('weldiya') || lc.includes('woldia')) return AKR_TIER
  if (lc.includes('djibouti') || name.includes('ጅቡቲ') || name.includes('جيبوتي')) return EDR_TIER

  // Unnamed/other standard gauge → nearer of the two coarse corridor polylines. EDR
  // and AKR diverge cleanly at Awash (EDR east, AKR north), so nearest-arm is robust
  // for the ~9% unnamed; the tie at Awash goes to the busier EDR (<=).
  const dEDR = pointToPolylineDist(lat, lon, EDR_CORRIDOR)
  const dAKR = pointToPolylineDist(lat, lon, AKR_CORRIDOR)
  return dEDR <= dAKR ? EDR_TIER : AKR_TIER
}

const tierKey = (pax: number, frt: number): string =>
  pax === 150 ? 'addis-lrt'
  : frt === 12 ? 'edr-addis-djibouti'
  : frt === 4 ? 'akr-awash-weldiya'
  : pax === 2 ? 'funicular'
  : 'metre-gauge-defunct'

async function main() {
  console.log(`=== ET Railway Enrichment — EDR/AKR/Addis-LRT corridor-tier defaults (${YEAR}) ===\n`)

  const inET = makeCountryGate('ET')
  const hexDirs = iterateCountryHexes(H3R4_DIR, ET_HEX_BBOX, 'railways.arrow')
  console.log(`  ET-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, ET_HEX_BBOX)) return null
        const t = classify(row.midLat, row.midLon, row.railType, row.name)
        if (!t) return null
        return { pax: t.pax, frt: t.frt, sourceId: MY_SOURCE_ID }
      },
      (_row, _i, applied) => {
        enriched++
        const k = tierKey(applied.pax, applied.frt)
        tierCount[k] = (tierCount[k] || 0) + 1
        sumPax += applied.pax
        sumFrt += applied.frt
      },
      undefined,
      inET, // #31.7 central country gate — see writeRailTrains
    )
    totalRows += r.rows
    skippedService += r.skippedService
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 20 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length} hexes, ${enriched.toLocaleString()} enriched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total rail rows scanned: ${totalRows.toLocaleString()}`)
  console.log(`  Skipped service tracks:  ${skippedService.toLocaleString()}`)
  console.log(`  Preserved (higher prio): ${preserved.toLocaleString()}`)
  console.log(`  Enriched:                ${enriched.toLocaleString()}`)
  console.log(`  Hexes updated:           ${hexesUpdated}/${hexDirs.length}`)
  console.log(`\n  By tier:`)
  for (const [k, v] of Object.entries(tierCount).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${k}: ${v.toLocaleString()}`)
  }
  console.log(`\n  Mean pax/day: ${(sumPax / Math.max(enriched, 1)).toFixed(0)}, mean frt/day: ${(sumFrt / Math.max(enriched, 1)).toFixed(0)}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
