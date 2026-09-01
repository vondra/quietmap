/**
 * Enrich railways.arrow with real train frequencies from GTFS feeds — the
 * continental multi-country aggregate.
 *
 * HEAVY RAIL (rail_type 0) now runs on the shared graph-walk driver
 * (`lib/rail-walk-enrich.ts`): per feed, `computeStopPairFrequenciesForFeed`
 * (lib/gtfs-stop-pairs.ts) turns stop_times.txt into station-PAIR frequencies;
 * every feed contributes through that ONE shared pair accumulator
 * (`enrichRailwaysByGraphWalk`'s own canonical accumulator does the final
 * SUM). Cross-feed overlap policy is a REGISTRY property: fr-idf is declared
 * tram-only (see its entry — the 2026-07-16 trip-fingerprint mirror dedup was
 * deleted on the 7-shared-keys data verdict documented there).
 * TRAM/light-rail (rail_type 1/2) keeps the pre-Phase-4
 * `nearestGridStop` 500 m stop-join (stop spacing is well under the radius —
 * banding was a heavy-rail-only bug), wired in as the
 * walk driver's `extraMatch` fallback arm via `buildTramExtraMatch` — hoisted
 * to `lib/gtfs-enrich-core.ts` so every national `enrich-railway-{cc}.ts`
 * enricher shares the SAME implementation (2026-07-16 Phase 4 point 2).
 *
 * PER-COUNTRY EXECUTION: `--country
 * CC` processes ONLY that country's own feed(s) against a border-buffered
 * rail graph (union of its feeds' `boundingBox`, padded 0.5°) — fixes
 * `country:XX` chain scope silently mutating ALL of Europe. No argument runs
 * EVERY country found in FEEDS, SEQUENTIALLY, each through the exact same
 * per-country path `processCountry` uses for `--country`; the world-wide
 * legacy OLD_FALLBACK retract sweep (CRITICAL-2, unchanged) stays but runs
 * ONLY after that all-countries loop, never under `--country`.
 *
 * #26C-scoped ownership: unlike the old whole-aggregate design, a per-country
 * run now DOES gate by `makeCountryGate(cc)` (#31.7) — the country scoping
 * this rewrite adds makes "which country does this row belong to" answerable
 * per invocation, so the previous "can't wire a country gate on a
 * multi-country blob" objection no longer applies once each country runs on
 * its own feed(s) alone.
 *
 * WHY (unchanged): segments the feeds don't reach STAY at source_id=0 on
 * purpose (engine defaults own unknowns — no class-default stamping under
 * this source id, and Europe is not silent-residual-capable, #26B — that
 * exists only for CZ's timetable, owner decision 2026-07-11 option b).
 *
 * Usage:
 *   npx tsx enrich-railway-europe.ts                    # all countries, sequential
 *   npx tsx enrich-railway-europe.ts --country DE       # Germany only
 *   npx tsx enrich-railway-europe.ts --country FR --stamp-only
 *   npx tsx enrich-railway-europe.ts --force-download
 *   npx tsx enrich-railway-europe.ts --enrich-only
 *   npx tsx enrich-railway-europe.ts --feed=de          # manual debug: one feed only
 *   npx tsx enrich-railway-europe.ts --feed=de,ch,at    # manual debug: several feeds
 *
 * --stamp-only: Phase-3/4 Step A rollout control (same semantics as
 * enrich-railway-cz.ts) — enableDestructive=false: the run still walk-stamps
 * heavy-rail counts AND divisors atomically and still runs the tram stop-join, but NO retract
 * (per-country or the world CRITICAL-2 sweep) and NO country-bleed heal fire.
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync, createReadStream, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execSync } from 'node:child_process'
import { gunzipSync } from 'node:zlib'
import { createInterface } from 'node:readline'
import { latLngToCell } from 'h3-js'
import { SOURCE_ID_GLOBAL_GTFS_TRANSIT } from './lib/source-ids.generated.js'
import { writeRailTrains, type RailRow } from './lib/railways-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { enrichRailwaysByGraphWalk } from './lib/rail-walk-enrich.js'
import type { RailStationPairCount } from './lib/rail-graph.js'
import { computeStopPairFrequenciesForFeed } from './lib/gtfs-stop-pairs.js'
import {
  RAIL_TYPES, TRAM_TYPES, METRO_TYPES, routeFamily,
  parseGtfsDate, formatDate, logRetractSkippedIncompleteInputs, GTFS_BORDER_MARGIN_DEG, type GtfsStop,
  dedupeStopsByLocation, buildTramExtraMatch, declaredRouteFamiliesForFeed, describeIncompleteFamilies,
} from './lib/gtfs-enrich-core.js'
// Re-exported for enrich-railway-europe.test.ts and any other importer that
// still names this module — the implementations live in gtfs-enrich-core.ts
// (2026-07-16 Phase 4 hoist) so every national enrich-railway-{cc}.ts shares
// them too, never 17 copies.
export { dedupeStopsByLocation, buildTramExtraMatch }
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_GLOBAL_GTFS_TRANSIT

const CACHE_DIR = resolve(import.meta.dirname, '../data/enrichment/global/gtfs')

const forceDownload = process.argv.includes('--force-download')
const enrichOnly = process.argv.includes('--enrich-only')
const stampOnly = process.argv.includes('--stamp-only')

// Parse --feed=xx,yy argument — manual debug knob (chain runs never pass it;
// see the manifest note on the 'railway-europe*' steps): narrows the feed
// list within whichever country/all-countries selection is also in effect.
const feedArg = process.argv.find(a => a.startsWith('--feed='))
const requestedFeeds = feedArg ? feedArg.replace('--feed=', '').split(',').map(s => s.trim().toLowerCase()) : null

// --country CC (bare `--country XX` or `--country=XX`, either spelling).
const countryArgEq = process.argv.find(a => a.startsWith('--country='))
const countryArgIdx = process.argv.indexOf('--country')
const countryArg = (countryArgEq ? countryArgEq.slice('--country='.length) : countryArgIdx >= 0 ? process.argv[countryArgIdx + 1] : undefined)
  ?.toUpperCase()

// ── Feed registry ──

interface FeedConfig {
  id: string
  name: string
  url: string
  country: string       // ISO 3166-1 alpha-2 this feed testifies about — its PRIMARY
                         // country even for a feed whose name suggests otherwise
                         // (none of today's 23 are genuinely multi-country; if one
                         // ever is, this is still the country the per-country
                         // `processCountry` run attributes it to).
  boundingBox: [number, number, number, number]  // [minLat, minLon, maxLat, maxLon] for sanity checks
  // GTFS route_type values that count as rail (2=rail, 0=tram, 1=subway, etc)
  railRouteTypes: Set<number>
  // Map this feed's METRO_TYPES routes to the 'rail' family (heavy rail) instead
  // of the default 'tram'/light_rail. Set where the operator tags heavy suburban
  // rail as GTFS "metro" (400-405) but OSM carries it as railway=rail — Melbourne
  // Metro Trains (au-vic feed 2, 35 route_type-400 routes) are the reference case.
  metroAsRail?: boolean
  // Cross-feed overlap policy: pairs from every feed are always SUMMED by the
  // walk's canonical accumulator. A feed that would double-publish another
  // feed's heavy rail must instead narrow its OWN railRouteTypes allow-list so
  // the overlapping family never leaves it (fr-idf is the reference case — see
  // its entry). The 2026-07-16 mirrorGroup/trip-fingerprint dedup was deleted
  // the same day: FR national × IDF caches share only 7 of 3 537/6 193
  // coordinate keys, so cross-producer fingerprints could never match.
}

// RAIL_TYPES/TRAM_TYPES/METRO_TYPES + routeFamily are hoisted to lib/gtfs-enrich-core.ts.
// Per-feed allow-list value: feeds with surface trams/light-rail/metro use this, rail-only feeds use
// RAIL_TYPES. METRO_TYPES is included so metro routes survive the per-feed filter and routeFamily can
// map them to the 'tram' family (they enrich OSM light_rail, rail_type 2) — without it, metro-bearing
// feeds (e.g. Sofia) would fall back to class defaults instead of real frequencies.
const ALL_RAIL_AND_TRAM = new Set([...RAIL_TYPES, ...TRAM_TYPES, ...METRO_TYPES])

/** The OSM rail family a route of this type maps to FOR THIS FEED, or null if the
 *  feed doesn't count that type. Gates on the feed's allow-list first, then
 *  overrides metro→'rail' for feeds that carry heavy suburban rail under GTFS
 *  metro types (metroAsRail), else defers to the global routeFamily. */
