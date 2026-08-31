/**
 * High-resolution country point-in-polygon gate from geoBoundaries CGAZ ADM0
 * (`geoBoundariesCGAZ_ADM0_s0005.geojson`, derived from the release GeoPackage
 * pinned at tag v6.0.0 — see below).
 *
 * Use to stop an enricher writing one country's data onto another country's roads
 * where a rectangular `*_HEX_BBOX` overlaps a neighbour: Poland's bbox blankets
 * most of Czechia, so a Czech "I/150" was matched to Polish "DW150" and given
 * Polish AADT (see the Stage-3 road audit). Gate the match by the road midpoint's
 * actual country.
 *
 * Why CGAZ and not Natural Earth 1:10m (the previous source): NE generalization
 * mis-assigns multi-km border salients — the Hlučínsko salient (Czech soil,
 * 49.98953 N 18.12880 E, ~4.5 km inside CZ) tested Poland-true, so the PL gate
 * legitimately wrote Polish AADT onto Czech roads there. geoBoundaries CGAZ is
 * OSM-derived, globally gap-filled, CC-BY 4.0 (atlas attribution: boundaries ©
 * geoBoundaries — Runfola et al. 2020, doi:10.1371/journal.pone.0231866); all
 * CZ salients verified correct (Hlučínsko, Šluknov, Frýdlant, Bogatynia).
 *
 * Derivation (needs GDAL's ogr2ogr, `apt install gdal-bin`): the 162 MB
 * GeoPackage is one-time-converted to a 95 MB GeoJSON with `-simplify 0.0005`
 * (~55 m Douglas-Peucker — full CGAZ rings are 8-15x denser, parse 3.6 s and
 * cost ~200 µs/gate-call for fidelity the gate doesn't need; the 55 m band
 * flips 0.06 % of points vs full, measured on a 111 m grid over Hlučínsko) and
 * `COORDINATE_PRECISION=6` (~0.1 m). The GeoPackage is kept beside the GeoJSON
 * so the tolerance can be retuned without re-acquiring it.
 *
 * ACQUIRING the dataset is NOT this module's job and never happens on the read
 * path: `prepare-country-polygon-caches.ts` is the ONE step that downloads it.
 * WHY the split — a unit test that merely writes a road row must not be able to
 * reach the network. `writeRoadAadt` derives its national gate from the stamped
 * `source_id`, so every writer test used to trigger a 162 MB download; when
 * geoBoundaries deleted the pinned v6.0.0 release (404 since 2026-08-17) that
 * turned the data-free CI gate red. A missing dataset now fails LOUD here
 * instead — nothing in this module substitutes different geometry, because a
 * silently ungated national enricher writes one country's counts onto another
 * country's roads, the exact bug the gate exists to prevent.
 *
 * This is an actual-polygon gate — NOT `h3r4-admin.bin`, which is H3 res-4
 * (~22 km, centroid-based) and far too coarse to separate roads at a border.
 */
