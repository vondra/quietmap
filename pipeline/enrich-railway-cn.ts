/**
 * Enrich CN railways.arrow with Mainland (China Railway + Metro) ArcGIS data.
 *
 * Source: `services7.arcgis.com/m6uLpqj7MgjPU371/arcgis/rest/services/
 * Mainland/FeatureServer` by user `hanzheng1994` — a community-maintained
 * ArcGIS Online mirror of the China Railway and urban metro networks.
 *
 * Layers used:
 *
 * 1. **Mainland/4** — 国家铁路线 (national railway lines)
 *    2,328 polylines with `Name`, `Opaerator` (CR-Beijing, CR-Guangzhou, etc.),
 *    `ServiceType` (National Rail / Regional Passenger Rail), `DesignSpeed`,
 *    `TopSpeed` (in km/h: 100/120/160/200/250/300/350), `Electrification`,
 *    `ROW` (right-of-way), `Status` (运营中=operating, 建设中=under construction).
 *    Includes the **42,000 km Chinese HSR network** — Beijing↔Shanghai HSR,
 *    Beijing↔Guangzhou↔Hong Kong HSR, Shanghai↔Kunming HSR, Lanzhou↔Xinjiang HSR,
 *    etc. TopSpeed=350 corresponds to Fuxing trainsets on the world's fastest
 *    commercial service (Beijing-Shanghai).
 *
 * 2. **Mainland/3** — 城市轨道交通线 (urban metro lines)
 *    1,256 polylines across 50+ Chinese cities with `ServiceType`:
 *      - Metro Heavy Rail: 842 (standard mass transit, Beijing/Shanghai/Guangzhou)
 *      - Metro Express Heavy Rail: 167 (airport + suburban express)
 *      - LRT / Metro Light Rail: 144
 *      - APM (automated people mover): 13
 *      - Streetcar: 16
 *      - BRT (bus rapid transit): 67 — excluded (not rail)
 *      - Tourist rail: 7
 *
 * 3. **Mainland/0** and **Mainland/1** — station points (2,398 national + 10,948 metro)
 *    Not directly used for matching (polylines cover the same corridors).
 *
 * ## CRITICAL — OSM Chinese metro tagging is `railway=rail`, NOT `subway`
 *
 * Unlike Dubai/Bangkok/Taipei/Delhi/etc., Chinese metros in OSM are tagged
 * `railway=rail`, confirmed with Guangzhou Metro Line 18. This means Chinese
 * metros ARE extracted into `railways.arrow` and CAN be enriched directly via
 * spatial match to the Mainland/3 layer. **China dodges the subway bug.**
 *
 * The trade-off: Chinese OSM rail is conflated with heavy rail (a metro and a
 * mainline are both `rail_type=0`), so the feed's own family tag is the only
 * way to tell metro from mainline. We split the feed into a national heavy-rail
 * grid (Mainland/4) and a metro grid (Mainland/3), then FAMILY-GATE the match:
 * an OSM `rail_type=0` row may match either grid (it's genuinely ambiguous —
 * nearest polyline wins), but a `rail_type=1/2` tram/light_rail row may match
 * the METRO grid ONLY. That gate makes cross-family inheritance impossible: a
 * surface tram can no longer pick up a neighbouring HSR line's ~180 trains/day.
 *
 * ## trains/day defaults
 *
 * From Mainland/4 (national rail):
 *   TopSpeed ≥ 350 (Fuxing HSR)     → 180 pax + 0 frt (pure HSR, Beijing-Shanghai)
 *   TopSpeed ≥ 300 (CRH380)         → 150 pax + 0 frt
 *   TopSpeed ≥ 250 (CRH2/5)         → 120 pax + 0 frt
 *   TopSpeed ≥ 200                  → 80 pax + 10 frt (mixed)
 *   TopSpeed ≥ 150                  → 50 pax + 20 frt (regional + freight)
 *   TopSpeed ≥ 100                  → 30 pax + 20 frt (mainline freight)
 *   TopSpeed < 100                  → 15 pax + 10 frt (local)
 *   TopSpeed = 0 (unknown)          → 25 pax + 10 frt (default)
 *
 * From Mainland/3 (urban metro):
 *   Metro Heavy Rail                → 500 pax + 0 frt (Beijing, Shanghai typical)
 *   Metro Express Heavy Rail        → 400 pax + 0 frt
 *   LRT / Metro Light Rail          → 300 pax + 0 frt
 *   APM                             → 500 pax + 0 frt (short high-frequency)
 *   Streetcar                       → 200 pax + 0 frt
 *   Tourist rail                    → 30 pax + 0 frt
 *   BRT                             → skip (not rail)
 *
 * Status filter: only 运营中 (operating). Under-construction/planned skipped.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-railway-cn.ts
 */

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { shouldOverwrite } from './lib/provenance.js'
import { writeRailTrains, type RailRow } from './lib/railways-arrow.js'
import { makeCountryGate, segmentWhollyOutside } from './lib/country-polygon.js'
import { cellToLatLng } from 'h3-js'
import { SOURCE_ID_CN_NATIONAL_RAILWAY } from './lib/source-ids.generated.js'
import { inBbox, pointToPolylineDist } from './lib/spatial.js'
import { logRetractSkippedIncompleteInputs } from './lib/gtfs-enrich-core.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_CN_NATIONAL_RAILWAY