export function railFamilyFor(routeType: number, feed: FeedConfig): 'rail' | 'tram' | null {
  if (!feed.railRouteTypes.has(routeType)) return null
  if (feed.metroAsRail && METRO_TYPES.has(routeType)) return 'rail'
  return routeFamily(routeType)
}

export const FEEDS: FeedConfig[] = [
  {
    id: 'de',
    name: 'Germany (DELFI)',
    url: 'https://data.public-transport.earth/gtfs/de',
    country: 'DE',
    boundingBox: [47.2, 5.8, 55.1, 15.1],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'ch',
    name: 'Switzerland (opentransportdata.swiss)',
    url: 'https://data.public-transport.earth/gtfs/ch',
    country: 'CH',
    boundingBox: [45.8, 5.9, 47.9, 10.5],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'at',
    name: 'Austria',
    url: 'https://static.web.oebb.at/open-data/soll-fahrplan-gtfs/GTFS_OP_2025_obb.zip',
    country: 'AT',
    boundingBox: [46.3, 9.5, 49.0, 17.2],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'nl',
    name: 'Netherlands',
    url: 'https://data.public-transport.earth/gtfs/nl',
    country: 'NL',
    boundingBox: [50.7, 3.3, 53.6, 7.3],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'se',
    name: 'Sweden',
    url: 'https://data.public-transport.earth/gtfs/se',
    country: 'SE',
    boundingBox: [55.3, 10.9, 69.1, 24.2],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'no',
    name: 'Norway',
    url: 'https://data.public-transport.earth/gtfs/no',
    country: 'NO',
    boundingBox: [57.9, 4.5, 71.2, 31.2],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'fi',
    name: 'Finland',
    // Fintraffic/Digitraffic; requires Accept-Encoding: gzip (handled in downloadFeed).
    // Old public-transport.earth/gtfs/fi went dead 2026-06.
    url: 'https://rata.digitraffic.fi/api/v1/trains/gtfs-all.zip',
    country: 'FI',
    boundingBox: [59.7, 19.1, 70.1, 31.6],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'be',
    name: 'Belgium',
    url: 'https://gtfs.irail.be/nmbs/gtfs/latest.zip',
    country: 'BE',
    boundingBox: [49.5, 2.5, 51.5, 6.4],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'in',
    name: 'India (Indian Railways, unofficial)',
    url: 'https://github.com/Neo2308/indianrailways-gtfs/raw/refs/heads/main/gtfs/gtfs.zip',
    country: 'IN',
    boundingBox: [6.7, 68.1, 35.7, 97.4],
    railRouteTypes: RAIL_TYPES,
  },
  {
    id: 'us',
    name: 'USA (Amtrak)',
    url: 'https://content.amtrak.com/content/gtfs/GTFS.zip',
    country: 'US',
    boundingBox: [24.5, -125.0, 49.0, -66.9],
    railRouteTypes: RAIL_TYPES,
  },
  {
    id: 'ca',
    name: 'Canada (VIA Rail)',
    url: 'https://www.viarail.ca/sites/all/files/gtfs/viarail.zip',
    country: 'CA',
    boundingBox: [41.7, -141.0, 60.0, -52.6],
    railRouteTypes: RAIL_TYPES,
  },
  {
    id: 'fr',
    name: 'France (SNCF TGV + IC + TER)',
    url: 'https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip',
    country: 'FR',
    boundingBox: [41.3, -5.2, 51.1, 9.6],
    railRouteTypes: ALL_RAIL_AND_TRAM,
    // THE national heavy-rail authority for FR, Île-de-France included: this
    // SNCF export already carries the Transilien/RER trains, so 'fr-idf' is
    // declared tram-only (see its entry) instead of being deduped against
    // this feed — the deleted 2026-07-16 trip-fingerprint dedup could never
    // match the two producers (7 shared coordinate keys of 3 537/6 193).
  },
  {
    id: 'lu',
    name: 'Luxembourg (CFL + Luxtram + buses)',
    url: 'https://files.mobilitydatabase.org/mdb-1108/latest.zip',
    country: 'LU',
    boundingBox: [49.4, 5.7, 50.2, 6.6],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'gr',
    name: 'Greece (TrainOSE / Hellenic Train, archived 2019)',
    url: 'https://files.mobilitydatabase.org/mdb-1161/latest.zip',
    country: 'GR',
    boundingBox: [34.8, 19.3, 41.8, 29.8],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'lv-pv',
    name: 'Latvia (Pasažieru Vilciens / Vivi rail)',
    url: 'https://files.mobilitydatabase.org/mdb-2015/latest.zip',
    country: 'LV',
    boundingBox: [55.6, 20.9, 58.1, 28.2],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'ee',
    name: 'Estonia (Peatus.ee national)',
    url: 'https://files.mobilitydatabase.org/mdb-1095/latest.zip',
    country: 'EE',
    boundingBox: [57.5, 21.8, 59.7, 28.2],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'bg-sofia',
    name: 'Bulgaria Sofia (Sofia Traffic metro+tram)',
    url: 'https://gtfs.sofiatraffic.bg/api/v1/static',
    country: 'BG',
    boundingBox: [42.5, 23.1, 42.8, 23.6],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'hr',
    name: 'Croatia (HŽ Putnički)',
    url: 'https://www.hzpp.hr/GTFS_files.zip',
    country: 'HR',
    boundingBox: [42.3, 13.4, 46.6, 19.5],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'hu',
    name: 'Hungary (MÁV-START via MenetBrand)',
    url: 'https://gtfs.menetbrand.com/download/mav/',
    country: 'HU',
    boundingBox: [45.7, 16.1, 48.6, 22.9],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'sk',
    name: 'Slovakia (ZSR/ŽSR)',
    // NKOD national open-data portal (distribution of dataset ebeeedf1-…).
    // Old zsr.sk direct .zip went dead 2026-06 (returns an HTML page).
    url: 'https://data.slovensko.sk/download?id=f63ef0f2-c4e7-496b-bdd1-44ba0e9438e9',
    country: 'SK',
    boundingBox: [47.7, 16.8, 49.6, 22.6],
    railRouteTypes: ALL_RAIL_AND_TRAM,
  },
  {
    id: 'fr-idf',
    name: 'France Île-de-France (Transilien)',
    url: 'https://eu.ftp.opendatasoft.com/sncf/gtfs/transilien-gtfs.zip',
    country: 'FR',
    boundingBox: [48.1, 1.4, 49.2, 3.6],
    // TRAM/LIGHT-RAIL ONLY (2026-07-16 /gg fix batch item 2): this feed's
    // heavy-rail (Transilien/RER) trains are ALREADY published in the
    // national 'fr' feed above, so counting them here would double-stamp
    // every Île-de-France line; only tram/light-rail route_types (SNCF
    // tram-train services) remain counted from this feed. The alternative —
    // trip-fingerprint dedup against 'fr' — was built and DELETED the same
    // day on a data verdict: the two producers' caches share only 7 of
    // 3 537/6 193 coordinate keys, so fingerprints could never match, while
    // the dedup path also bypassed the shape-conflict drop and false-merged
    // within-feed duplicate trips. Being tram-only, this feed never makes
    // heavy-rail retract claims (see feedDeclaresHeavyRail).
    railRouteTypes: TRAM_TYPES,
  },
  {
    id: 'au-vic',
    // PTV publishes ONE gtfs.zip that unzips into mode-split subfeeds
    // (1=V/Line regional rail, 2=Metro Trains, 3=Yarra Trams, 4-6/10-11=bus,
    // 10=interstate rail) each as its own <N>/google_transit.zip. downloadGtfs
    // extracts + processes the rail-bearing subfeeds; RAIL∪METRO keeps V/Line
    // (type 2) AND Metro Trains (type 400) — with metroAsRail the metro maps to
    // heavy rail (OSM railway=rail), not light_rail. Trams (type 0) stay out.
    name: 'Australia Victoria (PTV V/Line + Metro Trains)',
    url: 'https://data.ptv.vic.gov.au/downloads/gtfs.zip',
    country: 'AU',
    boundingBox: [-39.2, 140.9, -33.9, 150.0],
    railRouteTypes: new Set([...RAIL_TYPES, ...METRO_TYPES]),
    metroAsRail: true,
    // No overlap with au-qld: Victoria vs Queensland are geographically
    // disjoint networks (boundingBoxes don't overlap) — no shared physical
    // train can appear in both, so the plain SUM is already correct.
  },
  {
    id: 'au-qld',
    name: 'Australia Queensland (TransLink)',
    url: 'https://gtfsrt.api.translink.com.au/GTFS/SEQ_GTFS.zip',
    country: 'AU',
    boundingBox: [-29.2, 150.5, -26.0, 153.6],
    railRouteTypes: RAIL_TYPES,
    // See au-vic's comment — disjoint network, plain SUM.
  },
]

