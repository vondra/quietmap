/**
 * Vector building-footprint probe for the road `built_up` flag (task #15).
 *
 * Replaces the 166 GB building RASTER sampler with a read of the Overture
 * obstacle store the engine already screens against
 * (`data/enrichment/global/overture-obstacles/h3r4/<cell>/obstacles-<TILE>.arrow`,
 * written by `scripts/obstacles/ingest-overture-obstacles.py`). Nothing here is
 * acoustics: `built_up` only decides whether an UNTAGGED road of class 2/3/4/9
 * gets the country's legal urban or rural speed (CZ 50 vs 90), and the raster
 * was never more than an urban-density proxy that happened to be lying around.
 *
 * The ONE consumer that writes the flag is `enrich-roads-built-up.ts`; the
 * engine reads the resulting `built_up` column in
 * `noise-compute::defaults::resolve_speed_default`.
 *
 * THREE-STATE CONTRACT, unchanged — 0 is "covering data absent" and the engine
 * falls back to the legacy world speed table on it, so it must never be guessed
 * for a covered-but-empty place.
 *
 * WHY the window is a DEGREE box and not a metric one: the raster window was
 * 17 pixels of a 1/3600° grid in BOTH axes — 525 m × 337 m at 50° N, but
 * 525 m × 525 m at the equator. Reproducing it in degrees keeps the swap a
 * like-for-like change of DATA SOURCE; a metric window would silently
 * re-classify every road away from the equator at the same time.
 *
 * WHY "estimated built pixels" and not a raw footprint count: the retired
 * Overture rasterizer burned a pixel when its CENTRE fell inside a footprint
 * (gdal_rasterize without ALL_TOUCHED), so
 * the built-pixel count it thresholded is an unbiased estimate of
 * `built area / pixel area` — and pixel area shrinks with cos(lat). Dividing
 * the window's footprint area by the local pixel area therefore reproduces the
 * raster's own statistic, latitude effect included, and the calibrated
 * threshold [`BUILT_UP_MIN_BUILT_PIXELS`] keeps the meaning it was fitted with.
 *
 * Coverage is decided by the WORLD INGEST MANIFEST (`.ingested-tiles`), not by
 * the presence of shards: the ingest writes a shard only for cells that
 * received ≥1 footprint, so "no shard" is ambiguous between covered-and-empty
 * and never-staged (see `noise-compute::propagation::obstacle_ingest_coverage`
 * for the same rule on the engine side). The manifest lists 1° tiles and the
 * raster was one file per 1° tile, so the two probes agree on WHERE they know
 * anything: the two tile sets are identical (13 694 tiles, verified 2026-08-30).
 */

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { latLngToCell } from 'h3-js'

export const BUILT_UP_UNKNOWN = 0 // covering 1° tile never ingested — engine falls back to the legacy speed table
export const BUILT_UP_RURAL = 1
export const BUILT_UP_URBAN = 2

/** Half-width of the sampling window, in degrees of both lat and lon.
 *
 *  8.5 and not 8: the raster's window was 17 PIXELS across (centre ±8), and a
 *  pixel is an area, not a point — the ground it covered spans 17/3600°, half
 *  a pixel past the outermost node on each side. Sampling ±8/3600° instead
 *  measures a window 13 % smaller in area and reads ~11 % low, which showed up
 *  as the calibration optimum drifting from 8 built pixels to 7 (campaign
 *  2026-08-built-up-vector). */
export const BUILT_UP_WINDOW_HALF_DEG = 8.5 / 3600

/** Kept from the raster calibration (2026-07-03, cells 841942dffffffff GB +
 *  841e309ffffffff CZ against tagged maxspeed) — the unit is still the
 *  raster's, so the number keeps its provenance. RE-DERIVED against the vector
 *  store on 2026-08-30 (campaign 2026-08-built-up-vector): a grid search over
 *  2…24 on 27 951 road segments in CZ/DE/FR/GB/US/BR reproduces the raster's
 *  own answer best at exactly 8 (97.30 %; 7 → 97.23 %, 9 → 96.83 %). Two
 *  rejected statistics, same sample: footprint COUNT peaks at 92.39 % (th=30)
 *  and raw footprint AREA at 97.10 % (th=5000 m²) — area beats count because a
 *  village of sheds and a retail park differ hundredfold in mass at the same
 *  count, and dividing by the pixel carries the cos(lat) the raster had. */
export const BUILT_UP_MIN_BUILT_PIXELS = 8