const CACHE_DIR = resolve(import.meta.dirname, `../data/enrichment/${YEAR}/cn`)
// Hoisted so main() can verify input-file presence for the retractSafe gate.
const RAILWAY_NATIONAL_GEOJSON = resolve(CACHE_DIR, 'railway-national.geojson')
const METRO_LINES_GEOJSON = resolve(CACHE_DIR, 'metro-lines.geojson')

const CN_BBOX: [number, number, number, number] = [18.0, 73.0, 54.0, 135.5]

// Neighbour exclusion boxes DELETED (#32, /gg #31 round-2 Codex): the central
// writeRailTrains countryGate (exact CGAZ polygon) owns national scope now, and
// the hand rectangles provably clipped DOMESTIC territory (the CN 'Vietnam' box
// held Nanning, IN 'Pakistan' held Ahmedabad, TH 'Malaysia' held Sungai Kolok).

// ── Load Mainland rail + metro ──

interface RailFeat {
  coords: [number, number][]
  serviceType: string
  topSpeed: number
  isMetro: boolean
}

function loadRails(): RailFeat[] {
  const out: RailFeat[] = []

  if (existsSync(RAILWAY_NATIONAL_GEOJSON)) {
    const fc = JSON.parse(readFileSync(RAILWAY_NATIONAL_GEOJSON, 'utf-8'))
    for (const f of fc.features || []) {
      const g = f.geometry
      if (!g) continue
      const status = (f.properties?.Status || '').trim()
      if (status && status !== '运营中' && status !== 'operating') continue  // only operating
      let coords: [number, number][] = []
      if (g.type === 'LineString') coords = g.coordinates
      else if (g.type === 'MultiLineString') coords = g.coordinates[0] || []
      if (coords.length < 2) continue
      out.push({
        coords,
        serviceType: (f.properties?.ServiceType || '').trim(),
        topSpeed: (f.properties?.TopSpeed) ?? 0,
        isMetro: false,
      })
    }
  }

  if (existsSync(METRO_LINES_GEOJSON)) {
    const fc = JSON.parse(readFileSync(METRO_LINES_GEOJSON, 'utf-8'))
    for (const f of fc.features || []) {
      const g = f.geometry
      if (!g) continue
      const status = (f.properties?.Status || '').trim()
      if (status && status !== '运营中' && status !== 'operating') continue
      const serviceType = (f.properties?.ServiceType || '').trim()
      if (serviceType === 'BRT') continue  // skip bus rapid transit
      let coords: [number, number][] = []
      if (g.type === 'LineString') coords = g.coordinates
      else if (g.type === 'MultiLineString') {
        for (const part of g.coordinates) coords.push(...part)
      }
      if (coords.length < 2) continue
      out.push({
        coords,
        serviceType,
        topSpeed: (f.properties?.TopSpeed) ?? 80,
        isMetro: true,
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

/** Nearest feed polyline within `radiusM`, searching ONLY the supplied grids
 *  (the caller restricts which families are eligible — see the match closure). */
function nearestRail(midLat: number, midLon: number, grids: Map<string, RailFeat[]>[], radiusM: number): RailFeat | null {
  const reach = Math.max(1, Math.ceil(radiusM / 1000))
  const baseLat = Math.floor(midLat * 100)
  const baseLon = Math.floor(midLon * 100)
  let best: RailFeat | null = null
  let bestDist = radiusM
  for (const grid of grids) {
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
  }
  return best
}

// ── Train frequency by ServiceType/TopSpeed ──

function trainsFromFeature(feat: RailFeat): { pax: number; frt: number } {
  if (feat.isMetro) {
    const s = feat.serviceType
    if (s === 'Metro Heavy Rail') return { pax: 500, frt: 0 }
    if (s === 'Metro Express Heavy Rail') return { pax: 400, frt: 0 }
    if (s === 'LRT' || s === 'Metro Light Rail') return { pax: 300, frt: 0 }
    if (s === 'APM') return { pax: 500, frt: 0 }
    if (s === 'Streetcar') return { pax: 200, frt: 0 }
    if (s.includes('旅游')) return { pax: 30, frt: 0 }
    return { pax: 400, frt: 0 }  // unknown metro default
  }
  // National rail — speed-based
  const ts = feat.topSpeed || 0
  if (ts >= 350) return { pax: 180, frt: 0 }  // Fuxing HSR Beijing-Shanghai
  if (ts >= 300) return { pax: 150, frt: 0 }
  if (ts >= 250) return { pax: 120, frt: 0 }
  if (ts >= 200) return { pax: 80, frt: 10 }
  if (ts >= 150) return { pax: 50, frt: 20 }
  if (ts >= 100) return { pax: 30, frt: 20 }
  if (ts > 0) return { pax: 15, frt: 10 }
  return { pax: 25, frt: 10 }
}

// Retract signature for stamps the pre-2026-07-10 fallback design wrote: CN's deleted
// class-default table (was named `classDefault` here — same disease as the other
// enrichers' `defaultTrains`, purged with them under task #26). A row still owned by
// MY_SOURCE_ID whose counts exactly equal its class tuple was filled by that fallback,
// not matched to a Mainland polyline — exact-tuple + family ambiguity is negligible
// and the retract's `when` re-runs today's
// polyline join, so a live-covered row (e.g. a Streetcar at a real 200/0) is re-stamped
// by `match`, never disowned. No-match rows now return null: source_id stays 0 and the
// ENGINE default table (engine/noise-compute/src/emission/railway.rs::default_traffic)
// owns the "we don't know" case. DELETE this retract (and OLD_FALLBACK) after the world
// rail repaint confirms 0 retractions.
const OLD_FALLBACK = (railType: number, usage: number): [pax: number, frt: number] => {
  if (railType === 2) return [300, 0]  // light_rail
  if (railType === 1) return [200, 0]  // tram
  if (railType === 3) return [10, 0]
  if (railType === 4) return [5, 0]
  if (usage === 1) return [10, 10]
  if (usage === 2) return [0, 15]
  return [20, 15]  // mainline default for China
}
const wasOldFallbackStamp = (row: RailRow): boolean => {
  const [pax, frt] = OLD_FALLBACK(row.railType, row.usage)
  return row.existingPax === pax && row.existingFrt === frt
}

async function main() {
  console.log(`=== CN Railway Enrichment — Mainland (CR + metros) (${YEAR}) ===\n`)
  let rails = loadRails()
  // COUNTRY GATE (#26C): a national feed can carry cross-border links (China-Laos
  // Railway sections, KZ/MN/VN border connections), so the raw feature list may
  // contain foreign polylines — joining those would stamp a neighbour's track
  // under this feed's id, and the same-rank higher-id tiebreak can beat the
  // neighbour's own national source (mechanism: the PL feed stamped 11,856 km of
  // CZ track, 7fac2349). A national feed only speaks for its own country's
  // network: entirely-foreign polylines are dropped BEFORE any grid is built (a
  // line with at least one vertex inside CN stays whole — its foreign-side rows
  // are guarded per-row by the match gates and the retract country arm).
  const inCn = makeCountryGate('CN')
  const rawCount = rails.length
  rails = rails.filter((feat) => feat.coords.some(([lon, lat]) => inCn(lat, lon)))
  if (rawCount !== rails.length) {
    console.log(`  country gate: ${rawCount - rails.length} foreign polylines dropped (cross-border links)`)
  }
  const metroFeats = rails.filter(r => r.isMetro)
  const nationalFeats = rails.filter(r => !r.isMetro)
  console.log(`  Loaded: ${rails.length} (${nationalFeats.length} national rail + ${metroFeats.length} metro)`)

  // FAMILY-SPLIT grids: a metro/light-rail grid and a national heavy-rail grid,
  // tagged from the feed itself (Mainland/3 = metro, Mainland/4 = heavy rail).
  // The per-row FAMILY GATE in the match closure decides which grids an OSM row
  // may inherit from — crucially, a tram (rail_type 1/2) is barred from the
  // national grid, killing the family-blind bug that handed a tram up to ~180
  // trains/day. Mirrors th's tram-grid vs rail-grid split.
  const metroGrid = buildGrid(metroFeats)
  const nationalGrid = buildGrid(nationalFeats)
  console.log(`  Grid cells: national=${nationalGrid.size}, metro=${metroGrid.size}\n`)

  // CRITICAL-1b (/gg Codex): a retract may only run over a PROVABLY COMPLETE input
  // snapshot — both Mainland cache files present AND parsed to non-empty operating
  // sets. loadRails silently skips a missing file (fine for enrichment: matching
  // simply stamps less), but a missing/empty family grid makes the retract's
  // polyline corroboration read "no coverage" over that whole family and disown
  // REAL stamps. Only the retract is gated — never the stamping.
  const incompleteInputs: string[] = []
  if (!existsSync(RAILWAY_NATIONAL_GEOJSON)) incompleteInputs.push(`missing ${RAILWAY_NATIONAL_GEOJSON}`)
  else if (nationalFeats.length === 0) incompleteInputs.push('railway-national.geojson parsed to zero operating lines')
  if (!existsSync(METRO_LINES_GEOJSON)) incompleteInputs.push(`missing ${METRO_LINES_GEOJSON}`)
  else if (metroFeats.length === 0) incompleteInputs.push('metro-lines.geojson parsed to zero operating lines')
  const retractSafe = incompleteInputs.length === 0
  if (!retractSafe) logRetractSkippedIncompleteInputs(incompleteInputs.join('; '))

  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  const hexDirs: string[] = []
  for (const hex of allHexes) {
    try {
      const [lat, lon] = cellToLatLng(hex)
      if (inBbox(lat, lon, CN_BBOX)) {
        if (existsSync(resolve(H3R4_DIR, hex, 'railways.arrow'))) hexDirs.push(hex)
      }
    } catch {}
  }
  console.log(`  CN-bbox hexes with railways.arrow: ${hexDirs.length}`)

  let totalRails = 0, skippedService = 0
  let matchedMainland = 0, totalRetracted = 0
  let hexesUpdated = 0
  const startTime = Date.now()

  for (let hi = 0; hi < hexDirs.length; hi++) {
    const hex = hexDirs[hi]

    // FAMILY-gated routing (see the gate comment in the closure) → nearest-polyline
    // match, all inside the match closure; no-match rows return null (engine
    // default_traffic owns unknowns). writeRailTrains owns the service-skip, the
    // priority gate, the retract self-heal, and the byte-identical write.
    const r = await writeRailTrains(resolve(H3R4_DIR, hex, 'railways.arrow'), (row) => {
      if (!shouldOverwrite(row.existingSourceId, MY_SOURCE_ID)) return null
      if (!inBbox(row.midLat, row.midLon, CN_BBOX)) return null

      const rt = row.railType

      // FAMILY GATE — which feed grids this OSM row may inherit from.
      //   rt 1 (tram) / rt 2 (light_rail) ⇒ METRO grid ONLY. A surface tram can
      //     NEVER match a national heavy/HSR polyline — that was the family-blind
      //     bug (a tram sitting near a mainline inherited up to ~180 trains/day).
      //   rt 0 (rail) ⇒ national OR metro. Chinese metros are tagged
      //     `railway=rail` in OSM (→ rt 0, confirmed Guangzhou Line 18), so a rt-0
      //     row is genuinely either a mainline or a metro; the feed's own family
      //     tag (Mainland/4 vs /3) is the only disambiguator, so both are eligible
      //     and nearest-polyline wins. No cross-family leak: rt 1/2 still can't
      //     reach the national grid.
      //   rt 3/4 (narrow_gauge / funicular) ⇒ no feed family → null (engine default).
      const grids = rt === 1 || rt === 2 ? [metroGrid] : rt === 0 ? [nationalGrid, metroGrid] : []
      if (grids.length > 0) {
        const near = nearestRail(row.midLat, row.midLon, grids, 500)
        if (near) {
          const t = trainsFromFeature(near)
          matchedMainland++
          return { pax: t.pax, frt: t.frt, sourceId: MY_SOURCE_ID }
        }
      }

      // No Mainland match (or unhandled rail_type): return null — the row stays/goes
      // source_id=0 and the ENGINE default table (emission/railway.rs::default_traffic)
      // owns the unknown. Never stamp a guess under MY_SOURCE_ID.
      return null
    }, undefined,
    // CRITICAL-1b: retract only over a provably complete snapshot (retractSafe) —
    // with a missing/empty Mainland file, "no polyline covers this row" is an input
    // artifact, not evidence, and would disown REAL stamps.
    retractSafe ? {
      sourceId: MY_SOURCE_ID,
      // Disown a legacy pre-2026-07-10 class-default stamp ONLY when today's join no
      // longer reaches the row (same family routing + 500 m polyline join as `match`) —
      // a row a live polyline still covers is re-stamped with the real count instead.
      when: (row) => {
        // Country-bleed disown (#26C): ANY owned row physically wholly outside CN (start+mid+end — genuine border-straddlers stay ours; shared R9 predicate) is
        // foreign track this feed must not speak for — even when its count was
        // a real through-train figure, ownership belongs to the local country's
        // own timetable (its national enricher re-stamps on its next run).
        if (segmentWhollyOutside(inCn, row.midLat, row.midLon, row.startLat, row.startLon, row.endLat, row.endLon)) return true
        if (!wasOldFallbackStamp(row)) return false
        const grids = row.railType === 1 || row.railType === 2 ? [metroGrid] : row.railType === 0 ? [nationalGrid, metroGrid] : []
        return grids.length === 0 || nearestRail(row.midLat, row.midLon, grids, 500) === null
      },
    } : undefined,
    inCn, // #31.7 central country gate — see writeRailTrains
    )

    totalRails += r.rows
    totalRetracted += r.retracted
    skippedService += r.skippedService
    if (r.updated) hexesUpdated++

    const elapsed = Date.now() - startTime
    if (elapsed > 10_000 && hi % 50 === 0) {
      console.log(`  [${(elapsed / 1000).toFixed(0)}s] ${hi + 1}/${hexDirs.length}, ${matchedMainland} mainland, ${totalRetracted} retracted`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Total rails scanned:        ${totalRails.toLocaleString()}`)
  console.log(`  Skipped service tracks:     ${skippedService.toLocaleString()}`)
  console.log(`  Matched by Mainland:        ${matchedMainland.toLocaleString()} (${(100 * matchedMainland / Math.max(totalRails, 1)).toFixed(2)}%)`)
  console.log(`  Retracted legacy defaults:  ${totalRetracted.toLocaleString()}`)
  console.log(`  Hexes updated:              ${hexesUpdated}/${hexDirs.length}`)
}

// Import-safe: run only when invoked directly — importing this file must never
// trigger a download/enrichment pass (pattern from enrich-roads-cz.ts).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
