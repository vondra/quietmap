/**
 * City traffic enrichment driver — stamps municipal counter AADT onto road
 * arrows for every enabled city in `lib/city-datasets.ts`.
 *
 * NAMED OUTSIDE the `enrich-roads-*.ts` glob on purpose: `pipeline/bench/
 * rerun-measured.sh` launches that glob CONCURRENTLY (xargs -P) and a city
 * driver racing national enrichers over the same hex arrows would fight the
 * per-hex lock — this driver runs as the runner's sequential Phase 3
 * instead.
 *
 * Per city: adapter loads normalized per-street records (cached download) →
 * rows inside the municipal ADM2 polygon (`makeCityGate` — real boundary,
 * never a bbox) are matched by normalized street name, osm_id where the
 * source publishes it, or proximity for records carrying geometry (point
 * stations / counted-section lines; a pre-pass pins each one to the street
 * it measures, see `GeoRec.targetNames`) → `writeRoadAadt` stamps the
 * city's source_id (priority 90 → `city-measured`, outranks national
 * INSIDE the polygon only).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-cities-roads.ts [--city <slug>]
 *       [--enrich-only] [--force-download]
 */
import { resolve } from 'node:path'
import { CITY_DATASETS } from './lib/city-datasets.js'
import type { CityRecord } from './lib/city-adapters/types.js'
import { makeCityGate } from './lib/city-polygon.js'
import { iterateCountryHexes, writeRoadAadt, type RoadAadt, type RoadRow } from './lib/roads-arrow.js'
import { shouldOverwrite } from './lib/sources.js'
import { pointToPolylineDist } from './lib/spatial.js'
import { H3R4_DIR } from './lib/data-year.js'

const argv = process.argv.slice(2)
const ENRICH_ONLY = argv.includes('--enrich-only')
const FORCE_DOWNLOAD = argv.includes('--force-download')
const cityArg = argv.includes('--city') ? argv[argv.indexOf('--city') + 1] : null

/** Street-name normalization for the join: case/diacritics/whitespace-fold.
 *  TSK publishes "LEGEROVA", OSM has "Legerova"; abbreviations ("nám.",
 *  "tř.") stay as-is on both sides — extend per city if a real miss shows. */
function normStreet(s: string): string {
  return s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/\s+/g, ' ').trim()
}

/** One geometry record prepared for proximity matching: its `[minLat,
 *  minLon, maxLat, maxLon]` bbox pre-expanded by the match cap, so the
 *  per-row test is 4 compares before any exact distance math. */
interface GeoRec {
  rec: CityRecord
  bbox: readonly [number, number, number, number]
  /** Normalized street names this record may stamp, fixed by the pre-pass.
   *  Point stations get exactly ONE name (`pickTargetName`): a junction-
   *  adjacent station is within cap of side-street segments too — without
   *  this, Wien's Westbahnhof counter put the Gürtel's 66k DTV on the
   *  residential Mittelgasse (38 of 58 stations bled). Section lines keep
   *  every name that genuinely runs ALONG them (closest row ≤
   *  `LINE_NEAR_M`) — multiple names are legit (a counted section spans
   *  street renames), but a parallel residential street hovering at
   *  28–40 m for its whole stretch is bleed (178 such rows on Brno).
   *  Undefined = no row in reach; the record stamps nothing. */
  targetNames?: ReadonlySet<string>
}

const M_PER_DEG_LAT = 110_540
const M_PER_DEG_LON_EQ = 111_320

function makeGeoRec(rec: CityRecord, capM: number): GeoRec {
  const line = rec.line!
  let minLat = Infinity, minLon = Infinity, maxLat = -Infinity, maxLon = -Infinity
  for (const [lon, lat] of line) {
    if (lat < minLat) minLat = lat
    if (lat > maxLat) maxLat = lat
    if (lon < minLon) minLon = lon
    if (lon > maxLon) maxLon = lon
  }
  const latPad = capM / M_PER_DEG_LAT
  const lonPad = capM / (M_PER_DEG_LON_EQ * Math.cos((minLat * Math.PI) / 180))
  return { rec, bbox: [minLat - latPad, minLon - lonPad, maxLat + latPad, maxLon + lonPad] }
}