import { readFileSync, mkdirSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { replaceCacheFileAtomically } from './atomic-cache.js'
import { DOWNLOAD_CACHE_DIR, describeDownloadSearchPath, findDownloadedFile } from './download-cache.js'
import { pointInRing, pointToSegmentDist } from './spatial.js'

/** Release-pinned file names — a version bump renames both, so a stale cache
 *  can never masquerade as the current dataset (boundaries silently moving
 *  under a data gate would make enrichment runs irreproducible). */
export const CGAZ_GPKG_FILE = 'geoBoundariesCGAZ_ADM0.gpkg'
export const CGAZ_GEOJSON_FILE = 'geoBoundariesCGAZ_ADM0_s0005.geojson'

/** Absolute path of the prepared CGAZ GeoJSON, or null when no cache visible to
 *  this checkout holds it. The read-only probe callers use to ask "is the
 *  boundary dataset available here?" — the gate's own tests skip on null, and
 *  the prepare step uses it to decide whether to acquire. */
export function preparedCountryPolygonFile(): string | null {
  return findDownloadedFile(CGAZ_GEOJSON_FILE)
}

/** Convert an already-downloaded GeoPackage into the working cache's GeoJSON.
 *  Local only — ogr2ogr, no network. Returns null when no GeoPackage is on
 *  disk either. Atomic replace so an interrupted or concurrent cold start
 *  cannot leave a truncated cache or steal another process's temporary file. */
function deriveGeojsonFromDownloadedGeopackage(): string | null {
  const gpkg = findDownloadedFile(CGAZ_GPKG_FILE)
  if (!gpkg) return null
  const geojson = resolve(DOWNLOAD_CACHE_DIR, CGAZ_GEOJSON_FILE)
  mkdirSync(DOWNLOAD_CACHE_DIR, { recursive: true })
  replaceCacheFileAtomically(geojson, temporaryPath => {
    try {
      execFileSync('ogr2ogr', ['-f', 'GeoJSON', temporaryPath, gpkg,
        '-select', 'shapeGroup', '-simplify', '0.0005', '-lco', 'COORDINATE_PRECISION=6'])
    } catch (e) {
      throw new Error(`country-polygon: ogr2ogr conversion failed — is GDAL installed (apt install gdal-bin)? ${e}`)
    }
  })
  return geojson
}

type CgazFeature = { properties: Record<string, unknown>; geometry: { type: string; coordinates: unknown } }
let features: ReadonlyArray<CgazFeature> | null = null
function cgazFeatures(): ReadonlyArray<CgazFeature> {
  if (features) return features
  const path = preparedCountryPolygonFile() ?? deriveGeojsonFromDownloadedGeopackage()
  if (!path) {
    throw new Error(
      `country-polygon: the CGAZ ADM0 boundary dataset is not prepared — neither ${CGAZ_GEOJSON_FILE} nor ` +
      `${CGAZ_GPKG_FILE} found in ${describeDownloadSearchPath()}. Run ` +
      `\`npx tsx pipeline/prepare-country-polygon-caches.ts\`, the one step that acquires it.`)
  }
  // Log the resolved file: the gate's answers are only as trustworthy as the
  // geometry behind them, and the search path spans several caches.
  console.log(`  country-polygon: CGAZ ADM0 from ${path}`)
  return (features = JSON.parse(readFileSync(path, 'utf8')).features)
}

// CGAZ keys countries by ISO 3166-1 alpha-3 (`shapeGroup`); callers use alpha-2.
// The ONE table lives in scripts/iso-codes.mjs (shared with scripts/*.mjs);
// re-exported here for the pipeline's existing consumers (chain/scope.ts,
// admin-at.ts, validation/lib.ts).
import { ISO2_TO_ISO3 } from '../../scripts/iso-codes.mjs'
export { ISO2_TO_ISO3 }

type Ring = ReadonlyArray<readonly [number, number]>

/**
 * A load-once point-in-country tester: `true` iff `(lat, lon)` is inside an outer
 * ring of the ISO-3166-1 alpha-2 country AND not inside one of that ring's holes
 * (foreign enclaves are excluded). Throws on an alpha-2 code that is unknown or has
 * no CGAZ feature — fail loud, since a silent always-false gate would drop every match.
 * `makeCountryGate(iso2)` IS the reuse path for other enrichers; no need to
 * expose the raw rings.
 */
export function makeCountryGate(iso2: string): (lat: number, lon: number) => boolean {
  const idx = buildIndexed(isoToFeature(iso2).geometry)
  return (lat, lon) => inIndexed(idx, lat, lon)
}

/**
 * Ownership areas CGAZ cannot express — used by BOTH sides of the ownership
 * machinery, or they contradict each other (#32 /gg round-3 Codex):
 *
 * `NON_CGAZ_OWNERSHIP_BBOXES`: territories with NO CGAZ ADM0 feature at all,
 * where a pass legitimately owns by bbox (isolated — no foreign land inside
 * the box). Folded into `makeAnyCountryGate`, else R14/heal-industrial-orphans
 * treats their legitimate stamps as orphans (Usine Koniambo was among the
 * false positives); the territory's own pass imports the SAME entry for its
 * countryGate override.
 *
 * `COUNTRY_TERRITORY_EXTENSIONS`: source countries whose national network
 * legitimately spans an extra CGAZ territory. `makeOwnershipGate` unions them;
 * the R9 auditor, heal-rail-country-bleed AND the country's enrichers must all
 * use it — deriving bare `makeCountryGate(iso)` on one side turns the other
 * side's stamps into "foreign" and the chain ping-pongs (40 real EH rows of
 * ma-national-railway were one generic-heal run away from deletion).
 */
export const NON_CGAZ_OWNERSHIP_BBOXES: ReadonlyArray<{ iso2: string; bbox: readonly [number, number, number, number] }> = [
  // New Caledonia — no CGAZ feature OF ITS OWN (AdminAt resolves it FR via
  // FRA's overseas rings — measured on the 2026-07 admin-bin regen); NC keeps
  // a separate ownership identity for the national-pass machinery, where the
  // bbox IS a safe ownership area (nearest foreign land, Vanuatu, ~500 km).
  { iso2: 'NC', bbox: [-23.0, 163.5, -19.5, 168.5] },
]

export const COUNTRY_TERRITORY_EXTENSIONS: Readonly<Record<string, readonly string[]>> = {
  MA: ['EH'], // ONCF network + GEM scope span Western Sahara (CGAZ ESH)
}

/** Union of a country's polygon and its declared territory extensions — THE
 *  ownership gate every national-source consumer must use (writer countryGate,
 *  R9 auditor, heal-rail-country-bleed). Identical to makeCountryGate for the
 *  ~200 countries without extensions. */
export function makeOwnershipGate(iso2: string): (lat: number, lon: number) => boolean {
  const gates = [makeCountryGate(iso2), ...(COUNTRY_TERRITORY_EXTENSIONS[iso2.toUpperCase()] ?? []).map(makeCountryGate)]
  if (gates.length === 1) return gates[0]
  return (lat, lon) => gates.some(g => g(lat, lon))
}

/**
 * Lazy WORLD membership tester: true when (lat, lon) lies inside ANY CGAZ ADM0
 * country OR inside a declared non-CGAZ ownership bbox (see above). Powers the industrial shared-id orphan rule (#32 / auditor R13): a
 * stamp in no country at all is unreachable by every national pass since #31
 * (both destructive arms are countryGate-scoped), so it can only be stale
 * legacy. One flat ring index over the whole CGAZ set — the per-ring bbox
 * pre-check keeps far-out misses (open sea) nearly free. Built on first call,
 * cached for the process.
 */
export function makeAnyCountryGate(): (lat: number, lon: number) => boolean {
  let all: IndexedRing[] | null = null
  return (lat, lon) => {
    for (const t of NON_CGAZ_OWNERSHIP_BBOXES) {
      const [S, W, N, E] = t.bbox
      if (lat >= S && lat <= N && lon >= W && lon <= E) return true
    }
    if (!all) all = cgazFeatures().flatMap(f => buildIndexed(f.geometry))
    return inIndexed(all, lat, lon)
  }
}

/**
 * THE national-ownership predicate for segment geometry — a segment is foreign
 * to a country only when start, mid AND end all lie outside its polygon. This
 * is the auditor's R9 rule verbatim; the writeRailTrains countryGate, the
 * enrichers' retract `when` country arms and heal-rail-country-bleed MUST all
 * use it, or a genuine border-straddler oscillates between "heal retracts it"
 * and "the national enricher may not re-claim it" (#31.7 follow-up: 7 real
 * DE/CZ straddling rows were being deleted by the stricter midpoint-only
 * check). Midpoint first — the cheap majority case for in-country rows.
 */
export function segmentWhollyOutside(
  gate: (lat: number, lon: number) => boolean,
  midLat: number, midLon: number,
  startLat: number, startLon: number,
  endLat: number, endLon: number,
): boolean {
  return !gate(midLat, midLon) && !gate(startLat, startLon) && !gate(endLat, endLon)
}

// `bands` is a latitude-band index of the outer ring's edges (null for small rings,
// brute-forced): a ray-cast crossing can only come from an edge straddling the query
// latitude, so band[⌊(lat-s)/bandH⌋] lists exactly the candidate edges — turning the
// O(ring) point-in-polygon into O(edges-in-band) for huge rings (China/Russia).
// Exported (with cgazLandIndex/inIndexedRings below) for lib/admin-at.ts's global
// point→country index, which reuses the same banded rings instead of re-parsing CGAZ.
export type IndexedRing = { outer: Ring; holes: Ring[]; w: number; s: number; e: number; n: number; bands: number[][] | null; bandH: number }

/** Outer ring + interior holes + lon/lat bbox per polygon of a CGAZ geometry.
 *  Holes are enclaves of OTHER countries (e.g. a Tajik exclave inside Uzbekistan):
 *  a point in a hole is NOT in this country (gg 2026-06-14). The per-ring bbox is a
 *  cheap pre-check — CGAZ rings are dense and archipelagos carry hundreds of island
 *  rings (FRA incl. overseas: 204); the box test skips them ~free. */
function buildIndexed(g: { type: string; coordinates: unknown }): IndexedRing[] {
  const polys: { outer: Ring; holes: Ring[] }[] = []
  if (g.type === 'Polygon') {
    const r = g.coordinates as Ring[]
    polys.push({ outer: r[0], holes: r.slice(1) })
  } else if (g.type === 'MultiPolygon') {
    for (const poly of g.coordinates as Ring[][]) polys.push({ outer: poly[0], holes: poly.slice(1) })
  }
  return polys.map(({ outer, holes }) => {
    let w = Infinity, s = Infinity, e = -Infinity, n = -Infinity
    for (const [x, y] of outer) { if (x < w) w = x; if (x > e) e = x; if (y < s) s = y; if (y > n) n = y }
    const len = outer.length
    // Index only rings big enough to matter (~1 band per 8 edges, capped at 256).
    const bandCount = len > 64 ? Math.min(256, Math.ceil(len / 8)) : 0
    let bands: number[][] | null = null
    const bandH = bandCount > 0 ? (n - s) / bandCount || 1 : 0
    if (bandCount > 0) {
      bands = Array.from({ length: bandCount }, () => [] as number[])
      for (let i = 0; i < len; i++) {
        const j = i === 0 ? len - 1 : i - 1
        const yi = outer[i][1], yj = outer[j][1]
        let b0 = Math.floor((Math.min(yi, yj) - s) / bandH)
        let b1 = Math.floor((Math.max(yi, yj) - s) / bandH)
        if (b0 < 0) b0 = 0
        if (b1 >= bandCount) b1 = bandCount - 1
        for (let b = b0; b <= b1; b++) bands[b].push(i)
      }
    }
    return { outer, holes, w, s, e, n, bands, bandH }
  })
}

/** Ray-cast point-in-outer-ring, using the band index when present. Identical
 *  result to `pointInRing(lon, lat, r.outer)` — only the candidate set shrinks. */
function inOuter(r: IndexedRing, lat: number, lon: number): boolean {
  const { outer, bands } = r
  if (!bands) return pointInRing(lon, lat, outer)
  const len = outer.length
  let b = Math.floor((lat - r.s) / r.bandH)
  if (b < 0) b = 0; else if (b >= bands.length) b = bands.length - 1
  let inside = false
  for (const i of bands[b]) {
    const j = i === 0 ? len - 1 : i - 1
    const xi = outer[i][0], yi = outer[i][1], xj = outer[j][0], yj = outer[j][1]
    if (((yi > lat) !== (yj > lat)) && (lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi)) inside = !inside
  }
  return inside
}

/** True iff the point is inside an outer ring and not inside one of its holes. */
function inIndexed(idx: readonly IndexedRing[], lat: number, lon: number): boolean {
  for (const r of idx) {
    if (lon < r.w || lon > r.e || lat < r.s || lat > r.n) continue
    if (!inOuter(r, lat, lon)) continue
    if (r.holes.some(h => pointInRing(lon, lat, h))) continue // inside a foreign enclave
    return true
  }
  return false
}

function isoToFeature(iso2: string): { iso3: string; geometry: { type: string; coordinates: unknown } } {
  const code = iso2.toUpperCase()
  const iso3 = ISO2_TO_ISO3[code]
  if (!iso3) throw new Error(`country-polygon: unknown ISO_A2 country code ${code}`)
  const f = cgazFeatures().find(x => x.properties.shapeGroup === iso3)
  if (!f) throw new Error(`country-polygon: no CGAZ ADM0 feature for ${code} (${iso3})`)
  return { iso3, geometry: f.geometry }
}

/** True iff CGAZ has an ADM0 polygon for this alpha-2 — the CLEAN pre-check
 *  callers use instead of catching makeCountryGate's throw, so a genuinely
 *  country-less territory (NC/HK/PR/…) reads as `false` (expected: use a bbox
 *  override) while an I/O failure loading CGAZ (download / GDAL / corrupt file)
 *  still propagates LOUD from `cgazFeatures()` rather than being swallowed as
 *  "ungate this source" (/gg #33 Codex CRITICAL — fail-open masked I/O damage). */
export function hasCountryPolygon(iso2: string): boolean {
  const iso3 = ISO2_TO_ISO3[iso2.toUpperCase()]
  if (!iso3) return false
  return cgazFeatures().some(x => x.properties.shapeGroup === iso3)
}

// ── Per-polygon country bboxes (committed table) ─────────────────────────────

const BBOX_TABLE_FILE = 'country-polygon-bboxes-cgaz-v6-s0005.generated.json'

export type CountryBbox = readonly [number, number, number, number] // [s, w, n, e]

let bboxTable: Record<string, CountryBbox[]> | null = null

/**
 * ISO2 → bbox per CGAZ polygon PART (outer rings only). Per-part, never a
 * union: FR/GB/NO territories would span half the globe as one box. Consumers:
 * chain/scope.ts (country → hex enumeration) and lib/hex-country.ts
 * (microstate candidate discovery).
 *
 * Read from the COMMITTED table, never derived at call time. WHY committed: a
 * few hundred rectangles are the whole answer, and deriving them meant parsing
 * the 95 MB dataset — so every chain dry-run and every scope resolution
 * inherited the boundary dataset's availability. The file name pins the CGAZ
 * release and simplification tolerance it was generated from, so a version bump
 * renames it rather than silently changing scopes underneath a run; regenerate
 * with `npm run gen:country-polygon-bboxes`.
 */
export function allCountryPolygonBboxes(): Record<string, CountryBbox[]> {
  if (bboxTable) return bboxTable
  const table = JSON.parse(readFileSync(resolve(import.meta.dirname, BBOX_TABLE_FILE), 'utf8')) as
    { bboxes: Record<string, CountryBbox[]> }
  return (bboxTable = table.bboxes)
}

/**
 * Re-derive the committed bbox table from the prepared CGAZ GeoJSON — the
 * generator behind `country-polygon-bboxes-*.generated.json`. Only
 * `lib/gen-country-polygon-bboxes.ts` calls it; every consumer reads the
 * committed table so scope resolution never needs the boundary dataset.
 */
export function deriveCountryPolygonBboxes(): Record<string, CountryBbox[]> {
  const iso3to2 = new Map(Object.entries(ISO2_TO_ISO3).map(([a2, a3]) => [a3, a2]))
  const out: Record<string, CountryBbox[]> = {}
  for (const f of cgazFeatures()) {
    const iso2 = iso3to2.get(String(f.properties.shapeGroup ?? ''))
    if (!iso2) continue // numeric disputed-area codes — no ISO identity
    const g = f.geometry
    const rings: Ring[] =
      g.type === 'Polygon'
        ? [(g.coordinates as Ring[])[0]]
        : g.type === 'MultiPolygon'
          ? (g.coordinates as Ring[][]).map(poly => poly[0])
          : []
    out[iso2] = rings.map(ring => {
      let s = Infinity, w = Infinity, n = -Infinity, e = -Infinity
      for (const [lon, lat] of ring) {
        if (lat < s) s = lat
        if (lat > n) n = lat
        if (lon < w) w = lon
        if (lon > e) e = lon
      }
      return [s, w, n, e] as const
    })
  }
  return out
}

// Global land-mask over ALL CGAZ features (incl. disputed / non-ISO), memoised +
// shared. The coastal gate uses it to keep land borders strict and reject
// strait-ambiguous sea points; only the rare OUTSIDE rows hit it.
let landIdx: ReadonlyArray<{ iso3: string; idx: IndexedRing[] }> | null = null
function landMask(): ReadonlyArray<{ iso3: string; idx: IndexedRing[] }> {
  if (!landIdx) landIdx = cgazFeatures().map(f => ({ iso3: String(f.properties.shapeGroup), idx: buildIndexed(f.geometry) }))
  return landIdx
}
function inAnyCountry(lat: number, lon: number): boolean {
  for (const { idx } of landMask()) if (inIndexed(idx, lat, lon)) return true
  return false
}
// Global grid of every country's outer-ring SEGMENTS, tagged by iso3, bucketed
// into 0.1° (~11 km) cells, memoised + shared. The coastal gate's slow path asks
// only a THRESHOLD ("is this country's coast within bufferM?"), never a true
// minimum — so a query touches just the few segments in the cells within bufferM
// instead of scanning a whole country's outer ring. distToOuter used to scan
// China's entire border (~tens of k vertices) per offshore road, which went
// pathological at coastal multi-border hexes (RU/CN/PRK tripoint: 10+ min/hex).
const SEG_GRID_DEG = 0.1
type GridSeg = { iso3: string; aLat: number; aLon: number; bLat: number; bLon: number }
let segGrid: Map<number, GridSeg[]> | null = null
// iy∈[0,1800), ix∈[0,3600] (lon=180 → ix=3600). The 4096 stride exceeds the ix
// range so keys stay collision-free even for the small ±rx query overshoot.
const segCell = (ix: number, iy: number): number => iy * 4096 + ix

function buildSegGrid(): Map<number, GridSeg[]> {
  const grid = new Map<number, GridSeg[]>()
  for (const { iso3, idx } of landMask()) {
    for (const { outer } of idx) {
      for (let i = 0; i < outer.length - 1; i++) {
        const [aLon, aLat] = outer[i], [bLon, bLat] = outer[i + 1]
        // Skip antimeridian-spanning artifacts (only ATA at ±180 in CGAZ v6.0.0):
        // one such edge would smear across every longitude cell, and pointToSegmentDist
        // doesn't wrap at ±180 so its distance would be bogus anyway. No road-bearing
        // country's coast crosses the dateline seam, so dropping the edge is harmless.
        if (Math.abs(aLon - bLon) > 180) continue
        const seg: GridSeg = { iso3, aLat, aLon, bLat, bLon }
        const x0 = Math.floor((Math.min(aLon, bLon) + 180) / SEG_GRID_DEG)
        const x1 = Math.floor((Math.max(aLon, bLon) + 180) / SEG_GRID_DEG)
        const y0 = Math.floor((Math.min(aLat, bLat) + 90) / SEG_GRID_DEG)
        const y1 = Math.floor((Math.max(aLat, bLat) + 90) / SEG_GRID_DEG)
        for (let ix = x0; ix <= x1; ix++)
          for (let iy = y0; iy <= y1; iy++) {
            const k = segCell(ix, iy), b = grid.get(k)
            if (b) b.push(seg); else grid.set(k, [seg])
          }
      }
    }
  }
  return grid
}

/** True iff any outer-ring segment of a country matching `want(iso3)` lies within
 *  `distM` of the point. Replaces the per-country `distToOuter <= buf` scan. */
function coastWithin(lat: number, lon: number, distM: number, want: (iso3: string) => boolean): boolean {
  const grid = segGrid ?? (segGrid = buildSegGrid())
  // Cell radius covering distM. lon degrees shrink toward the poles, so widen the
  // lon search by 1/cosLat (floored at 0.05 ≈ 87°N — past any road).
  const cosLat = Math.max(Math.cos(lat * Math.PI / 180), 0.05)
  const ry = Math.ceil(distM / (SEG_GRID_DEG * 110_540)) + 1
  const rx = Math.ceil(distM / (SEG_GRID_DEG * 111_320 * cosLat)) + 1
  const cx = Math.floor((lon + 180) / SEG_GRID_DEG)
  const cy = Math.floor((lat + 90) / SEG_GRID_DEG)
  for (let ix = cx - rx; ix <= cx + rx; ix++) {
    for (let iy = cy - ry; iy <= cy + ry; iy++) {
      const bucket = grid.get(segCell(ix, iy))
      if (!bucket) continue
      for (const s of bucket) {
        if (!want(s.iso3)) continue
        if (pointToSegmentDist(lat, lon, s.aLat, s.aLon, s.bLat, s.bLon) <= distM) return true
      }
    }
  }
  return false
}

function otherCountryWithin(lat: number, lon: number, bufferM: number, exceptIso3: string): boolean {
  return coastWithin(lat, lon, bufferM, iso3 => iso3 !== exceptIso3)
}

// ── AdminAt raw material (lib/admin-at.ts) ──────────────────────────────────

/** Land rings of EVERY CGAZ feature (ISO-mapped AND disputed numeric codes),
 *  memoised per process — the raw material for AdminAt's global point→country
 *  index. Exposed so admin-at.ts never re-parses the 95 MB GeoJSON nor
 *  re-derives the band indexes this module already builds. */
export function cgazLandIndex(): ReadonlyArray<{ iso3: string; idx: IndexedRing[] }> {
  return landMask()
}

/** Banded PIP over a cgazLandIndex ring set — the exported twin of the private
 *  inIndexed, for AdminAt's per-part exact test. */
export function inIndexedRings(idx: readonly IndexedRing[], lat: number, lon: number): boolean {
  return inIndexed(idx, lat, lon)
}

/** Distinct shapeGroups (ISO alpha-3 AND disputed numeric codes) whose coast
 *  lies within `distM` of the point — the candidate set for AdminAt's
 *  uniquely-attributable coastal fallback: exactly one shapeGroup within the
 *  buffer means the sea point belongs to that country; zero means open sea,
 *  two or more means strait ambiguity (never "first candidate wins"). Same
 *  segment grid + radius math as coastWithin, collecting instead of
 *  short-circuiting. */
export function coastShapeGroupsWithin(lat: number, lon: number, distM: number): string[] {
  const grid = segGrid ?? (segGrid = buildSegGrid())
  const cosLat = Math.max(Math.cos(lat * Math.PI / 180), 0.05)
  const ry = Math.ceil(distM / (SEG_GRID_DEG * 110_540)) + 1
  const rx = Math.ceil(distM / (SEG_GRID_DEG * 111_320 * cosLat)) + 1
  const cx = Math.floor((lon + 180) / SEG_GRID_DEG)
  const cy = Math.floor((lat + 90) / SEG_GRID_DEG)
  const found = new Set<string>()
  for (let ix = cx - rx; ix <= cx + rx; ix++) {
    for (let iy = cy - ry; iy <= cy + ry; iy++) {
      const bucket = grid.get(segCell(ix, iy))
      if (!bucket) continue
      for (const s of bucket) {
        if (found.has(s.iso3)) continue
        if (pointToSegmentDist(lat, lon, s.aLat, s.aLon, s.bLat, s.bLon) <= distM) found.add(s.iso3)
      }
    }
  }
  return [...found]
}

/**
 * Like `makeCountryGate`, but tolerant of COASTAL roads whose midpoint falls just
 * offshore of the CGAZ polygon (the polygon coastline sits inland of OSM waterfront
 * roads on reclaimed land — ~14 % of class-0 roads in the Lagos hex were lost this
 * way, gg 2026-06-15). Accepts a point inside the country, OR over the sea within
 * `bufferM` of it AND not inside / within `bufferM` of any OTHER country — so land
 * borders stay strict (a neighbour-side point is `inAnyCountry`) and narrow straits
 * don't double-claim. Road enrichers use THIS; `makeCountryGate` stays land-only for
 * negative-gate callers (e.g. enrich-industrial-kr `!inNK && !inJP`).
 */
export function makeCoastalCountryGate(iso2: string, bufferM = 2000): (lat: number, lon: number) => boolean {
  // A sea buffer is a few km; reject misuse loudly — an unbounded bufferM would blow
  // up coastWithin's cell-radius loop (gg 2026-06-15, Codex).
  if (!(bufferM > 0 && bufferM <= 50_000)) throw new Error(`makeCoastalCountryGate: bufferM ${bufferM} out of (0, 50000] m`)
  const { iso3, geometry } = isoToFeature(iso2)
  const target = buildIndexed(geometry)
  // Same conjunction as a strict land gate would reject on, ordered cheapest-first:
  // the two grid-backed coast-threshold checks reject the bulk of foreign/far-sea
  // points, so the expensive whole-polygon `inAnyCountry` PIP runs only for the few
  // survivors (genuine sea points hugging the target's own coast).
  return (lat, lon) => {
    if (inIndexed(target, lat, lon)) return true                       // target land — fast path (bulk)
    if (!coastWithin(lat, lon, bufferM, x => x === iso3)) return false // too far from target coast
    if (otherCountryWithin(lat, lon, bufferM, iso3)) return false      // neighbour/strait coast within buffer
    if (inAnyCountry(lat, lon)) return false                           // deep inside a neighbour — rare survivor
    return true                                                         // over sea, uniquely nearest target
  }
}
