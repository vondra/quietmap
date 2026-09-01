/**
 * Enrich IQ railways.arrow with Iraqi Republic Railways corridor defaults.
 *
 * No open rail data exists: the Iraqi Republic Railways Company (IRR) publishes
 * no GTFS — and there is barely a service to publish. IRR's ~2,000 km of 1,435 mm
 * standard gauge was wrecked twice over (2003 invasion, then ISIS 2014-17 across
 * the northern lines), and most of the network is dormant. The one corridor with
 * regular partially-restored service is Baghdad ↔ Basra; the rest of the OSM rail
 * geometry carries, at most, the occasional freight movement.
 *
 * This is the layer where the engine default ("80 passenger + 20 freight for ALL
 * main lines") is catastrophically wrong: it would assign 100 phantom trains/day
 * to dead track running through Mosul, Kirkuk and the western desert. We replace
 * that with two evidence-anchored tiers. Counts are trains/day in BOTH directions
 * (engine convention).
 *
 * ## Tiers (trains/day, both directions)
 *
 * | tier                                   | pax | frt | applied to |
 * |----------------------------------------|----:|----:|------------|
 * | Baghdad ↔ Basra (partially restored)   |   3 |   4 | rail_type 0 near the corridor |
 * | rest of the near-dead network          |   1 |   1 | all other rail_type 0 |
 *
 * Iraq has NO functioning metro or tram (a Baghdad metro has been "planned" for
 * decades but never built), so surface tram / light_rail rows (rail_type 1/2) are
 * left at the engine default — there is no urban-rail service to model.
 *
 * NOTE the floor is 1, not 0: `normalize_rail` reads a count of 0 as "use the
 * type/usage default" (mainline → 80 pax + 20 freight), so writing 0 on a dead
 * line would RESURRECT the very phantom trains we're removing. 1 pax + 1 freight
 * is the smallest count the engine takes literally — acoustically ~silent and,
 * unlike 0, it actually replaces the default. Dropping dead lines from 80+20 to
 * 1+1 strips 15-20 dB of phantom rail noise from northern/western Iraq, which
 * dominates the error here. Full derivation in
 * data/enrichment/2026/iq/provenance.md.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-iq.ts
 */

import { resolve } from 'node:path'
import { shouldOverwrite } from './lib/provenance.js'
import { SOURCE_ID_IQ_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { writeRailTrains } from './lib/railways-arrow.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IQ_NATIONAL_RAILWAY


const IQ_HEX_BBOX: [number, number, number, number] = [29.0, 38.7, 37.4, 48.6]

// ── Baghdad ↔ Basra corridor [lon, lat] — matched by min distance to the whole
// line. The restored line runs Baghdad → Diwaniyah → Samawah → Nasiriyah → Basra
// down the Euphrates/Tigris plain. ──
const BAGHDAD_BASRA: [number, number][] = [
  [44.36, 33.31], // Baghdad
  [44.66, 32.60],
  [44.93, 31.99], // Diwaniyah
  [45.30, 31.32], // Samawah
  [46.26, 31.05], // Nasiriyah
  [47.78, 30.51], // Basra
]

const nearCorridor = (lat: number, lon: number) =>
  pointToPolylineDist(lat, lon, BAGHDAD_BASRA) <= 12000

/** Classify one rail row into trains/day. null = leave at engine default.
 *  Counts must be ≥1: normalize_rail treats 0 as "use the mainline default"
 *  (80+20), so 0 would un-silence a dead line — see the module header. */
function classify(lat: number, lon: number, railType: number): { pax: number; frt: number } | null {
  if (railType !== 0) return null               // no functioning tram/metro/narrow-gauge in Iraq
  if (nearCorridor(lat, lon)) return { pax: 3, frt: 4 }
  return { pax: 1, frt: 1 }                      // near-dead remainder of the network (engine floor)
}

async function main() {
  console.log(`=== IQ Railway Enrichment — IRR Baghdad↔Basra corridor + near-dead defaults (${YEAR}) ===\n`)

  const inIQ = makeCountryGate('IQ')
  const hexDirs = iterateCountryHexes(H3R4_DIR, IQ_HEX_BBOX, 'railways.arrow')
  console.log(`  IQ-bbox hexes with railways.arrow: ${hexDirs.length}\n`)

  let totalRows = 0, enriched = 0, hexesUpdated = 0, preserved = 0, skippedService = 0
  const tierCount: Record<string, number> = {}
  let sumPax = 0, sumFrt = 0
  const startTime = Date.now()

  const tierKey = (pax: number, frt: number): string => (pax === 3 && frt === 4 ? 'baghdad-basra' : 'near-dead')

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]
    const r = await writeRailTrains(
      resolve(H3R4_DIR, hex, 'railways.arrow'),
      (row) => {
        if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) { preserved++; return null }
        if (!inBbox(row.midLat, row.midLon, IQ_HEX_BBOX)) return null
        const t = classify(row.midLat, row.midLon, row.railType)
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
      inIQ, // #31.7 central country gate — see writeRailTrains
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
  console.log(`\n  Mean pax/day: ${(sumPax / Math.max(enriched, 1)).toFixed(1)}, mean frt/day: ${(sumFrt / Math.max(enriched, 1)).toFixed(1)}`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