/** Metres per degree of latitude on the raster grid (WGS84 mean). */
const M_PER_DEG_LAT = 111_132.0
const M_PER_DEG_LON_EQ = 111_320.0
/** Pixels per degree of the retired 30 m building raster (3601² node grid). */
const RASTER_PIXELS_PER_DEG = 3600

/** The staged Overture obstacle store and the world-ingest manifest beside it.
 *  Exported so the enricher can fail loud when the store is absent — a silent
 *  world of `built_up = 0` would drop the urban/rural signal everywhere. */
export const OBSTACLE_STORE_DIR = resolve(
  import.meta.dirname,
  '../../data/enrichment/global/overture-obstacles/h3r4',
)
export const OBSTACLE_INGEST_MANIFEST = resolve(
  import.meta.dirname,
  '../../data/enrichment/global/overture-obstacles/.ingested-tiles',
)

/** Area of one raster pixel at this latitude (m²) — the divisor that turns
 *  built area into the raster's built-pixel count. */
function rasterPixelAreaM2(lat: number): number {
  const dLat = M_PER_DEG_LAT / RASTER_PIXELS_PER_DEG
  const dLon = (M_PER_DEG_LON_EQ * Math.cos((lat * Math.PI) / 180)) / RASTER_PIXELS_PER_DEG
  return dLat * dLon
}

/**
 * One H3 R4 cell's footprints, reduced to what this probe needs: centroid
 * position and ground area. Bucketed on a grid whose cell is exactly the
 * sampling window, so a query scans at most 2×2 buckets.
 *
 * This is a query helper over data already loaded, not a second obstacle
 * index: it holds no geometry, is never written to disk, and dies with the
 * process. Without it a dense hex costs segments × footprints comparisons.
 */
interface CellFootprints {
  lat: Float64Array
  lon: Float64Array
  areaM2: Float64Array
  /** bucket key → [start, end) into `order` */
  bucket: Map<number, [number, number]>
  order: Uint32Array
}

const BUCKET_DEG = 2 * BUILT_UP_WINDOW_HALF_DEG
/** Keys pack (latIdx, lonIdx) of a ±90/±180° grid into one exact f64. */
const BUCKET_KEY_STRIDE = 1 << 18

function bucketKey(latIdx: number, lonIdx: number): number {
  return (latIdx + 65536) * BUCKET_KEY_STRIDE + (lonIdx + 131072)
}

/** Ground area of one WKB footprint (2D XY, Polygon or MultiPolygon) in m²,
 *  by the shoelace formula in a local equirectangular frame — the same
 *  flat-earth model the engine's kernels use. Rings are metre-scale, so the
 *  projection error inside one footprint is far below the ~600 m² pixel this
 *  feeds.
 *
 *  Holes are SUBTRACTED, unlike `noise_compute::wkb::outer_ring_area_m2`,
 *  which answers a different question (emission scales with a source's
 *  footprint, whereas gdal_rasterize did not burn a courtyard and neither does
 *  this). Type codes match the engine's parser: anything but 3 or 6 is not a
 *  polygon, which the ingest contract says cannot occur. */
function wkbFootprintAreaM2(wkb: Uint8Array): number {
  const view = new DataView(wkb.buffer, wkb.byteOffset, wkb.byteLength)
  let p = 0
  const readHeader = (): { little: boolean; type: number } => {
    const little = view.getUint8(p) === 1
    p += 1
    const type = view.getUint32(p, little)
    p += 4
    return { little, type }
  }
  const readRingArea = (little: boolean): number => {
    const n = view.getUint32(p, little)
    p += 4
    if (n < 3) {
      p += n * 16
      return 0
    }
    const lon0 = view.getFloat64(p, little)
    const lat0 = view.getFloat64(p + 8, little)
    const mLat = M_PER_DEG_LAT
    const mLon = M_PER_DEG_LON_EQ * Math.cos((lat0 * Math.PI) / 180)
    let px = 0
    let py = 0
    let twiceArea = 0
    for (let i = 0; i < n; i++) {
      const lon = view.getFloat64(p, little)
      const lat = view.getFloat64(p + 8, little)
      p += 16
      const x = (lon - lon0) * mLon
      const y = (lat - lat0) * mLat
      if (i > 0) twiceArea += px * y - x * py
      px = x
      py = y
    }
    return Math.abs(twiceArea) / 2
  }
  const first = readHeader()
  if (first.type === 3) {
    const rings = view.getUint32(p, first.little)
    p += 4
    let area = 0
    for (let r = 0; r < rings; r++) {
      const ring = readRingArea(first.little)
      if (r === 0) area = ring
      else area -= ring // interior courtyards carry no building mass
    }
    return Math.max(area, 0)
  }
  if (first.type === 6) {
    const parts = view.getUint32(p, first.little)
    p += 4
    let area = 0
    for (let i = 0; i < parts; i++) {
      const part = readHeader()
      const rings = view.getUint32(p, part.little)
      p += 4
      for (let r = 0; r < rings; r++) {
        const ring = readRingArea(part.little)
        if (r === 0) area += ring
        else area -= ring
      }
    }
    return Math.max(area, 0)
  }
  return 0 // not a polygon — the ingest contract says it cannot happen
}

