/**
 * Enrich IN railways.arrow with Living Atlas India railway network + metro.
 *
 * Data sources:
 *
 * 1. **IN Railway Network** — 119,446 per-segment polylines from Indian Railways.
 *    URL: `livingatlas.esri.in/server1/rest/services/Railway/IN_Railway_Line/MapServer/1`
 *    Fields: fromjunction, tojunction, `speed` (km/h), `type` (Broad/Metre/Narrow
 *    Gauge), `railwayzone`, `nooflanes`, `bridge_yn`, `tunnel_yn`
 *    - 114,968 Broad Gauge (1676mm — the Indian standard)
 *    - 2,123 Metre Gauge + 1,864 Narrow Gauge (heritage, tourist, limited)
 *    - Speed buckets: 100+ km/h (53,123 segments) 60-99 (7,361) 0-59 (58,579)
 *    - 18 IR zones (Central, Western, Northern, Southern, etc.)
 *
 * 2. **IN Metro Network** — 83 metro lines + 1,401 stations covering Delhi, Mumbai,
 *    Bangalore, Ahmedabad, Patna, Chennai, Kolkata, Hyderabad, Kochi, Nagpur,
 *    Lucknow, Jaipur, Pune, Bhopal.
 *    URL: `livingatlas.esri.in/server1/rest/services/MetroNetwork/India_Metro_Network/MapServer`
 *
 * Strategy: per-family spatial match of OSM railway segments to the nearest Living
 * Atlas line within 500m. The feed is split into two family grids so a tram/
 * light_rail row can NEVER inherit a heavy-rail count (the Mumbai ~1,300/day bug):
 *   - rail_type 0 (heavy rail) → matches only IR network features (`isMetro:false`)
 *   - rail_type 1/2 (tram / light_rail) → matches only metro features (`isMetro:true`)
 * Apply trains/day default based on the matched feature:
 *   - Metro match: 400 trains/day (typical UTO)
 *   - Mumbai suburban (Central/Western Railway within Mumbai bbox): 1,300/day
 *     (world's busiest suburban system: 2,342 daily services across 3 lines)
 *   - Delhi suburban (Northern Railway within Delhi NCR bbox): 350/day
 *   - Chennai/Kolkata/Bangalore/Hyderabad suburban: 250/day
 *   - Speed >= 100 km/h broad gauge: 30 pax + 15 freight (IR mainline)
 *   - Speed 60-99 km/h broad gauge: 20 pax + 10 freight
 *   - Speed < 60 km/h broad gauge: 10 pax + 5 freight
 *   - Metre/Narrow gauge: 5 pax (heritage)
 *
 *   Scope: the exact CGAZ IN polygon via the central writeRailTrains countryGate (#31.7).
 * China.
 *
 * Note: Delhi Metro and most other Indian metros are tagged `railway=subway`
 * in OSM — pipeline extractor bug (same as Bangkok MRT, Dubai Metro, Taipei,
 * Singapore, Seoul, Tokyo, HK, Mexico City). The Living Atlas Metro Network
 * geometry is cached but cannot be matched to OSM subway segments because
 * they're not in railways.arrow. Some elevated sections tagged `light_rail`
 * WILL match (the tram/light-rail family grid).
 *
 * writeRailTrains owns the read + seed + SERVICE-SKIP + the priority gate
 * (shouldOverwrite) + fail-loud validation + byte-identical write; only the
 * per-row family routing lives in the match closure; no-match rows stay
 * source_id=0 (engine default_traffic owns unknowns — the class-default
 * fallback was purged 2026-07-10, see OLD_FALLBACK retract).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-in.ts
 */

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { shouldOverwrite } from './lib/provenance.js'
import { writeRailTrains, type RailRow } from './lib/railways-arrow.js'
import { makeCountryGate, segmentWhollyOutside } from './lib/country-polygon.js'
import { cellToLatLng } from 'h3-js'
import { SOURCE_ID_IN_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToSegmentDist } from './lib/spatial.js'
import { logRetractSkippedIncompleteInputs } from './lib/gtfs-enrich-core.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_IN_NATIONAL_RAILWAY