/**
 * Distance (m) between a geometry record and a road row.
 *
 * Point station (1 vertex): distance from the station to the row's
 * segment — the station stamps whatever carriageway it stands next to.
 *
 * Counted section line (2+ vertices): the WORST of the row's start/mid/end
 * distances to the line. Requiring all three near means the row runs
 * ALONG the counted section; a perpendicular side street that merely
 * touches it at the junction has far endpoints and is rejected.
 */
function geoRecDist(rec: CityRecord, row: RoadRow): number {
  const line = rec.line!
  if (line.length === 1) {
    return pointToPolylineDist(line[0][1], line[0][0], [
      [row.startLon, row.startLat],
      [row.endLon, row.endLat],
    ])
  }
  return Math.max(
    pointToPolylineDist(row.startLat, row.startLon, line),
    pointToPolylineDist(row.midLat, row.midLon, line),
    pointToPolylineDist(row.endLat, row.endLon, line),
  )
}

/** A name group must come at least this close to a section line somewhere
 *  along it to be stampable from that section. Tuned on Brno: legit groups
 *  (incl. the far carriageway of divided arterials — pentlogram lines sit
 *  on the street axis) all approach within 24 m, parallel-street bleed
 *  groups never get closer than 28 m. */
const LINE_NEAR_M = 28

/** Nearest geometry record within `capM` of the row whose pre-pass street
 *  set allows the row's name, or undefined. */
function nearestGeoRec(geoRecs: readonly GeoRec[], row: RoadRow, capM: number): CityRecord | undefined {
  const rowMinLat = Math.min(row.startLat, row.endLat)
  const rowMaxLat = Math.max(row.startLat, row.endLat)
  const rowMinLon = Math.min(row.startLon, row.endLon)
  const rowMaxLon = Math.max(row.startLon, row.endLon)
  const rowName = normStreet(row.name ?? '')
  let best: CityRecord | undefined
  let bestD = capM
  for (const { rec, bbox, targetNames } of geoRecs) {
    if (rowMinLat > bbox[2] || rowMaxLat < bbox[0] || rowMinLon > bbox[3] || rowMaxLon < bbox[1]) continue
    if (!targetNames?.has(rowName)) continue
    const d = geoRecDist(rec, row)
    if (d <= bestD) {
      bestD = d
      best = rec
    }
  }
  return best
}

/** Effective class for "which street does this station measure" ranking:
 *  links rank just after their parent (10/11/12 = motorway/trunk/primary
 *  link — engine constants.rs) so a trunk ramp still beats a residential
 *  lane, but never the named arterial itself. */
function effectiveClass(cls: number): number {
  return cls === 10 ? 0.5 : cls === 11 ? 1.5 : cls === 12 ? 2.5 : cls
}

/** Groups farther than the nearest group + this much don't compete for a
 *  point station: class may arbitrate Gürtel-vs-Mittelgasse at 27 vs 28 m,
 *  but the A22 passing 31 m from the Reichsbrücke station (own street at
 *  5.8 m) is a different road, not a candidate. */
const POINT_BAND_M = 15

/**
 * The street a point station measures, from the rows within its cap:
 * among the comparably-near name groups (`POINT_BAND_M`), the biggest road
 * class wins (a permanent MIV counter stands on the arterial, not the side
 * lane — fixes Wien's Taubstummengasse, where the station is NAMED after
 * the side lane but counts Favoritenstraße); class ties break on the
 * station label (fixes Hauptstraße-vs-Mauerbachstraße at the junction the
 * station is named after), then on distance. Trailing roman numerals are
 * duplicate-station suffixes ("Wagramer Straße II"), not part of the
 * street name.
 */