// ── Types (GtfsStop hoisted to lib/gtfs-enrich-core.ts) ──

export interface StopTrainCount {
  stop_id: string
  lat: number
  lon: number
  name: string
  h3r4: string
  family: 'rail' | 'tram'
  trains_passenger: number
  trains_freight: number   // GTFS rarely has freight, but keep for consistency
}

// dedupeStopsByLocation now lives in lib/gtfs-enrich-core.ts (2026-07-16 Phase 4
// hoist) — imported above and re-exported for this file's own test importers.

// ── GTFS parsing ──

/** Stream-parse a large CSV file line by line. Returns array of objects. */
async function parseCsvStream(filePath: string): Promise<Record<string, string>[]> {
  const results: Record<string, string>[] = []
  const stream = createReadStream(filePath, { encoding: 'utf-8' })
  const rl = createInterface({ input: stream, crlfDelay: Infinity })

  let headers: string[] | null = null
  for await (const rawLine of rl) {
    // Strip BOM from first line
    const line = headers === null ? rawLine.replace(/^\uFEFF/, '') : rawLine
    if (line.trim() === '') continue

    if (!headers) {
      headers = parseCsvLine(line)
      continue
    }
    const values = parseCsvLine(line)
    const row: Record<string, string> = {}
    for (let i = 0; i < headers.length; i++) {
      row[headers[i]] = values[i] || ''
    }
    results.push(row)
  }
  return results
}

/** Parse a single CSV line, handling quoted fields with commas. */
function parseCsvLine(line: string): string[] {
  const fields: string[] = []
  let current = ''
  let inQuotes = false
  for (let i = 0; i < line.length; i++) {
    const ch = line[i]
    if (inQuotes) {
      if (ch === '"') {
        if (i + 1 < line.length && line[i + 1] === '"') {
          current += '"'
          i++ // skip escaped quote
        } else {
          inQuotes = false
        }
      } else {
        current += ch
      }
    } else {
      if (ch === '"') {
        inQuotes = true
      } else if (ch === ',') {
        fields.push(current.trim())
        current = ''
      } else {
        current += ch
      }
    }
  }
  fields.push(current.trim())
  return fields
}

/** Find a Wednesday within the GTFS calendar validity period. */
function findTargetWednesday(calendarRows: Record<string, string>[]): string {
  // Count how many services are active on each Wednesday in the calendar range.
  // Pick the Wednesday with the most active services (= busiest typical day).
  // This avoids stale dates when feeds span years but only recent services are active.

  // Build list of (start_date, end_date, wednesday_flag) per service
  const services: { start: string; end: string; wed: boolean }[] = []
  for (const row of calendarRows) {
    const start = row['start_date'] || ''
    const end = row['end_date'] || ''
    const wed = (row['wednesday'] || '0') === '1'
    if (start && end) services.push({ start, end, wed })
  }

  if (services.length === 0) {
    const now = new Date()
    now.setDate(now.getDate() + 7)
    while (now.getDay() !== 3) now.setDate(now.getDate() + 1)
    return now.toISOString().substring(0, 10).replace(/-/g, '')
  }

  // Find overall date range
  const allStarts = services.map(s => s.start).sort()
  const allEnds = services.map(s => s.end).sort()
  const minDate = allStarts[0]
  const maxDate = allEnds[allEnds.length - 1]

  // Sample Wednesdays across the range (weekly), count active services
  const startMs = parseGtfsDate(minDate)
  const endMs = parseGtfsDate(maxDate)
  const d = new Date(startMs)
  while (d.getDay() !== 3) d.setDate(d.getDate() + 1)

  let bestDate = ''
  let bestCount = 0
  while (d.getTime() <= endMs) {
    const ds = d.toISOString().substring(0, 10).replace(/-/g, '')
    let count = 0
    for (const s of services) {
      if (s.wed && ds >= s.start && ds <= s.end) count++
    }
    if (count > bestCount) {
      bestCount = count
      bestDate = ds
    }
    d.setDate(d.getDate() + 7)
  }

  if (!bestDate) {
    // Fallback to midpoint
    const mid = new Date(startMs + (endMs - startMs) / 2)
    while (mid.getDay() !== 3) mid.setDate(mid.getDate() + 1)
    return mid.toISOString().substring(0, 10).replace(/-/g, '')
  }

  return bestDate
}

// ── Step 1: Download GTFS feed ──

const REQUIRED_GTFS_FILES = ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt'] as const

/** A PTV-style feed ships ONE outer gtfs.zip that unzips into numbered subfeeds,
 *  each its own <N>/google_transit.zip (mode-split: rail / tram / bus). Return
 *  the subfeed zips present directly under dir (empty for the normal flat
 *  layout). */
export function findInnerGtfsZips(dir: string): { label: string; zip: string }[] {
  if (!existsSync(dir)) return []
  const out: { label: string; zip: string }[] = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue
    const zip = resolve(dir, entry.name, 'google_transit.zip')
    if (existsSync(zip)) out.push({ label: entry.name, zip })
  }
  return out.sort((a, b) => a.label.localeCompare(b.label))
}

/** Cheap peek: does this subfeed carry ≥1 route of a type this feed counts as
 *  rail? Reads only routes.txt (unzip -p, no full unpack) so a pure-bus subfeed
 *  is skipped before its large stop_times is ever extracted. Uses the SAME
 *  feed.railRouteTypes set as the parse filter — never a divergent copy. */
export function innerFeedHasRail(zipPath: string, feed: FeedConfig): boolean {
  let txt: string
  try {
    txt = execSync(`unzip -p "${zipPath}" routes.txt`, { timeout: 30_000, maxBuffer: 128 * 1024 * 1024 }).toString('utf-8')
  } catch {
    return false
  }
  const lines = txt.split('\n')
  if (lines.length < 2) return false
  const ti = parseCsvLine(lines[0].replace(/^\uFEFF/, '')).indexOf('route_type')
  if (ti < 0) return false
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].trim() === '') continue
    const rt = parseInt(parseCsvLine(lines[i])[ti] || '', 10)
    if (Number.isFinite(rt) && feed.railRouteTypes.has(rt)) return true
  }
  return false
}

/** Extract the rail-bearing subfeeds of a nested PTV-style feed into
 *  <parent>/<N>/extracted/ (reused if present) and return those dirs.
 *  computeStopFrequencies then runs once per subfeed and the caller concatenates
 *  — NO cross-feed GTFS merge (each subfeed keeps its own id space; stops at a
 *  shared location sum downstream by coordinate). */
