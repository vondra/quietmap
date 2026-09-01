/**
 * Enrich TZ railways.arrow with SGR / TAZARA / Central-Line corridor-tier defaults.
 *
 * Tanzania Railways Corporation (TRC) and TAZARA publish no GIS/GTFS — not even for
 * the flagship SGR. So, like the road layer, we apply corridor-tier defaults over the
 * OSM rail geometry, gated to Tanzanian soil by the country polygon. Counts are
 * trains/day in BOTH directions (engine convention).
 *
 * Tanzania's rail context:
 *   - **SGR (Standard Gauge Railway)** — the only modern busy line, built parallel to
 *       the old Central-Line corridor:
 *         Phase 1 Dar es Salaam↔Morogoro, 300 km, opened 2021.
 *         Phase 2 Morogoro↔Makutupora/Dodoma, 422 km, opened 2024.
 *         Phases 3-5 (Makutupora↔Tabora↔Isaka↔Mwanza + Kigoma) under construction.
 *   - **TAZARA** — 1,860 km Chinese-built (1970-75), Cape gauge, Dar es Salaam↔Kapiri
 *       Mposhi (Zambia). The Copperbelt freight spine: copper/cobalt from Zambia & DRC
 *       to Dar port. Southern route through the Selous to Mbeya↔Tunduma — freight-heavy,
 *       sparse passenger.
 *   - **Central Line + branches** — colonial metre gauge (TRC). Dar↔Tabora↔Kigoma (Lake
 *       Tanganyika) + Tabora↔Mwanza (Lake Victoria) + the northern Tanga↔Moshi↔Arusha
 *       line. Partially defunct; low intermittent freight.
 *   - **No trams, light rail, or metros**; Dar es Salaam's mass transit is the DART BRT
 *       (buses, not rail).
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                                  | pax | frt | applied to |
 * |---------------------------------------|----:|----:|------------|
 * | SGR (Dar↔Morogoro↔Dodoma, 2021/2024)  |  10 |  20 | SGR corridor + Dar terminus |
 * | TAZARA (Dar↔Tunduma copper corridor)  |   3 |  12 | southern Selous→Mbeya route |
 * | industrial spur                       |   0 |   4 | usage=industrial (Dar port shunting) |
 * | Central Line + other metre gauge      |   2 |   6 | everything else (Tabora/Mwanza/Tanga) |
 *
 * The SGR and the old Central Line run PARALLEL (often <5 km apart) along Dar↔Dodoma —
 * geography can't separate them, so that corridor is scored as the busy SGR (it has
 * absorbed the passenger traffic the Central Line lost). SGR is checked before TAZARA
 * so the shared Dar terminus is labelled SGR, not low-passenger copper freight.
 * Counts are coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, the right
 * granularity given zero published per-line data. Full derivation in
 * data/enrichment/2026/tz/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-tz.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_TZ_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_TZ_NATIONAL_RAILWAY


const TZ_HEX_BBOX: [number, number, number, number] = [-11.8, 29.3, -0.9, 40.5]

// ── SGR corridor [lon, lat] — Dar es Salaam → Morogoro → Dodoma → Makutupora ──
// Phase 1 (2021) Dar↔Morogoro + Phase 2 (2024) Morogoro↔Makutupora/Dodoma. Built
// alongside the old Central Line; this polyline also covers that parallel metre-gauge
// track (it has lost its passengers to the SGR, so the busy tier is the honest one).
const SGR_DAR_DODOMA: [number, number][] = [
  [39.28, -6.82],  // Dar es Salaam (Pugu)
  [38.95, -6.85],
  [38.65, -6.75],  // Ruvu
  [37.661, -6.821], // Morogoro
  [37.00, -6.83],  // Kilosa
  [36.30, -6.40],
  [35.742, -6.173], // Dodoma
  [35.78, -5.97],  // Makutupora (Phase 2 terminus)
]

// ── TAZARA corridor [lon, lat] — Dar es Salaam → Selous → Mbeya → Tunduma ──
// Cape-gauge copper/cobalt freight spine to Zambia (Kapiri Mposhi). Diverges from the
// SGR south of Dar through the Selous, rejoining the TANZAM road axis near Makambako.
const TAZARA_CORRIDOR: [number, number][] = [
  [39.28, -6.82],  // Dar es Salaam (Kurasini)
  [38.30, -7.30],
  [37.60, -7.75],  // Selous / Kisaki
  [36.68, -8.13],  // Ifakara
  [35.83, -8.86],  // Mlimba
  [34.83, -8.85],  // Makambako
  [33.46, -8.91],  // Mbeya
  [32.77, -9.30],  // Tunduma (Zambia border)
]

const near = (lat: number, lon: number, line: [number, number][], m = 12000) =>
  pointToPolylineDist(lat, lon, line) <= m

/** Classify one rail row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType !== 0) return null              // TZ has no tram / light_rail / metro
  if (usage === 2) return { pax: 0, frt: 4 }   // industrial spur (Dar port shunting etc.)

  // SGR first so the Dar terminus (shared with TAZARA + Central Line) is scored busy.
  if (near(lat, lon, SGR_DAR_DODOMA)) return { pax: 10, frt: 20 }
  if (near(lat, lon, TAZARA_CORRIDOR)) return { pax: 3, frt: 12 }

  // Everything else rail_type 0 in TZ is old metre gauge — Central Line (Tabora /
  // Kigoma / Mwanza) and the northern Tanga↔Moshi↔Arusha line. Low intermittent use.
  return { pax: 2, frt: 6 }
}

async function main() {
  console.log(`=== TZ Railway Enrichment — SGR/TAZARA/Central-Line corridor-tier defaults (${YEAR}) ===\n`)

  const inTZ = makeCountryGate('TZ')
  const hexDirs = iterateCountryHexes(H3R4_DIR, TZ_HEX_BBOX, 'railways.arrow')
  console.log(`  TZ-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 10 ? 'sgr' : pax === 3 ? 'tazara' : pax === 0 ? 'industrial' : 'metre-gauge'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, TZ_HEX_BBOX)) return null
        const t = classify(row.midLat, row.midLon, row.railType, row.usage)
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
      inTZ, // #31.7 central country gate — see writeRailTrains
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