/**
 * Reads the staged obstacle store and answers the urban/rural question for a
 * point. Cells are loaded once and LRU-cached; the enricher walks one H3R4 hex
 * at a time, so the working set is that hex plus whatever its border segments
 * reach into.
 */
export class BuildingFootprintSampler {
  private cells = new Map<string, CellFootprints | null>()
  private manifest: Set<string> | null | undefined

  constructor(
    private storeDir: string = OBSTACLE_STORE_DIR,
    private manifestPath: string = OBSTACLE_INGEST_MANIFEST,
    private maxCachedCells = 8,
  ) {}

  /** SW-corner 1° tile name, e.g. (49.78, 14.17) → "N49E014" — the manifest's
   *  form, and the retired raster's file name. */
  static tileNameFor(lat: number, lon: number): string {
    const la = Math.floor(lat)
    const lo = Math.floor(lon)
    const ns = la >= 0 ? 'N' : 'S'
    const ew = lo >= 0 ? 'E' : 'W'
    return `${ns}${String(Math.abs(la)).padStart(2, '0')}${ew}${String(Math.abs(lo)).padStart(3, '0')}`
  }

  private ingestedTiles(): Set<string> | null {
    if (this.manifest === undefined) {
      this.manifest = existsSync(this.manifestPath)
        ? new Set(
            readFileSync(this.manifestPath, 'utf8')
              .split('\n')
              .map((l) => l.trim())
              .filter((l) => l.length > 0),
          )
        : null
    }
    return this.manifest
  }

  private cellFootprints(cell: string): CellFootprints | null {
    const hit = this.cells.get(cell)
    if (hit !== undefined) {
      this.cells.delete(cell) // re-insert → most-recently-used
      this.cells.set(cell, hit)
      return hit
    }
    const loaded = this.loadCell(cell)
    this.cells.set(cell, loaded)
    if (this.cells.size > this.maxCachedCells) {
      const oldest = this.cells.keys().next().value
      if (oldest !== undefined) this.cells.delete(oldest)
    }
    return loaded
  }

  private loadCell(cell: string): CellFootprints | null {
    const dir = resolve(this.storeDir, cell)
    let shards: string[]
    try {
      shards = readdirSync(dir)
        .filter((f) => f.startsWith('obstacles') && f.endsWith('.arrow'))
        .sort()
    } catch (err) {
      // Absence is a legitimate "this cell received no footprint"; the
      // manifest — not the shard tree — decides whether that is knowledge.
      if ((err as NodeJS.ErrnoException).code === 'ENOENT') return null
      throw err
    }
    if (shards.length === 0) return null

    const lats: number[] = []
    const lons: number[] = []
    const areas: number[] = []
    for (const shard of shards) {
      const table = tableFromIPC(readFileSync(resolve(dir, shard)))
      const clat = table.getChild('centroid_lat')
      const clon = table.getChild('centroid_lon')
      const wkb = table.getChild('polygon_wkb')
      if (!clat || !clon || !wkb) throw new Error(`obstacle shard missing columns: ${dir}/${shard}`)
      for (let i = 0; i < table.numRows; i++) {
        const la = clat.get(i) as number | null
        const lo = clon.get(i) as number | null
        const geom = wkb.get(i) as Uint8Array | null
        if (la === null || lo === null || geom === null) continue
        lats.push(la)
        lons.push(lo)
        areas.push(wkbFootprintAreaM2(geom))
      }
    }

    const n = lats.length
    const lat = Float64Array.from(lats)
    const lon = Float64Array.from(lons)
    const areaM2 = Float64Array.from(areas)
    // Counting sort into per-bucket runs: one pass to size the runs, one to
    // fill them. Keeps the whole index in two flat arrays plus a small Map.
    const counts = new Map<number, number>()
    const keys = new Float64Array(n)
    for (let i = 0; i < n; i++) {
      const k = bucketKey(Math.floor(lat[i] / BUCKET_DEG), Math.floor(lon[i] / BUCKET_DEG))
      keys[i] = k
      counts.set(k, (counts.get(k) ?? 0) + 1)
    }
    const bucket = new Map<number, [number, number]>()
    const cursor = new Map<number, number>()
    let acc = 0
    for (const [k, c] of counts) {
      bucket.set(k, [acc, acc + c])
      cursor.set(k, acc)
      acc += c
    }
    const order = new Uint32Array(n)
    for (let i = 0; i < n; i++) {
      const k = keys[i]
      const p = cursor.get(k)!
      order[p] = i
      cursor.set(k, p + 1)
    }
    return { lat, lon, areaM2, bucket, order }
  }