function extractNestedRailFeeds(feed: FeedConfig, inner: { label: string; zip: string }[]): string[] {
  const dirs: string[] = []
  for (const { label, zip } of inner) {
    const dest = resolve(zip, '..', 'extracted')
    if (existsSync(resolve(dest, 'stops.txt'))) { dirs.push(dest); continue }
    if (!innerFeedHasRail(zip, feed)) {
      console.log(`  [${feed.id}] subfeed ${label}: no rail routes — skipping`)
      continue
    }
    mkdirSync(dest, { recursive: true })
    execSync(`unzip -o -q "${zip}" -d "${dest}"`, { timeout: 120_000 })
    for (const f of REQUIRED_GTFS_FILES) {
      if (!existsSync(resolve(dest, f))) throw new Error(`GTFS feed ${feed.id} subfeed ${label} missing required file: ${f}`)
    }
    console.log(`  [${feed.id}] subfeed ${label}: extracted rail GTFS → ${dest}`)
    dirs.push(dest)
  }
  return dirs
}

/** Resolve a feed to one or more flat GTFS extract dirs. A normal feed yields
 *  [dir]; a nested PTV-style feed yields one dir per rail-bearing subfeed. */
export async function downloadGtfs(feed: FeedConfig): Promise<string[]> {
  const feedDir = resolve(CACHE_DIR, feed.id)
  const zipPath = resolve(feedDir, 'gtfs.zip')
  const extractDir = resolve(feedDir, 'extracted')

  // Nested PTV-style layout (au-vic): the outer gtfs.zip has unzipped into
  // <N>/google_transit.zip subfeeds — process the rail-bearing ones. Checked
  // FIRST because such a feed has no flat stops.txt of its own. The staged
  // cache unzips in place (feedDir/<N>/), a fresh download into extractDir/<N>/
  // — look in both so a re-run after a fresh download still finds the subfeeds
  // (/gg Gemini).
  const inner = findInnerGtfsZips(feedDir)
  const nested = inner.length > 0 ? inner : findInnerGtfsZips(extractDir)
  if (nested.length > 0) return extractNestedRailFeeds(feed, nested)

  // #31.6: the staged cache uses a FLAT layout (stops.txt beside gtfs.zip,
  // extracted in place) — accept it, or --enrich-only silently no-ops on all
  // 23 staged feeds while hunting <id>/extracted/ (every throw below is
  // caught per-feed and a 0-feed run exits 0).
  if (!forceDownload && existsSync(resolve(feedDir, 'stops.txt'))) {
    console.log(`  [${feed.id}] Using cached GTFS (flat layout): ${feedDir}`)
    return [feedDir]
  }

  if (!forceDownload && existsSync(resolve(extractDir, 'stops.txt'))) {
    console.log(`  [${feed.id}] Using cached GTFS: ${extractDir}`)
    return [extractDir]
  }
  if (enrichOnly) {
    if (!existsSync(resolve(extractDir, 'stops.txt'))) {
      throw new Error(`--enrich-only but no cached GTFS for ${feed.id} at ${extractDir}`)
    }
    return [extractDir]
  }

  mkdirSync(feedDir, { recursive: true })

  console.log(`  [${feed.id}] Downloading GTFS from ${feed.url}...`)
  const res = await fetch(feed.url, {
    signal: AbortSignal.timeout(600_000), // 10 min — large feeds
    // Digitraffic (fi) returns HTTP 406 without Accept-Encoding: gzip; harmless elsewhere.
    headers: { 'Accept': 'application/zip, application/octet-stream, */*', 'Accept-Encoding': 'gzip' },
    redirect: 'follow',
  })
  if (!res.ok) throw new Error(`GTFS download failed for ${feed.id}: ${res.status} ${res.statusText}`)

  let buf = Buffer.from(await res.arrayBuffer())
  // GTFS zips start with 'PK'. If a server handed back a gzip stream (magic 1f 8b)
  // that undici didn't auto-inflate (Digitraffic fi), inflate to recover the .zip.
  if (buf[0] === 0x1f && buf[1] === 0x8b) buf = gunzipSync(buf)
  writeFileSync(zipPath, buf)
  console.log(`  [${feed.id}] Downloaded: ${(buf.length / 1e6).toFixed(1)} MB`)

  mkdirSync(extractDir, { recursive: true })
  execSync(`unzip -o -q "${zipPath}" -d "${extractDir}"`, { timeout: 120_000 })
  execSync(`rm -f "${zipPath}"`) // reclaim space before returning either way

  // Flat GTFS (the common case)?
  if (existsSync(resolve(extractDir, 'stops.txt'))) {
    for (const f of REQUIRED_GTFS_FILES) {
      if (!existsSync(resolve(extractDir, f))) throw new Error(`GTFS feed ${feed.id} missing required file: ${f}`)
    }
    return [extractDir]
  }
  // A nested outer zip unzips to <N>/google_transit.zip subfeeds instead.
  const freshInner = findInnerGtfsZips(extractDir)
  if (freshInner.length > 0) return extractNestedRailFeeds(feed, freshInner)
  throw new Error(`GTFS feed ${feed.id}: unzip produced neither flat stops.txt nor <N>/google_transit.zip subfeeds`)
}

// ── Step 2: Parse GTFS and compute stop frequencies ──