function pickTargetName(cands: ReadonlyArray<{ name: string; cls: number; dist: number }>, label: string): string | undefined {
  if (cands.length === 0) return undefined
  const hint = normStreet(label).replace(/ (i{1,3}|iv|v)$/, '')
  const maxDist = Math.min(...cands.map((c) => c.dist)) + POINT_BAND_M
  const groups = new Map<string, { cls: number; dist: number }>()
  for (const c of cands) {
    if (c.dist > maxDist) continue
    const g = groups.get(c.name)
    const cls = effectiveClass(c.cls)
    if (!g) groups.set(c.name, { cls, dist: c.dist })
    else {
      g.cls = Math.min(g.cls, cls)
      g.dist = Math.min(g.dist, c.dist)
    }
  }
  const ranked = [...groups.entries()].sort(
    (a, b) => a[1].cls - b[1].cls || Number(b[0] === hint) - Number(a[0] === hint) || a[1].dist - b[1].dist,
  )
  return ranked[0][0]
}

async function enrichCity(slug: string): Promise<void> {
  const city = CITY_DATASETS.find((c) => c.slug === slug)
  if (!city) throw new Error(`unknown city slug "${slug}" (have: ${CITY_DATASETS.map((c) => c.slug).join(', ')})`)
  if (!city.enabled) {
    console.log(`[cities] ${slug}: disabled, skipping`)
    return
  }
  const records = await city.load({ enrichOnly: ENRICH_ONLY, forceDownload: FORCE_DOWNLOAD })
  const capM = city.geoMatchCapM ?? 40
  const byName = new Map<string, CityRecord>()
  const byOsmId = new Map<number, CityRecord>()
  const geoRecs: GeoRec[] = []
  for (const r of records) {
    if (r.line && r.line.length > 0) {
      geoRecs.push(makeGeoRec(r, capM)) // geometry-only — see CityRecord.line
    } else {
      byName.set(normStreet(r.street), r)
      if (r.osmId != null) byOsmId.set(r.osmId, r)
    }
  }
  const inCity = makeCityGate(city.adm2.iso3, city.adm2.shapeName)
  const hexes = iterateCountryHexes(H3R4_DIR, city.hexBbox)
  console.log(`[cities] ${slug}: ${records.length} street records, ${hexes.length} hexes in envelope`)

  // Pre-pass for geometry records: collect every in-polygon row within cap,
  // then fix what each record may stamp — ONE street per point station
  // (`pickTargetName`), the along-the-line street set per section line.
  // Collected WITHOUT the coverage filter: a motorway section must SEE its
  // own class-0 rows as the closest street and stamp nothing — with the
  // filter they are invisible and the nearest visible group is an adjacent
  // residential lane (Brno D1 put 72k on "Martina Ševčíka"). Read-only —
  // the collector callback never returns a match, so `writeRoadAadt`
  // leaves every file untouched.
  if (geoRecs.length > 0) {
    const cands = new Map<GeoRec, { name: string; cls: number; dist: number }[]>()
    for (const hex of hexes) {
      await writeRoadAadt(resolve(H3R4_DIR, hex, 'roads.arrow'), (row) => {
        if (!inCity(row.midLat, row.midLon)) return null
        const rowMinLat = Math.min(row.startLat, row.endLat)
        const rowMaxLat = Math.max(row.startLat, row.endLat)
        const rowMinLon = Math.min(row.startLon, row.endLon)
        const rowMaxLon = Math.max(row.startLon, row.endLon)
        // living_street/service/track (6–8) can hug a station or section at
        // arm's length, but a traffic counter never measures them — letting
        // them into candidacy poisons the distance band and the primary-
        // street pick. Real roads only (motorways stay visible on purpose:
        // they must be able to WIN and void the record, see above).
        if (row.roadClass >= 6 && row.roadClass <= 8) return null
        for (const g of geoRecs) {
          const [s, w, n, e] = g.bbox
          if (rowMinLat > n || rowMaxLat < s || rowMinLon > e || rowMaxLon < w) continue
          const d = geoRecDist(g.rec, row)
          if (d > capM) continue
          const list = cands.get(g) ?? []
          cands.set(g, list)
          list.push({ name: normStreet(row.name ?? ''), cls: row.roadClass, dist: d })
        }
        return null
      })
    }
    let unmatchable = 0
    for (const g of geoRecs) {
      const cs = cands.get(g) ?? []
      if (g.rec.line!.length === 1) {
        // A station whose measured street is out of coverage targets that
        // street anyway — the writer's class gate then stamps nothing,
        // which is exactly right for an excluded-class street.
        const target = pickTargetName(cs, g.rec.street)
        g.targetNames = target === undefined ? undefined : new Set([target])
      } else {
        const groups = new Map<string, { dist: number; cls: number }>()
        for (const c of cs) {
          const e = groups.get(c.name)
          if (!e || c.dist < e.dist) groups.set(c.name, { dist: c.dist, cls: c.cls })
        }
        const primary = [...groups.values()].sort((a, b) => a.dist - b.dist)[0]
        const along =
          primary === undefined || !city.coverage.has(primary.cls)
            ? [] // section drawn on an excluded street (D1/D2) — stamp nothing
            : [...groups.entries()].filter(([, g2]) => g2.dist <= LINE_NEAR_M && city.coverage.has(g2.cls)).map(([n]) => n)
        g.targetNames = along.length > 0 ? new Set(along) : undefined
      }
      if (g.targetNames === undefined) unmatchable++
    }
    console.log(`[cities] ${slug}: ${geoRecs.length} geometry records targeted (${unmatchable} stamp nothing)`)
  }

  let totalMatched = 0
  let zeroSplitSkipped = 0
  for (const hex of hexes) {
    const res = await writeRoadAadt(resolve(H3R4_DIR, hex, 'roads.arrow'), (row): RoadAadt | null => {
      // Rank fast-exit first (cheap), THEN the polygon gate — the
      // city-measured rank is only honest inside the municipal boundary;
      // bbox hexes reach far beyond it, so every stamped row passes PiP.
      if (!shouldOverwrite(row.existingSourceId, city.sourceId)) return null
      if (!inCity(row.midLat, row.midLon)) return null
      const rec =
        (row.osmId != null ? byOsmId.get(row.osmId) : undefined) ??
        (row.name ? byName.get(normStreet(row.name)) : undefined) ??
        (geoRecs.length > 0 ? nearestGeoRec(geoRecs, row, capM) : undefined)
      if (!rec) return null
      // Generic guard for EVERY adapter (Praha's length-weighted mean, Wien's
      // kfz-minus-lkw clamp, Brno's thousands split — and any future city):
      // each can legitimately round every class to zero for a low-traffic
      // street/station. Catching it HERE, after city.load() and downstream of
      // any adapter cache, means no per-adapter cache-bypass duplication is
      // needed (#31.4 — never stamp zeros under a measured id).
      if (rec.aadtLight + rec.aadtMedium + rec.aadtHeavy + rec.aadtMoto === 0) {
        zeroSplitSkipped++
        return null
      }
      return {
        light: rec.aadtLight,
        medium: rec.aadtMedium,
        heavy: rec.aadtHeavy,
        moto: rec.aadtMoto,
        sourceId: city.sourceId,
      }
    }, undefined, city.coverage)
    totalMatched += res.matched
  }
  const zeroNote = zeroSplitSkipped > 0 ? ` (${zeroSplitSkipped} rows skipped: zero AADT split under a measured id)` : ''
  console.log(`[cities] ${slug}: stamped ${totalMatched} rows across ${hexes.length} hexes${zeroNote}`)
  if (totalMatched === 0) {
    throw new Error(`[cities] ${slug}: 0 rows matched — name normalization or gate regression?`)
  }
}

const targets = cityArg ? [cityArg] : CITY_DATASETS.filter((c) => c.enabled).map((c) => c.slug)
for (const slug of targets) {
  await enrichCity(slug)
}