const H3R4_DIR = resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/in`)
// Hoisted so main() can verify input-file presence for the retractSafe gate.
const RAILWAY_NETWORK_GEOJSON = resolve(CACHE_DIR, 'railway-network.geojson')
const METRO_LINES_GEOJSON = resolve(CACHE_DIR, 'metro-lines.geojson')

const IN_BBOX: [number, number, number, number] = [6.5, 68.0, 37.0, 98.0]

// Neighbour exclusion boxes DELETED (#32, /gg #31 round-2 Codex): the central
// writeRailTrains countryGate (exact CGAZ polygon) owns national scope now,
// and the hand rectangles provably clipped DOMESTIC territory (the 'Pakistan'
// box held Ahmedabad's west flank).

// Indian metro cities (broader bbox for suburban rail)
const MUMBAI_BBOX: [number, number, number, number] = [18.9, 72.7, 19.4, 73.3]
const DELHI_BBOX: [number, number, number, number] = [28.3, 76.8, 29.2, 77.6]
const CHENNAI_BBOX: [number, number, number, number] = [12.8, 80.1, 13.3, 80.4]
const KOLKATA_BBOX: [number, number, number, number] = [22.3, 88.2, 22.8, 88.6]
const BENGALURU_BBOX: [number, number, number, number] = [12.8, 77.4, 13.2, 77.8]
const HYDERABAD_BBOX: [number, number, number, number] = [17.2, 78.2, 17.6, 78.7]
const AHMEDABAD_BBOX: [number, number, number, number] = [22.9, 72.4, 23.2, 72.8]
const PUNE_BBOX: [number, number, number, number] = [18.4, 73.7, 18.7, 74.0]
const KOCHI_BBOX: [number, number, number, number] = [9.8, 76.1, 10.1, 76.4]
const LUCKNOW_BBOX: [number, number, number, number] = [26.75, 80.8, 27.0, 81.1]
const JAIPUR_BBOX: [number, number, number, number] = [26.8, 75.7, 27.0, 76.0]
const NAGPUR_BBOX: [number, number, number, number] = [21.05, 79.0, 21.25, 79.2]

function pointToPolylineDist(pLat: number, pLon: number, coords: [number, number][]): number {
  let best = Infinity
  for (let i = 0; i < coords.length - 1; i++) {
    const d = pointToSegmentDist(pLat, pLon, coords[i][1], coords[i][0], coords[i + 1][1], coords[i + 1][0])
    if (d < best) best = d
  }
  return best
}

// ── Load Living Atlas railway + metro ──

interface RailFeat {
  coords: [number, number][]
  speed: number
  type: string    // gauge
  zone: string
  lanes: number
  isMetro: boolean
}

function loadLivingAtlasRails(): RailFeat[] {
  const out: RailFeat[] = []
  if (existsSync(RAILWAY_NETWORK_GEOJSON)) {
    const fc = JSON.parse(readFileSync(RAILWAY_NETWORK_GEOJSON, 'utf-8'))
    for (const f of fc.features || []) {
      const g = f.geometry
      if (!g) continue
      let coords: [number, number][] = []
      if (g.type === 'LineString') coords = g.coordinates
      else if (g.type === 'MultiLineString') coords = g.coordinates[0] || []
      if (coords.length < 2) continue
      out.push({
        coords,
        speed: (f.properties?.speed) ?? 0,
        type: (f.properties?.type || '').trim(),
        zone: (f.properties?.railwayzone || '').trim(),
        lanes: (f.properties?.nooflanes) ?? 1,
        isMetro: false,
      })
    }
  }
  if (existsSync(METRO_LINES_GEOJSON)) {
    const fc = JSON.parse(readFileSync(METRO_LINES_GEOJSON, 'utf-8'))
    for (const f of fc.features || []) {
      const g = f.geometry
      if (!g) continue
      let coords: [number, number][] = []
      if (g.type === 'LineString') coords = g.coordinates
      else if (g.type === 'MultiLineString') {
        // Concatenate all parts
        for (const part of g.coordinates) coords.push(...part)
      }
      if (coords.length < 2) continue
      out.push({
        coords, speed: 80, type: 'Standard Gauge', zone: 'Metro', lanes: 2, isMetro: true,
      })
    }
  }
  return out
}

function buildGrid(features: RailFeat[]): Map<string, RailFeat[]> {
  const grid = new Map<string, RailFeat[]>()
  for (const feat of features) {
    const seen = new Set<string>()
    for (const [lon, lat] of feat.coords) {
      const key = `${Math.floor(lat * 100)}_${Math.floor(lon * 100)}`
      if (!seen.has(key)) {
        seen.add(key)
        if (!grid.has(key)) grid.set(key, [])
        grid.get(key)!.push(feat)
      }
    }
  }
  return grid
}

function nearestRail(midLat: number, midLon: number, grid: Map<string, RailFeat[]>, radiusM: number): RailFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: RailFeat | null = null
  let bestDist = radiusM
  for (let dy = -reach; dy <= reach; dy++) {
    for (let dx = -reach; dx <= reach; dx++) {
      const cell = grid.get(`${baseLat + dy}_${baseLon + dx}`)
      if (!cell) continue
      for (const feat of cell) {
        const d = pointToPolylineDist(midLat, midLon, feat.coords)
        if (d < bestDist) { bestDist = d; best = feat }
      }
    }
  }
  return best
}

// ── Trains/day logic ──

function trainsFromFeature(feat: RailFeat, midLat: number, midLon: number): { pax: number; frt: number } {
  if (feat.isMetro) return { pax: 400, frt: 0 }
  const gauge = (feat.type || '').toLowerCase()
  if (gauge.includes('narrow') || gauge.includes('metre')) return { pax: 5, frt: 0 }

  // Mumbai Suburban (Central + Western + Harbour lines) — the world's busiest commuter rail
  if (inBbox(midLat, midLon, MUMBAI_BBOX)) {
    const z = feat.zone.toLowerCase()
    if (z.includes('central') || z.includes('western')) return { pax: 1300, frt: 30 }
  }
  // Delhi suburban (Northern Railway within NCR)
  if (inBbox(midLat, midLon, DELHI_BBOX) && feat.zone.toLowerCase().includes('northern')) {
    return { pax: 350, frt: 20 }
  }
  // Kolkata suburban (Eastern + South Eastern Railway)
  if (inBbox(midLat, midLon, KOLKATA_BBOX)) {
    const z = feat.zone.toLowerCase()
    if (z.includes('eastern')) return { pax: 500, frt: 25 }
  }
  // Chennai suburban (Southern Railway)
  if (inBbox(midLat, midLon, CHENNAI_BBOX) && feat.zone.toLowerCase().includes('southern')) {
    return { pax: 350, frt: 20 }
  }
  // Other metros get broad-gauge defaults (no dedicated suburban rail scaling)
  if (inBbox(midLat, midLon, BENGALURU_BBOX) ||
      inBbox(midLat, midLon, HYDERABAD_BBOX) ||
      inBbox(midLat, midLon, AHMEDABAD_BBOX) ||
      inBbox(midLat, midLon, PUNE_BBOX)) {
    return { pax: 60, frt: 20 }
  }

  // Speed-based defaults for the remaining broad gauge mainline network
  if (feat.speed >= 120) return { pax: 30, frt: 15 }  // Vande Bharat corridors
  if (feat.speed >= 100) return { pax: 25, frt: 15 }  // regular express
  if (feat.speed >= 60) return { pax: 15, frt: 10 }   // secondary lines
  return { pax: 8, frt: 5 }                            // local / low-speed
}

// Retract signature for stamps the pre-2026-07-10 fallback design wrote: IN's deleted
// class-default table (was named `classDefault` here — same disease as the other
// enrichers' `defaultTrains`, purged with them under task #26). A row still owned by
// MY_SOURCE_ID whose counts exactly equal its class tuple was filled by that fallback,
// not matched to a Living Atlas feature — exact-tuple + family ambiguity is negligible
// and the retract's `when` re-runs today's
// feature join, so a live-covered row is re-stamped by `match`, never disowned.
// No-match rows now return null: source_id stays 0 and the ENGINE default table
// (engine/noise-compute/src/emission/railway.rs::default_traffic) owns the "we don't
// know" case. DELETE this retract (and OLD_FALLBACK) after the world rail repaint
// confirms 0 retractions.
const OLD_FALLBACK = (railType: number, usage: number): [pax: number, frt: number] => {
  if (railType === 2) return [200, 0]  // light_rail (metro elevated)
  if (railType === 1) return [200, 0]  // tram
  if (railType === 3) return [10, 0]
  if (railType === 4) return [5, 0]
  if (usage === 1) return [5, 5]
  if (usage === 2) return [0, 10]
  return [12, 8]
}
const wasOldFallbackStamp = (row: RailRow): boolean => {
  const [pax, frt] = OLD_FALLBACK(row.railType, row.usage)
  return row.existingPax === pax && row.existingFrt === frt
}

async function main() {
  console.log(`=== IN Railway Enrichment — Living Atlas IR Network + Metros (${YEAR}) ===\n`)
  let rails = loadLivingAtlasRails()
  // COUNTRY GATE (#26C): a national feed can carry cross-border links (IR runs
  // into Pakistan/Bangladesh/Nepal border stations), so the raw feature list may
  // contain foreign polylines — joining those would stamp a neighbour's track
  // under this feed's id, and the same-rank higher-id tiebreak can beat the
  // neighbour's own national source (mechanism: the PL feed stamped 11,856 km of
  // CZ track, 7fac2349). A national feed only speaks for its own country's
  // network: entirely-foreign polylines are dropped BEFORE any grid is built (a
  // line with at least one vertex inside IN stays whole — its foreign-side rows
  // are guarded per-row by the match gates and the retract country arm).
  const inIn = makeCountryGate('IN')
  const rawCount = rails.length
  rails = rails.filter((feat) => feat.coords.some(([lon, lat]) => inIn(lat, lon)))
  if (rawCount !== rails.length) {
    console.log(`  country gate: ${rawCount - rails.length} foreign polylines dropped (cross-border links)`)
  }
  const metroFeats = rails.filter(r => r.isMetro)
  const irFeats = rails.filter(r => !r.isMetro)
  console.log(`  Loaded Living Atlas rails: ${rails.length} features (${metroFeats.length} metro + ${irFeats.length} IR)`)

  // Split the feed into per-family grids so cross-family inheritance is impossible:
  // heavy rail (rail_type 0) may only match IR network features; tram / light_rail
  // (rail_type 1/2) may only match metro features. (Mirrors th's tram/rail grids.)
  const railGrid = buildGrid(irFeats)
  const tramGrid = buildGrid(metroFeats)
  console.log(`  Spatial grid cells: ${railGrid.size} rail, ${tramGrid.size} tram/metro\n`)

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE input
  // snapshot — both Living Atlas cache files present AND parsed non-empty.
  // loadLivingAtlasRails silently skips a missing file (fine for enrichment:
  // matching simply stamps less), but a missing/empty family grid makes the
  // retract's feature corroboration read "no coverage" over that whole family and
  // disown REAL stamps. Only the retract is gated — never the stamping.
  const incompleteInputs: string[] = []
  if (!existsSync(RAILWAY_NETWORK_GEOJSON)) incompleteInputs.push(`missing ${RAILWAY_NETWORK_GEOJSON}`)
  else if (irFeats.length === 0) incompleteInputs.push('railway-network.geojson parsed to zero features')
  if (!existsSync(METRO_LINES_GEOJSON)) incompleteInputs.push(`missing ${METRO_LINES_GEOJSON}`)
  else if (metroFeats.length === 0) incompleteInputs.push('metro-lines.geojson parsed to zero features')
  const retractSafe = incompleteInputs.length === 0
  if (!retractSafe) logRetractSkippedIncompleteInputs(incompleteInputs.join('; '))

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (inBbox(lat, lon, IN_BBOX)) {
        if (existsSync(resolve(H3R4_DIR, hex, 'railways.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  IN-bbox hexes with railways.arrow: ${hexDirs.length}`)

  let totalRails = 0, skippedService = 0
  let matchedAtlas = 0, totalRetracted = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]

    // FAMILY routing (rail grid for rail_type 0, tram/metro grid for 1/2) →
    // Living Atlas nearest-feature match, all inside the match closure; no-match
    // rows return null (engine default_traffic owns unknowns). writeRailTrains owns
    // the service-skip, the priority gate, the retract self-heal, and the
    // byte-identical write.
    const r = await writeRailTrains(resolve(H3R4_DIR, hex, 'railways.arrow'), (row) => {
      if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null
      if (!inBbox(row.midLat, row.midLon, IN_BBOX)) return null

      const rt = row.railType

      // Heavy rail matches only IR features; tram/light_rail only metro features.
      // A null grid (narrow gauge / funicular) has no feed family → null (engine
      // default), so a tram near a mainline can never inherit the mainline's count.
      const grid = rt === 0 ? railGrid : rt === 1 || rt === 2 ? tramGrid : null
      if (grid) {
        const near = nearestRail(row.midLat, row.midLon, grid, 500)
        if (near) {
          const t = trainsFromFeature(near, row.midLat, row.midLon)
          matchedAtlas++
          return { pax: t.pax, frt: t.frt, sourceId: MY_SOURCE_ID }
        }
      }

      // No Living Atlas match (or unhandled rail_type): return null — the row
      // stays/goes source_id=0 and the ENGINE default table
      // (emission/railway.rs::default_traffic) owns the unknown. Never stamp a
      // guess under MY_SOURCE_ID.
      return null
    }, undefined,
    // CRITICAL-1b: retract only over a provably complete snapshot (retractSafe) —
    // with a missing/empty Living Atlas file, "no feature covers this row" is an
    // input artifact, not evidence, and would disown REAL stamps.
    retractSafe ? {
      sourceId: MY_SOURCE_ID,
      // Disown a legacy pre-2026-07-10 class-default stamp ONLY when today's join no
      // longer reaches the row (same family routing + 500 m feature join as `match`) —
      // a row a live feature still covers is re-stamped with the real count instead.
      when: (row) => {
        // Country-bleed disown (#26C): ANY owned row physically wholly outside IN (start+mid+end — genuine border-straddlers stay ours; shared R9 predicate) is
        // foreign track this feed must not speak for — even when its count was
        // a real through-train figure, ownership belongs to the local country's
        // own timetable (its national enricher re-stamps on its next run).
        if (segmentWhollyOutside(inIn, row.midLat, row.midLon, row.startLat, row.startLon, row.endLat, row.endLon)) return true
        if (!wasOldFallbackStamp(row)) return false
        const grid = row.railType === 0 ? railGrid : row.railType === 1 || row.railType === 2 ? tramGrid : null
        return !grid || nearestRail(row.midLat, row.midLon, grid, 500) === null
      },
    } : undefined,
    inIn, // #31.7 central country gate — see writeRailTrains
    )

    totalRails += r.rows
    totalRetracted += r.retracted
    skippedService += r.skippedService
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 50 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedAtlas} atlas, ${totalRetracted} retracted`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total rails scanned:        ${totalRails.toLocaleString()}`)
  console.log(`  Skipped service tracks:     ${skippedService.toLocaleString()}`)
  console.log(`  Matched by Living Atlas:    ${matchedAtlas.toLocaleString()} (${(100 * matchedAtlas / Math.max(totalRails, 1)).toFixed(2)}%)`)
  console.log(`  Retracted legacy defaults:  ${totalRetracted.toLocaleString()}`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

// Import-safe: run only when invoked directly — importing this file must never
// trigger a download/enrichment pass (pattern from enrich-roads-cz.ts).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
