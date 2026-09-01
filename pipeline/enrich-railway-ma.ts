/**
 * Enrich MA railways.arrow with ONCF/Al Boraq/OCP/tramway corridor-tier defaults.
 *
 * No open rail geometry or GTFS exists: ONCF (oncf.ma — DNS dead), the OCP
 * phosphate railway (private, never released), and the Casa/Rabat tram operators
 * (RATP Dev) publish corporate HTML only — not even Al Boraq, Africa's only HSR,
 * is in any open feed. So — like the road layer — we apply tier defaults over the
 * OSM rail geometry, gated to Moroccan soil (incl. the Western Sahara provinces,
 * via MA ∪ EH, matching enrich-industrial-ma.ts). Counts are trains/day in BOTH
 * directions (engine convention).
 *
 * Morocco's rail context: ONCF runs ~2,295 km conventional (Casablanca the hub),
 * plus Al Boraq — Africa's first and only HSR (Tangier↔Kenitra at 320 km/h, 2018).
 * The acoustically dominant line is the OCP phosphate railway (Khouribga ↔ Jorf
 * Lasfar / Youssoufia ↔ Safi — the world's largest mineral railway for a single
 * commodity, ~40 Mtpa): a freight-only corridor whose block-braked ore trains
 * outweigh every passenger service in Lden terms. Urban transit: Casa Tramway
 * (2 lines, 2012) + Rabat-Salé Tramway (2 lines, 2011) — no metro.
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                                    | pax | frt | applied to |
 * |-----------------------------------------|----:|----:|------------|
 * | Casa Tramway                            | 300 |   0 | tram near Casablanca |
 * | Rabat-Salé Tramway                      | 250 |   0 | tram near Rabat |
 * | other tram / light_rail                 | 200 |   0 | tram elsewhere (none today) |
 * | OCP phosphate corridor                  |   0 |  60 | Khouribga↔Jorf, Benguerir↔Youssoufia↔Safi |
 * | Eastern mainline (Casa↔Fez↔Oujda)       |  40 |  15 | Kenitra→Fez→Oujda spine |
 * | Atlantic mainline (Tangier↔Casa)        |  40 |   8 | Tangier→Kenitra→Casa (incl. Al Boraq, see below) |
 * | Casablanca↔Marrakech                    |  30 |  10 | Casa→Settat→Benguerir→Marrakech |
 * | secondary main (off-corridor usage=main)|   8 |   6 | Casa↔El Jadida/Safi, Nador, Oujda↔Bouarfa |
 * | industrial spur                         |   0 |   8 | usage=industrial (port/plant) |
 * | branch                                  |   4 |   3 | usage=branch |
 *
 * Al Boraq HSR (60 pax, 0 frt per the doc) runs on a dedicated LGV ~10-30 km
 * from the conventional coastal line — too close to separate by proximity, and
 * RailRow carries no high-speed flag. It is therefore folded into the Atlantic
 * corridor (40 pax / 8 frt). The acoustic cost is small: the HSR's real signature
 * is its 320 km/h speed (30·log10 v dominates rail emission), which the engine
 * reads from the OSM `maxspeed` tag, not from this script. See provenance.md.
 *
 * Counts are coarse (±30-40%) but freight/day moves Lden in ~1-2 dB steps, so this
 * is the right granularity given zero published per-line data. Full derivation in
 * data/enrichment/2026/ma/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-ma.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_MA_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeOwnershipGate,  } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_MA_NATIONAL_RAILWAY


const MA_HEX_BBOX: [number, number, number, number] = [20.7, -17.3, 36.1, -0.9]

// ── ONCF / OCP corridor polylines [lon, lat] — matched by min distance to the line ──
// Atlantic mainline: Tangier ↔ Asilah ↔ Kenitra ↔ Rabat ↔ Casablanca (the busy
// coastal trunk; Al Boraq HSR is folded in — see header).
const ATLANTIC_MAIN: [number, number][] = [
  [-5.80, 35.77], [-6.04, 35.47], [-6.15, 35.18], [-6.58, 34.26],
  [-6.82, 34.02], [-7.38, 33.69], [-7.59, 33.57],
]
// Eastern mainline: Kenitra ↔ Sidi Kacem ↔ Meknes ↔ Fez ↔ Taza ↔ Oujda (ONCF's
// highest conventional-freight spine — Oujda industry + Algeria-border traffic).
const EASTERN_MAIN: [number, number][] = [
  [-6.58, 34.26], [-5.71, 34.22], [-5.55, 33.90], [-5.00, 34.04],
  [-4.01, 34.21], [-2.89, 34.41], [-1.91, 34.68],
]
// Casablanca ↔ Marrakech: Casa ↔ Berrechid ↔ Settat ↔ Benguerir ↔ Marrakech.
const CASA_MARRAKECH: [number, number][] = [
  [-7.59, 33.57], [-7.59, 33.27], [-7.62, 33.00], [-7.95, 32.24], [-8.01, 31.63],
]
// OCP phosphate freight — two distinct alignments, kept off the Casa↔Marrakech
// trunk except at the Benguerir/Settat junction nodes (documented overlap).
//   north: Khouribga (world's largest phosphate mine) → Jorf Lasfar export port
const PHOSPHATE_JORF: [number, number][] = [
  [-6.91, 32.88], [-7.50, 32.92], [-8.10, 33.00], [-8.63, 33.13],
]
//   south: Benguerir → Youssoufia → Safi phosphate port
const PHOSPHATE_SAFI: [number, number][] = [
  [-7.95, 32.24], [-8.53, 32.25], [-9.24, 32.30],
]

const near = (lat: number, lon: number, line: [number, number][], m = 12000) =>
  pointToPolylineDist(lat, lon, line) <= m
const nearPhosphate = (lat: number, lon: number) =>
  near(lat, lon, PHOSPHATE_JORF, 10000) || near(lat, lon, PHOSPHATE_SAFI, 10000)

// Tram-city boxes (centre lat/lon, half-width deg).
const nearBox = (lat: number, lon: number, clat: number, clon: number, half: number) =>
  lat >= clat - half && lat <= clat + half && lon >= clon - half && lon <= clon + half

/** Classify one rail/tram row into trains/day. null = leave at engine default. */
function classify(lat: number, lon: number, railType: number, usage: number): { pax: number; frt: number } | null {
  if (railType === 1 || railType === 2) {           // tram / light_rail
    if (nearBox(lat, lon, 33.57, -7.59, 0.25)) return { pax: 300, frt: 0 } // Casablanca
    if (nearBox(lat, lon, 34.02, -6.82, 0.20)) return { pax: 250, frt: 0 } // Rabat-Salé
    return { pax: 200, frt: 0 }                       // no other MA trams today
  }
  if (railType !== 0) return null                     // narrow_gauge / funicular → default
  if (usage === 2) return { pax: 0, frt: 8 }          // industrial spur (port/plant)

  // Freight-dominated phosphate corridor first (block-braked ore — loudest rail
  // source in Morocco), then the passenger trunks. pax floored at 1, not 0:
  // normalize_rail reads pax=0 as "use def_pax" (80/day on a mainline), which
  // would un-silence this freight-only line.
  if (nearPhosphate(lat, lon)) return { pax: 1, frt: 60 }
  if (near(lat, lon, EASTERN_MAIN)) return { pax: 40, frt: 15 }
  if (near(lat, lon, ATLANTIC_MAIN)) return { pax: 40, frt: 8 }
  if (near(lat, lon, CASA_MARRAKECH)) return { pax: 30, frt: 10 }

  if (usage === 1) return { pax: 4, frt: 3 }          // branch
  return { pax: 8, frt: 6 }                            // secondary main off-corridor
}

async function main() {
  console.log(`=== MA Railway Enrichment — ONCF/Al Boraq/OCP/tramway corridor-tier defaults (${YEAR}) ===\n`)

  // MA∪EH via the SSOT union (COUNTRY_TERRITORY_EXTENSIONS) — the auditor and
  // heal-rail-country-bleed derive the SAME gate for source key `ma-…`, so the
  // three sides can never disagree about Western Sahara rows (#32 round-3).
  const inCountry = makeOwnershipGate('MA')

  const hexDirs = iterateCountryHexes(H3R4_DIR, MA_HEX_BBOX, 'railways.arrow')
  console.log(`  MA-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string =>
    pax === 300 ? 'casa-tram' : pax === 250 ? 'rabat-tram' : pax === 200 ? 'other-tram'
    : pax === 0 && frt === 60 ? 'phosphate' : pax === 40 && frt === 15 ? 'eastern-main'
    : pax === 40 && frt === 8 ? 'atlantic-main' : pax === 30 ? 'casa-marrakech'
    : pax === 0 && frt === 8 ? 'industrial' : pax === 4 ? 'branch' : 'secondary-main'

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, MA_HEX_BBOX)) return null
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
      inCountry, // #31.7 central country gate — see writeRailTrains
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