export async function computeStopFrequencies(
  extractDir: string,
  feed: FeedConfig,
): Promise<StopTrainCount[]> {
  // Versioned filename: the family-aware schema added a mandatory `family` field, so a
  // pre-migration cache (family-less stops) must NOT be reused — a stale entry would lack
  // a family and break the rail↔tram grid routing. A new name forces a clean rebuild.
  // Keyed by extractDir (NOT feed.id): a nested feed processes several subfeeds through
  // this function under one feed.id, so a feed.id key would make the subfeeds overwrite
  // each other's cache and double-count on the next run. A STAGED flat feed returns
  // [feedDir] (extractDir==feedDir) so its cache path is unchanged; a freshly downloaded
  // flat feed (extractDir==feedDir/extracted) just gets a harmless one-time rebuild.
  const cacheFile = resolve(extractDir, 'family-frequencies.json')
  if (!forceDownload && existsSync(cacheFile)) {
    console.log(`  [${feed.id}] Using cached stop frequencies: ${cacheFile}`)
    return JSON.parse(readFileSync(cacheFile, 'utf-8'))
  }

  console.log(`  [${feed.id}] Parsing GTFS files...`)
  const startTime = Date.now()

  // ── Parse routes.txt: route_id -> route_type ──
  console.log(`  [${feed.id}] Reading routes.txt...`)
  const routesRaw = await parseCsvStream(resolve(extractDir, 'routes.txt'))
  const routeTypeMap = new Map<string, number>()  // route_id -> route_type
  for (const r of routesRaw) {
    const routeType = parseInt(r['route_type'] || '3')
    routeTypeMap.set(r['route_id'], routeType)
  }
  console.log(`  [${feed.id}] ${routeTypeMap.size} routes, ${routesRaw.length} total`)

  // Map each route to its OSM rail family, respecting the per-feed route-type allow-list
  // (a rail-only feed yields only rail-family stops; an ALL_RAIL_AND_TRAM feed yields mixed).
  const routeFam = new Map<string, 'rail' | 'tram'>()
  for (const [routeId, routeType] of routeTypeMap) {
    const fam = railFamilyFor(routeType, feed)
    if (fam) routeFam.set(routeId, fam)
  }
  console.log(`  [${feed.id}] ${routeFam.size} rail/tram routes`)
  if (routeFam.size === 0) {
    console.log(`  [${feed.id}] WARNING: No rail routes found. Skipping.`)
    return []
  }

  // ── Parse calendar.txt: service_id -> days of week ──
  console.log(`  [${feed.id}] Reading calendar.txt...`)
  const calendarPath = resolve(extractDir, 'calendar.txt')
  const calendarDatesPath = resolve(extractDir, 'calendar_dates.txt')

  // Build set of active service IDs for a target Wednesday
  const activeServiceIds = new Set<string>()

  // Exact-dates convention detection mirrors lib/gtfs-enrich-core.ts (same 2026-07-16 SE
  // finding): calendar.txt whose every weekday flag is 0 carries no day information — the
  // calendar_dates-only branch below is the correct reader for it. Duplication note: this
  // per-stop counter is the pre-Phase-4 copy of the core's active-set logic; fold into
  // computeActiveTripFamiliesForFeed when this counter next changes.
  const calendarRawOrNull = existsSync(calendarPath) ? await parseCsvStream(calendarPath) : null
  const calendarWeekdayDriven = calendarRawOrNull !== null && calendarRawOrNull.some((r) =>
    ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday'].some((d) => r[d] === '1'))

  if (calendarRawOrNull !== null && calendarWeekdayDriven) {
    const calendarRaw = calendarRawOrNull
    const targetDate = findTargetWednesday(calendarRaw)
    const targetDateNum = targetDate  // YYYYMMDD string
    console.log(`  [${feed.id}] Target date: ${formatDate(targetDate)} (Wednesday)`)

    for (const r of calendarRaw) {
      const start = r['start_date'] || ''
      const end = r['end_date'] || ''
      const wednesday = r['wednesday'] || '0'
      if (wednesday === '1' && targetDateNum >= start && targetDateNum <= end) {
        activeServiceIds.add(r['service_id'])
      }
    }

    // Apply calendar_dates.txt exceptions
    if (existsSync(calendarDatesPath)) {
      const calDates = await parseCsvStream(calendarDatesPath)
      for (const r of calDates) {
        if (r['date'] !== targetDateNum) continue
        if (r['exception_type'] === '1') activeServiceIds.add(r['service_id'])
        if (r['exception_type'] === '2') activeServiceIds.delete(r['service_id'])
      }
    }
  } else if (existsSync(calendarDatesPath)) {
    // Feeds with only calendar_dates.txt — or an exact-dates calendar.txt (all weekday
    // flags 0, e.g. SE Trafiklab), which carries no usable day flags either way.
    console.log(`  [${feed.id}] ${calendarRawOrNull ? 'calendar.txt is exact-dates (all weekday flags 0)' : 'No calendar.txt'}, using calendar_dates.txt only`)
    const calDates = await parseCsvStream(calendarDatesPath)

    // Find a target date: pick the most common date in the file that is a Wednesday
    const dateCounts = new Map<string, number>()
    for (const r of calDates) {
      if (r['exception_type'] === '1') {
        const d = r['date'] || ''
        dateCounts.set(d, (dateCounts.get(d) || 0) + 1)
      }
    }
    // Find Wednesdays sorted by count
    const wednesdays = [...dateCounts.entries()]
      .filter(([d]) => {
        const ms = parseGtfsDate(d)
        return new Date(ms).getDay() === 3
      })
      .sort((a, b) => b[1] - a[1])

    if (wednesdays.length === 0) {
      // No Wednesdays, pick the date with most services
      const best = [...dateCounts.entries()].sort((a, b) => b[1] - a[1])
      if (best.length > 0) {
        console.log(`  [${feed.id}] No Wednesday found, using busiest date: ${formatDate(best[0][0])}`)
        for (const r of calDates) {
          if (r['date'] === best[0][0] && r['exception_type'] === '1') {
            activeServiceIds.add(r['service_id'])
          }
        }
      }
    } else {
      const targetDate = wednesdays[0][0]
      console.log(`  [${feed.id}] Target date (from calendar_dates): ${formatDate(targetDate)} (Wednesday, ${wednesdays[0][1]} services)`)
      for (const r of calDates) {
        if (r['date'] === targetDate && r['exception_type'] === '1') {
          activeServiceIds.add(r['service_id'])
        }
      }
    }
  } else {
    // No calendar at all — treat ALL trips as active
    console.log(`  [${feed.id}] WARNING: No calendar files found. Counting all trips.`)
  }

  console.log(`  [${feed.id}] ${activeServiceIds.size} active service IDs on target date`)

  // ── Parse trips.txt: trip_id -> (route_id, service_id) ──
  console.log(`  [${feed.id}] Reading trips.txt...`)
  const tripsRaw = await parseCsvStream(resolve(extractDir, 'trips.txt'))
  // Filter to rail/tram trips running on target day, carrying their family forward
  const tripFam = new Map<string, 'rail' | 'tram'>()
  for (const r of tripsRaw) {
    const fam = routeFam.get(r['route_id'])
    if (!fam) continue
    // If we have calendar info, filter by service; otherwise count all
    if (activeServiceIds.size > 0 && !activeServiceIds.has(r['service_id'])) continue
    tripFam.set(r['trip_id'], fam)
  }
  console.log(`  [${feed.id}] ${tripFam.size} rail trips on target day (of ${tripsRaw.length} total)`)

  if (tripFam.size === 0) {
    console.log(`  [${feed.id}] WARNING: No active rail trips. Skipping.`)
    return []
  }

  // ── Parse stop_times.txt: count departures per stop for rail trips ──
  console.log(`  [${feed.id}] Reading stop_times.txt (this may take a while for large feeds)...`)
  const stopDepartures = new Map<string, { rail: number; tram: number }>() // stop_id -> per-family departure count

  // Stream stop_times.txt because it can be very large (hundreds of MB)
  const stStream = createReadStream(resolve(extractDir, 'stop_times.txt'), { encoding: 'utf-8' })
  const stRl = createInterface({ input: stStream, crlfDelay: Infinity })
  let stHeaders: string[] | null = null
  let stLines = 0
  let stMatched = 0
  let tripIdIdx = -1
  let stopIdIdx = -1
  let lastProgressTime = Date.now()

  for await (const rawLine of stRl) {
    const line = stHeaders === null ? rawLine.replace(/^\uFEFF/, '') : rawLine
    if (line.trim() === '') continue

    if (!stHeaders) {
      stHeaders = parseCsvLine(line)
      tripIdIdx = stHeaders.indexOf('trip_id')
      stopIdIdx = stHeaders.indexOf('stop_id')
      if (tripIdIdx < 0 || stopIdIdx < 0) {
        throw new Error(`stop_times.txt missing required columns. Found: ${stHeaders.join(', ')}`)
      }
      continue
    }

    stLines++
    // Fast path: extract only trip_id and stop_id without full CSV parse
    const fields = parseCsvLine(line)
    const tripId = fields[tripIdIdx]
    const fam = tripFam.get(tripId)
    if (!fam) continue

    const stopId = fields[stopIdIdx]
    let counts = stopDepartures.get(stopId)
    if (!counts) { counts = { rail: 0, tram: 0 }; stopDepartures.set(stopId, counts) }
    counts[fam]++
    stMatched++

    if (Date.now() - lastProgressTime > 10_000) {
      console.log(`  [${feed.id}]   ... ${(stLines / 1e6).toFixed(1)}M lines, ${stMatched} rail stop-times`)
      lastProgressTime = Date.now()
    }
  }
  console.log(`  [${feed.id}] ${stLines} stop_times lines, ${stMatched} rail stop-times, ${stopDepartures.size} unique stops`)

  // ── Parse stops.txt: stop_id -> (lat, lon, name) ──
  console.log(`  [${feed.id}] Reading stops.txt...`)
  const stopsRaw = await parseCsvStream(resolve(extractDir, 'stops.txt'))
  const stopsMap = new Map<string, GtfsStop>()
  let skippedNoCoords = 0
  let skippedOutOfBounds = 0

  for (const r of stopsRaw) {
    const lat = parseFloat(r['stop_lat'] || '')
    const lon = parseFloat(r['stop_lon'] || '')
    if (!lat || !lon || isNaN(lat) || isNaN(lon)) { skippedNoCoords++; continue }

    // Bounding box sanity check — the SAME shared border margin as the pair
    // parser's stops envelope and the country graph bbox (item 6).
    const [minLat, minLon, maxLat, maxLon] = feed.boundingBox
    const m = GTFS_BORDER_MARGIN_DEG
    if (lat < minLat - m || lat > maxLat + m || lon < minLon - m || lon > maxLon + m) {
      skippedOutOfBounds++
      continue
    }

    let h3r4: string
    try { h3r4 = latLngToCell(lat, lon, 4) } catch { continue }

    stopsMap.set(r['stop_id'], {
      stop_id: r['stop_id'],
      lat, lon,
      name: (r['stop_name'] || '').trim(),
      h3r4,
    })
  }
  console.log(`  [${feed.id}] ${stopsMap.size} stops with valid coords`)
  if (skippedOutOfBounds > 0) console.log(`  [${feed.id}] Skipped (out of bounds): ${skippedOutOfBounds}`)

  // ── Resolve parent stations ──
  // GTFS stops can reference parent_station. If a stop has departures but isn't
  // in our stops map (it's a platform), check its parent.
  // Build child -> parent mapping from stops.txt
  const childToParent = new Map<string, string>()
  for (const r of stopsRaw) {
    const parentId = (r['parent_station'] || '').trim()
    if (parentId) {
      childToParent.set(r['stop_id'], parentId)
    }
  }

  // ── Build final stop frequency list ──
  const results: StopTrainCount[] = []
  let resolvedViaParent = 0

  for (const [stopId, counts] of stopDepartures) {
    let stop = stopsMap.get(stopId)
    if (!stop) {
      // Try parent station
      const parentId = childToParent.get(stopId)
      if (parentId) {
        stop = stopsMap.get(parentId)
        if (stop) resolvedViaParent++
      }
    }
    if (!stop) continue

    // All GTFS departures are passenger (freight isn't in GTFS). Emit one row per
    // non-zero family so rail and tram stops route to their own OSM rail_type grid.
    for (const family of ['rail', 'tram'] as const) {
      if (counts[family] === 0) continue
      results.push({
        stop_id: stop.stop_id,
        lat: stop.lat,
        lon: stop.lon,
        name: stop.name,
        h3r4: stop.h3r4,
        family,
        trains_passenger: counts[family],
        trains_freight: 0,
      })
    }
  }

  // Multiple stop_ids (platforms) can resolve to the same parent coords — sum them.
  const deduped = dedupeStopsByLocation(results)

  console.log(`  [${feed.id}] ${deduped.length} stops with train counts (${resolvedViaParent} resolved via parent station)`)

  // Stats
  const paxCounts = deduped.map(s => s.trains_passenger).sort((a, b) => b - a)
  if (paxCounts.length > 0) {
    console.log(`  [${feed.id}] Train frequency: max=${paxCounts[0]}, median=${paxCounts[Math.floor(paxCounts.length / 2)]}, min=${paxCounts[paxCounts.length - 1]}`)
    // Top 5 busiest stops
    const top5 = deduped.sort((a, b) => b.trains_passenger - a.trains_passenger).slice(0, 5)
    console.log(`  [${feed.id}] Busiest stops:`)
    for (const s of top5) {
      console.log(`    ${s.trains_passenger} trains/day: ${s.name} (${s.lat.toFixed(4)}, ${s.lon.toFixed(4)})`)
    }
  }

  const elapsed = ((Date.now() - startTime) / 1000).toFixed(1)
  console.log(`  [${feed.id}] GTFS parsing took ${elapsed}s`)

  // Cache results (cacheFile is under extractDir, which already exists)
  writeFileSync(cacheFile, JSON.stringify(deduped))
  console.log(`  [${feed.id}] Cached to ${cacheFile}`)

  return deduped
}

// ── Step 3: Match stops to railway segments and write Arrow ──

// Retract signature for stamps the pre-2026-07-10 fallback design wrote: the deleted
// class-default table, verbatim. A row still owned by MY_SOURCE_ID whose counts exactly
// equal its class tuple was filled by that fallback, not measured — exact-tuple + family
// ambiguity is negligible, and the retract's
// `when` additionally re-runs today's stop join, so a live-covered row is re-stamped by
// `match`, never disowned. No-match rows now return null: source_id stays 0 and the
// ENGINE default table (engine/noise-compute/src/emission/railway.rs::default_traffic)
// owns the "we don't know" case. DELETE this retract (and OLD_FALLBACK) after the world
// rail repaint confirms 0 retractions.
// rail_type: 0=rail 1=tram 2=light_rail 3=narrow_gauge 4=funicular; usage: 0=main 1=branch 2=industrial
const OLD_FALLBACK = (railType: number, usage: number): [pax: number, frt: number] => {
  if (railType === 2) return [250, 0]
  if (railType === 1) return [200, 0]
  if (railType === 3) return [30, 0]
  if (railType === 4) return [30, 0]
  if (usage === 1) return [80, 0]
  if (usage === 2) return [0, 8]
  return [50, 10]
}
const wasOldFallbackStamp = (row: RailRow): boolean => {
  const [pax, frt] = OLD_FALLBACK(row.railType, row.usage)
  return row.existingPax === pax && row.existingFrt === frt
}

interface CountryFeedOutcome {
  feed: FeedConfig
  tramStops: StopTrainCount[]
  pairs: RailStationPairCount[]
  /** Families this feed's own routes.txt declares under its allow-list
   *  classifier (union across a nested feed's subfeed dirs) — the evidence
   *  base for the BIDIRECTIONAL completeness check (review item 4). Empty
   *  when `failed` (never read then). */
  declared: Set<'rail' | 'tram'>
  failed: string | null
}

interface CountryProcessResult {
  cc: string
  /** Hex ids `iterateCountryHexes` returned for this country's bbox — the
   *  walk driver's own visitation set, recomputed here (cheap: a readdir +
   *  filter, no re-parse) so main() can union it across countries and tell
   *  the CRITICAL-2 sweep which hexes NOT to re-touch. Empty when nothing
   *  was walked (no GTFS data this country). */
  hexes: string[]
  retractSafe: boolean
  loaded: number
  denom: number
}

/** Country bbox for the per-country border-buffer graph: the union of this
 *  country's own feed `boundingBox` values — already
 *  hand-curated per feed for the stops.txt sanity check — padded 0.5° so the
 *  rail graph can bridge a station just across the border. Derived from data
 *  the registry already carries: no separate hand-authored per-country bbox
 *  table to drift out of sync with FEEDS, and no CGAZ overseas-territory
 *  blow-up (a country's own feed bbox is already scoped to where its network
 *  actually runs — unlike a union of ALL that country's CGAZ ADM0 polygons,
 *  which for FR/US/etc. would span oceans via dependent territories no GTFS
 *  feed here ever covers). */
export function countryBboxFor(cc: string): [number, number, number, number] {
  const feeds = FEEDS.filter(f => f.country === cc)
  if (feeds.length === 0) throw new Error(`countryBboxFor: no FEEDS entry carries country '${cc}'`)
  // SAME margin as the stops envelope (GTFS_BORDER_MARGIN_DEG — item 6): a
  // stop kept by loadStopsWithCoords must always find its rail graph loaded,
  // or every cross-border pair snap-fails into an endpoint quarantine.
  const MARGIN = GTFS_BORDER_MARGIN_DEG
  let minLat = Infinity, minLon = Infinity, maxLat = -Infinity, maxLon = -Infinity
  for (const f of feeds) {
    minLat = Math.min(minLat, f.boundingBox[0])
    minLon = Math.min(minLon, f.boundingBox[1])
    maxLat = Math.max(maxLat, f.boundingBox[2])
    maxLon = Math.max(maxLon, f.boundingBox[3])
  }
  return [
    Math.max(-90, minLat - MARGIN),
    Math.max(-180, minLon - MARGIN),
    Math.min(90, maxLat + MARGIN),
    Math.min(180, maxLon + MARGIN),
  ]
}

// buildTramExtraMatch now lives in lib/gtfs-enrich-core.ts (2026-07-16 Phase 4
// hoist) — imported above and re-exported for this file's own test importers.

/** Downloads one feed and computes BOTH the tram per-stop counts (unchanged
 *  `nearestGridStop` join input) and the heavy-rail station-PAIR counts (the
 *  new graph-walk input) from the SAME extract dir(s) — a nested PTV-style
 *  feed (au-vic) yields several dirs, concatenated the same way both arms
 *  already did before this rewrite. A tram-only feed (fr-idf) simply yields
 *  zero pairs through the same code path — its allow-list never maps a route
 *  to 'rail'. */
async function loadCountryFeed(feed: FeedConfig): Promise<CountryFeedOutcome> {
  try {
    const extractDirs = await downloadGtfs(feed)
    let tramStops: StopTrainCount[] = []
    let pairs: RailStationPairCount[] = []
    const declared = new Set<'rail' | 'tram'>()
    for (const dir of extractDirs) {
      // Declared families from THIS extract's own routes.txt, classified by the
      // SAME allow-list (+ metroAsRail) the two parse arms use — the evidence
      // base for bidirectional completeness (item 4). Throws on a malformed
      // routes.txt (item 3), which the catch below records as feed FAILED —
      // never as "legitimately declares nothing".
      for (const fam of await declaredRouteFamiliesForFeed(dir, (rt) => railFamilyFor(rt, feed))) declared.add(fam)

      const stopCounts = await computeStopFrequencies(dir, feed)
      tramStops = tramStops.concat(stopCounts.filter(s => s.family === 'tram'))

      const pairResult = await computeStopPairFrequenciesForFeed(dir, {
        bbox: feed.boundingBox,
        // Narrowed to 'rail' only: heavy rail is this function's whole remit
        // (tram stays on the per-stop join above) — reuses the SAME per-feed
        // allow-list + metroAsRail override as the per-stop path, just
        // dropping the 'tram' outcome.
        familyOf: (routeType) => (railFamilyFor(routeType, feed) === 'rail' ? 'rail' : null),
        // Same target-day policy as computeStopFrequencies's busiest-Wednesday
        // sampler, so pax counts describe the SAME day across both arms.
        dateSelection: findTargetWednesday,
        optionsKey: 'europe-busiest-wed',
      })
      pairs = pairs.concat(pairResult.pairs)
    }
    if (extractDirs.length > 1) tramStops = dedupeStopsByLocation(tramStops)
    return { feed, tramStops, pairs, declared, failed: null }
  } catch (err: any) {
    console.error(`  [${feed.id}] FAILED: ${err.message}`)
    return { feed, tramStops: [], pairs: [], declared: new Set(), failed: err.message }
  }
}

/** Does this feed's own allow-list (+ metroAsRail override) map ANY route
 *  type to heavy rail? Derived from the registry data the feed already
 *  carries — no separate "declares heavy rail" flag to drift out of sync.
 *  Drives the per-family completeness rule below AND documents which feeds
 *  can ever contribute heavy-rail retract evidence (a tram-only feed like
 *  fr-idf never can). */
export function feedDeclaresHeavyRail(feed: FeedConfig): boolean {
  return [...feed.railRouteTypes].some(rt => railFamilyFor(rt, feed) === 'rail')
}

/** Per-family completeness, BIDIRECTIONAL (2026-07-16 review item 4 — the
 *  earlier one-directional version accepted `tram=0, pairs=100` as complete,
 *  so a broken tram part could ride a working heavy-rail part into a retract
 *  that disowned tram stamps with nothing to re-claim them; the same masking
 *  the original item-3 fix closed in the OTHER direction). `declared` is the
 *  family set this feed's OWN routes.txt actually declares under its own
 *  allow-list classifier (`declaredRouteFamiliesForFeed` in loadCountryFeed)
 *  — registry-only declaration was wrong for e.g. an ALL_RAIL_AND_TRAM feed
 *  whose extract carries no trams (HR): a family the extract never declares
 *  is exempt, a family it declares must have parsed non-empty. Thin shim over
 *  the shared `describeIncompleteFamilies` (gtfs-enrich-core.ts) so europe
 *  and the 16 national enrichers can never drift on this rule. */
export function feedPartsIncomplete(declared: ReadonlySet<'rail' | 'tram'>, tramStopCount: number, pairCount: number): boolean {
  return describeIncompleteFamilies('feed', declared, pairCount, tramStopCount) !== ''
}

/** UNION bleed gate for the shared GLOBAL_GTFS_TRANSIT id (item 1, world
 *  mode only): the id is stamped by EVERY country in FEEDS, so a row is
 *  disownable-by-geometry only when it lies outside the union of ALL those
 *  territories — a single country's gate here would disown rows a
 *  neighbouring country's pass legitimately stamped (the CH-run-kills-
 *  southern-DE-rows bug). Gates are built lazily on the first call (ONE
 *  build per process) because makeCountryGate may download CGAZ on first
 *  use and module import must stay side-effect-free. */
function buildSharedIdUnionBleedGate(): (lat: number, lon: number) => boolean {
  let gates: Array<(lat: number, lon: number) => boolean> | null = null
  return (lat, lon) => {
    if (!gates) gates = [...new Set(FEEDS.map(f => f.country))].map(cc => makeCountryGate(cc))
    return gates.some(g => g(lat, lon))
  }
}

/** Runs the FULL per-country pipeline: download+parse this country's own
 *  feed(s), build heavy-rail station pairs (every feed summed through the
 *  walk's ONE canonical accumulator) and the tram stop grid, then ONE
 *  `enrichRailwaysByGraphWalk` call scoped to a border-buffered country
 *  bbox. This is the SAME function `--country CC` and the all-countries loop
 *  both call — a `country:XX` chain scope and a full run touch identical
 *  code, never two paths that can drift. Returns `null` when FEEDS carries no feed for `cc` at all
 *  (nothing to do — not an error, since not every country has a europe feed).
 *
 *  `bleedGate` (item 1): geometry-evidence gate for the driver's bleed
 *  retract arm. Omitted in per-country mode — a single country's run must
 *  never disown the SHARED id's rows another country legitimately stamped;
 *  foreign residue of the shared id is healed by the world-mode legacy sweep
 *  + heal-rail-country-bleed.ts. World mode passes the FEEDS-union gate. */
async function processCountry(cc: string, bleedGate?: (lat: number, lon: number) => boolean): Promise<CountryProcessResult | null> {
  const feeds = FEEDS.filter(f => f.country === cc && (!requestedFeeds || requestedFeeds.includes(f.id)))
  if (feeds.length === 0) return null

  console.log(`\n=== Country: ${cc} (${feeds.length} feed(s): ${feeds.map(f => f.id).join(', ')}) ===`)

  const outcomes: CountryFeedOutcome[] = []
  for (const feed of feeds) {
    console.log(`\n--- Feed: ${feed.name} (${feed.id}) ---`)
    outcomes.push(await loadCountryFeed(feed))
  }

  const failedFeedIds = outcomes.filter(o => o.failed).map(o => o.feed.id)
  // Per-family completeness, BIDIRECTIONAL (item 4): `feedPartsIncomplete` —
  // every family the feed's own routes.txt declares must have parsed
  // non-empty (declares rail → pairs>0, declares tram → tramStops>0), so
  // retractSafe below can never disown one family's stamps on the OTHER
  // family's working snapshot; a family the extract never declares is exempt.
  const emptyFeedIds = outcomes
    .filter(o => !o.failed && feedPartsIncomplete(o.declared, o.tramStops.length, o.pairs.length))
    .map(o => o.feed.id)
  const loaded = outcomes.length - failedFeedIds.length - emptyFeedIds.length
  const denom = feeds.length

  // #31.6 completeness marker, scoped to THIS country's own feed count (plan
  // Phase 4 point 2: "per-country mode emits its own marker with that
  // country's feed count") — never claims the global 23.
  {
    const state = loaded >= denom ? 'complete' : loaded === 0 ? 'missing' : 'partial'
    const detail = `${loaded}/${denom} feeds loaded${failedFeedIds.length ? `; failed: ${failedFeedIds.join(',')}` : ''}${emptyFeedIds.length ? `; empty: ${emptyFeedIds.join(',')}` : ''}`
    console.log(`QM_COMPLETENESS ${JSON.stringify({ actual: loaded, state, detail })}`)
  }

  // CRITICAL-1b (/gg Codex, unchanged rationale): a retract may only run over a
  // PROVABLY COMPLETE input snapshot for THIS country — every one of its own
  // feeds loaded non-empty, no --feed subset narrowing it.
  const retractSafe = requestedFeeds === null && failedFeedIds.length === 0 && emptyFeedIds.length === 0
  if (!retractSafe) {
    logRetractSkippedIncompleteInputs([
      requestedFeeds !== null ? `--feed subset run: ${requestedFeeds.join(',')}` : '',
      failedFeedIds.length > 0 ? `failed feeds: ${failedFeedIds.join(',')}` : '',
      emptyFeedIds.length > 0 ? `feeds parsed empty: ${emptyFeedIds.join(',')}` : '',
    ].filter(Boolean).join('; '))
  }

  // Plain concatenation — `accumulateCanonicalPairs` downstream (inside the
  // walk) sums by canonical OD key regardless of how many source arrays
  // contributed; overlap policy lives in the registry (fr-idf tram-only),
  // never in a merge step here.
  const pairs: RailStationPairCount[] = outcomes.flatMap(o => o.pairs)

  const tramStops = dedupeStopsByLocation(outcomes.flatMap(o => o.tramStops))
  console.log(`  [${cc}] ${pairs.length} heavy-rail station pairs, ${tramStops.length} tram/light-rail stops`)

  if (pairs.length === 0 && tramStops.length === 0) {
    console.log(`  [${cc}] No GTFS data to enrich — skipping the walk.`)
    return { cc, hexes: [], retractSafe, loaded, denom }
  }

  const bbox = countryBboxFor(cc)
  const countryGate = makeCountryGate(cc)
  const stats = await enrichRailwaysByGraphWalk({
    h3r4Dir: H3R4_DIR,
    bbox,
    pairs,
    sourceId: MY_SOURCE_ID,
    countryGate,
    // Item 1: SHARED-id bleed evidence — absent in per-country mode (this
    // run's gate covers only ONE of the territories the id legitimately
    // stamps), the FEEDS-union gate in world mode. See processCountry's doc.
    bleedGate,
    extraMatch: buildTramExtraMatch(tramStops, MY_SOURCE_ID),
    // Single id — europe has no retired predecessor id to absorb (unlike CZ's
    // migration #26, this file's own source id is unchanged across this rewrite).
    retract: { sourceIds: [MY_SOURCE_ID] },
    retractSafe,
    enableDestructive: !stampOnly,
    // #26B: europe is NOT silent-capable (silentResidual omitted) — a
    // quarantine-free row the walk never reaches simply stays at
    // source_id=0 for the engine's own class default, same as before this
    // rewrite; only CZ's timetable evidence (owner decision option b)
    // justifies a residual.
    sidecar: {
      scope: `europe-${cc.toLowerCase()}`,
      extractFingerprint: `europe-gtfs:${feeds.map(f => f.id).join('+')}`,
      feeds: feeds.map(f => f.id),
    },
  })
  console.log(`  [${cc}] walk stats: ${JSON.stringify(stats)}`)

  return { cc, hexes: iterateCountryHexes(H3R4_DIR, bbox, 'railways.arrow'), retractSafe, loaded, denom }
}

/** CRITICAL-2 (/gg Codex, unchanged since before Phase 4): stale
 *  pre-2026-07-10 OLD_FALLBACK stamps hide precisely in hexes with ZERO
 *  stops today (their stop coverage vanished since the legacy stamp) — no
 *  per-country walk visits those, since a country's own bbox only reaches
 *  hexes ITS OWN feeds cover. Retract-only pass (`match` is `() => null`)
 *  over every railways.arrow hex NOT already visited this run; runs ONLY in
 *  all-countries mode, never under `--country`, and only when every country this run processed was
 *  itself retract-safe (see main()). */
async function runLegacyFallbackSweep(alreadyVisitedHexIds: ReadonlySet<string>): Promise<void> {
  let sweepScanned = 0, sweepUpdated = 0, sweepRetracted = 0
  const sweepStart = Date.now()
  const allHexDirs = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  console.log(`\n  Retract-only sweep: ${allHexDirs.length} hex dirs enumerated, ${alreadyVisitedHexIds.size} already visited this run`)
  for (const hexId of allHexDirs) {
    if (alreadyVisitedHexIds.has(hexId)) continue
    const railPath = resolve(H3R4_DIR, hexId, 'railways.arrow')
    if (!existsSync(railPath)) continue
    sweepScanned++
    const r = await writeRailTrains(railPath, () => null, undefined, { sourceId: MY_SOURCE_ID, when: wasOldFallbackStamp })
    sweepRetracted += r.retracted
    if (r.updated) sweepUpdated++
    if (sweepScanned % 2000 === 0) {
      console.log(`  [sweep ${((Date.now() - sweepStart) / 1000).toFixed(0)}s] ${sweepScanned} stopless hexes, ${sweepUpdated} updated, ${sweepRetracted.toLocaleString()} retracted`)
    }
  }
  console.log(`  Retract-only sweep done in ${((Date.now() - sweepStart) / 1000).toFixed(0)}s — ${sweepRetracted.toLocaleString()} retracted, ${sweepUpdated}/${sweepScanned} hexes updated`)
}

// ── Main ──

async function main() {
  console.log(`=== Europe/Global Railway GTFS Enrichment (Phase 4: graph-walk) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Cache: ${CACHE_DIR}`)
  console.log(`  Year: ${YEAR}\n`)

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  if (countryArg) {
    // NO bleedGate (item 1): a per-country run's own gate covers only one of
    // the territories the shared id legitimately stamps — bleed stays off and
    // foreign residue is left to the world-mode sweep + heal-rail-country-bleed.ts.
    const result = await processCountry(countryArg)
    if (!result) {
      const known = [...new Set(FEEDS.map(f => f.country))].sort()
      console.error(`ERROR: no FEEDS entry carries country '${countryArg}' (available: ${known.join(' ')})`)
      process.exit(1)
    }
    console.log(`\n=== Done (country:${countryArg}, ${stampOnly ? 'stamp-only — Step A proof mode' : 'destructive ops enabled'}) ===`)
    return
  }

  // ALL-COUNTRIES MODE: every country FEEDS carries, SEQUENTIALLY, through the
  // exact same processCountry() a --country invocation uses. The world-wide
  // legacy sweep runs only here.
  const countries = [...new Set(FEEDS.map(f => f.country))]
  console.log(`  Processing ${countries.length} countries sequentially: ${countries.join(' ')}\n`)

  const allVisited = new Set<string>()
  let totalLoaded = 0
  let totalDenom = 0
  let anyCountryIncomplete = requestedFeeds !== null

  // World mode runs with the shared id's UNION bleed gate (item 1): rows
  // outside EVERY feed country's territory are geometry-evidence stale and
  // disownable; rows inside another feed country's territory are that
  // country's own run's business, never this one's.
  const sharedIdUnionBleedGate = buildSharedIdUnionBleedGate()

  for (const cc of countries) {
    const result = await processCountry(cc, sharedIdUnionBleedGate)
    if (!result) continue
    for (const h of result.hexes) allVisited.add(h)
    totalLoaded += result.loaded
    totalDenom += result.denom
    if (!result.retractSafe) anyCountryIncomplete = true
  }

  // Aggregate marker — printed LAST among every QM_COMPLETENESS line this
  // process emits (each processCountry call above already printed its own
  // per-country marker), so chain/status.ts's parseCompletenessMarker (which
  // keeps only the LAST match in the log) reports the world total against
  // the manifest's FEEDS-derived expectMinInputs floor (item 7: no more
  // hand-written 23) — the per-country lines are informational only.
  {
    const state = totalLoaded >= totalDenom ? 'complete' : totalLoaded === 0 ? 'missing' : 'partial'
    const detail = `${totalLoaded}/${totalDenom} feeds loaded across ${countries.length} countries`
    console.log(`\nQM_COMPLETENESS ${JSON.stringify({ actual: totalLoaded, state, detail })}`)
  }

  if (stampOnly) {
    console.log(`\n  Retract-only sweep SKIPPED — --stamp-only (Step A proof mode).`)
  } else if (anyCountryIncomplete) {
    console.log(`\n  Retract-only sweep SKIPPED — at least one country ran retract-unsafe this pass (see its own log lines above).`)
  } else {
    await runLegacyFallbackSweep(allVisited)
  }

  console.log(`\n=== Done (all countries, ${stampOnly ? 'stamp-only — Step A proof mode' : 'destructive ops enabled'}) ===`)
}

// Import-safe: run only when invoked directly — importing this file must never
// trigger a download/enrichment pass (pattern from enrich-roads-cz.ts).
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(err => { console.error('Error:', err); process.exit(1) })
}