  /**
   * How many footprints have their CENTROID within
   * [`BUILT_UP_WINDOW_HALF_DEG`] of the point in both axes, and how much
   * ground area they carry, or `null` when a 1° tile the window touches was
   * never ingested.
   *
   * Centroid attribution rather than clipped overlap: a footprint is metres
   * across and the window is hundreds, so which side of the border its area
   * lands on is noise next to the pixel this feeds — and centroid ownership is
   * also how the store assigns footprints to cells in the first place.
   *
   * `count` is not part of the decision: it was the rival statistic the
   * calibration rejected, and `research/2026-08-built-up-vector` still reads it
   * to re-derive that comparison.
   */
  windowFootprints(lat: number, lon: number): { count: number; areaM2: number } | null {
    const tiles = this.ingestedTiles()
    if (!tiles) return null
    const h = BUILT_UP_WINDOW_HALF_DEG
    const latLo = lat - h
    const latHi = lat + h
    const lonLo = lon - h
    const lonHi = lon + h
    for (const [tLat, tLon] of [
      [latLo, lonLo],
      [latLo, lonHi],
      [latHi, lonLo],
      [latHi, lonHi],
    ]) {
      if (!tiles.has(BuildingFootprintSampler.tileNameFor(tLat, tLon))) return null
    }

    // Every R4 cell the window overlaps. Sampled on a 3×3 grid of the window
    // rather than via grid_disk(1): an R4 cell is ~22 km across and the window
    // is half a kilometre, so at most a cell corner falls inside it, and the
    // nine points always land in each of the (at most three) cells that meet
    // there — while grid_disk(1) would load six neighbours the window never
    // touches.
    const cells = new Set<string>()
    for (const la of [latLo, lat, latHi]) {
      for (const lo of [lonLo, lon, lonHi]) cells.add(latLngToCell(la, lo, 4))
    }

    let area = 0
    let count = 0
    for (const cell of cells) {
      const fp = this.cellFootprints(cell)
      if (!fp) continue // ingested and empty — proven by the manifest above
      const latIdx0 = Math.floor(latLo / BUCKET_DEG)
      const latIdx1 = Math.floor(latHi / BUCKET_DEG)
      const lonIdx0 = Math.floor(lonLo / BUCKET_DEG)
      const lonIdx1 = Math.floor(lonHi / BUCKET_DEG)
      for (let bi = latIdx0; bi <= latIdx1; bi++) {
        for (let bj = lonIdx0; bj <= lonIdx1; bj++) {
          const run = fp.bucket.get(bucketKey(bi, bj))
          if (!run) continue
          for (let p = run[0]; p < run[1]; p++) {
            const i = fp.order[p]
            const la = fp.lat[i]
            const lo = fp.lon[i]
            if (la >= latLo && la <= latHi && lo >= lonLo && lo <= lonHi) {
              area += fp.areaM2[i]
              count++
            }
          }
        }
      }
    }
    return { count, areaM2: area }
  }

  /**
   * The retired raster's statistic, estimated from the polygons: how many
   * 1/3600° pixels of the window would carry a building. `null` ⇒ no coverage.
   */
  estimatedBuiltPixels(lat: number, lon: number): number | null {
    const w = this.windowFootprints(lat, lon)
    return w === null ? null : w.areaM2 / rasterPixelAreaM2(lat)
  }

  /** The calibrated urban/rural decision for one point (a road-segment midpoint). */
  classifyBuiltUp(lat: number, lon: number): number {
    const pixels = this.estimatedBuiltPixels(lat, lon)
    if (pixels === null) return BUILT_UP_UNKNOWN
    return pixels >= BUILT_UP_MIN_BUILT_PIXELS ? BUILT_UP_URBAN : BUILT_UP_RURAL
  }
}
