import { readFile } from 'node:fs/promises'
import { existsSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { deflateSync, crc32 } from 'node:zlib'
import { tableFromIPC } from 'apache-arrow'
import { DATA_YEAR } from '../data-year.js'

type LayerId = 'dem' | 'forest' | 'barriers'

interface SourceTile {
  data: Int16Array | Uint8Array
  samples: number
}

const DATA_DIR = resolve(import.meta.dirname, '..', '..', '..', 'data', 'prepared')
const DEM_DIRS = [
  join(DATA_DIR, 'dem', 'copernicus'),
  join(DATA_DIR, 'dem', 'srtm'),
]
const FOREST_DIR = join(DATA_DIR, 'rasters', 'forest')

const MAX_TILES_PER_LAYER = 5
// Per-layer Map for O(1) LRU via insertion order (delete + re-set promotes)
const tileCacheByLayer = new Map<LayerId, Map<string, SourceTile | null>>()

function getLayerCache(layer: LayerId): Map<string, SourceTile | null> {
  let m = tileCacheByLayer.get(layer)
  if (!m) { m = new Map(); tileCacheByLayer.set(layer, m) }
  return m
}

function evictIfNeeded(layer: LayerId): void {
  const cache = getLayerCache(layer)
  while (cache.size >= MAX_TILES_PER_LAYER) {
    const oldest = cache.keys().next().value!
    cache.delete(oldest)
  }
}

function tileName(latInt: number, lonInt: number, ext: string): string {
  const ns = latInt >= 0 ? 'N' : 'S'
  const ew = lonInt >= 0 ? 'E' : 'W'
  return `${ns}${String(Math.abs(latInt)).padStart(2, '0')}${ew}${String(Math.abs(lonInt)).padStart(3, '0')}.${ext}`
}

async function readSourceTile(layer: LayerId, latInt: number, lonInt: number): Promise<SourceTile | null> {
  const cache = getLayerCache(layer)
  const cacheKey = `${latInt}:${lonInt}`
  const cached = cache.get(cacheKey)
  if (cached !== undefined) {
    // Promote to newest
    cache.delete(cacheKey)
    cache.set(cacheKey, cached)
    return cached
  }

  let tilePath: string | null = null
  if (layer === 'dem') {
    const fname = tileName(latInt, lonInt, 'hgt')
    for (const dir of DEM_DIRS) {
      const p = join(dir, fname)
      if (existsSync(p)) { tilePath = p; break }
    }
  } else {
    const fname = tileName(latInt, lonInt, 'raw')
    const p = join(FOREST_DIR, fname)
    if (existsSync(p)) tilePath = p
  }

  if (!tilePath) {
    cache.set(cacheKey, null)
    return null
  }

  evictIfNeeded(layer)

  const buf = await readFile(tilePath)
  let tile: SourceTile

  if (layer === 'dem') {
    const data = new Int16Array(buf.byteLength / 2)
    for (let i = 0; i < data.length; i++) {
      data[i] = (buf[i * 2] << 8) | buf[i * 2 + 1]
      if (data[i] === -32768) data[i] = 0
    }
    tile = { data, samples: Math.round(Math.sqrt(data.length)) }
  } else {
    const data = new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength)
    tile = { data, samples: Math.round(Math.sqrt(data.length)) }
  }

  cache.set(cacheKey, tile)
  return tile
}

function sampleNearest(tile: SourceTile, fracLat: number, fracLon: number): number {
  const s = tile.samples
  const row = Math.round((1.0 - fracLat) * (s - 1))
  const col = Math.round(fracLon * (s - 1))
  const r = Math.max(0, Math.min(s - 1, row))
  const c = Math.max(0, Math.min(s - 1, col))
  return tile.data[r * s + c]
}

function sampleBilinear(tile: SourceTile, fracLat: number, fracLon: number): number {
  const s = tile.samples
  const row = (1.0 - fracLat) * (s - 1)
  const col = fracLon * (s - 1)
  const ri = Math.min(Math.floor(row), s - 2)
  const ci = Math.min(Math.floor(col), s - 2)
  const rf = row - ri
  const cf = col - ci
  const v00 = tile.data[ri * s + ci]
  const v01 = tile.data[ri * s + ci + 1]
  const v10 = tile.data[(ri + 1) * s + ci]
  const v11 = tile.data[(ri + 1) * s + ci + 1]
  return v00 * (1 - cf) * (1 - rf) + v01 * cf * (1 - rf) + v10 * (1 - cf) * rf + v11 * cf * rf
}

// DEM colormap: green → tan → brown → gray → white
const DEM_STOPS: [number, number, number, number][] = [
  [0, 0x2d, 0x6a, 0x4f],
  [200, 0x74, 0xc6, 0x9d],
  [500, 0xd4, 0xa3, 0x73],
  [1000, 0xbc, 0x6c, 0x25],
  [2000, 0x8b, 0x8b, 0x8b],
  [4000, 0xf0, 0xf0, 0xf0],
]

function demColor(elev: number): [number, number, number, number] {
  if (elev <= 0) return [0x2d, 0x6a, 0x4f, 160]
  for (let i = 1; i < DEM_STOPS.length; i++) {
    if (elev <= DEM_STOPS[i][0]) {
      const [e0, r0, g0, b0] = DEM_STOPS[i - 1]
      const [e1, r1, g1, b1] = DEM_STOPS[i]
      const t = (elev - e0) / (e1 - e0)
      return [
        Math.round(r0 + t * (r1 - r0)),
        Math.round(g0 + t * (g1 - g0)),
        Math.round(b0 + t * (b1 - b0)),
        160,
      ]
    }
  }
  return [0xf0, 0xf0, 0xf0, 160]
}

function buildingColor(h: number): [number, number, number, number] {
  if (h === 0) return [0, 0, 0, 0]
  if (h <= 5) return [0xfe, 0xe0, 0x8b, 180]
  if (h <= 15) return [0xfd, 0xae, 0x61, 180]
  if (h <= 30) return [0xf4, 0x6d, 0x43, 180]
  if (h <= 60) return [0xd7, 0x30, 0x27, 180]
  return [0xa5, 0x00, 0x26, 180]
}

function forestColor(v: number): [number, number, number, number] {
  if (v === 0) return [0, 0, 0, 0]
  // Canopy-density ramp (geodata-v2 2a): alpha scales with v ∈ (0, 100].
  // The legacy binary raster reads 100 where forested → the old flat green.
  const alpha = Math.round(150 * Math.min(v, 100) / 100)
  return [0x2d, 0x6a, 0x4f, Math.max(alpha, 40)]
}

// --- Barrier data (wall micro-segments from the per-cell structure table) ---

interface BarrierSegment {
  startLat: number; startLon: number
  endLat: number; endLon: number
  height: number
}

let barrierSegments: BarrierSegment[] | null = null

let barrierLoadPromise: Promise<BarrierSegment[]> | null = null

function yieldToEventLoop(): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, 0))
}

/** A barrier row's geometry: the 2-point little-endian WKB LineString
 *  build-structures.py writes (byte 0 = 1 for LE, u32 type 2 at [1..5], u32
 *  npoints=2 at [5..9], then four f64 LE as lon/lat, lon/lat — 41 bytes). */
function wallSegmentFromWkb(wkb: Uint8Array): { startLat: number; startLon: number; endLat: number; endLon: number } | null {
  if (wkb.byteLength !== 41) return null
  const view = new DataView(wkb.buffer, wkb.byteOffset, wkb.byteLength)
  if (view.getUint8(0) !== 1 || view.getUint32(1, true) !== 2 || view.getUint32(5, true) !== 2) return null
  return {
    startLon: view.getFloat64(9, true), startLat: view.getFloat64(17, true),
    endLon: view.getFloat64(25, true), endLat: view.getFloat64(33, true),
  }
}

async function loadAllBarriersAsync(): Promise<BarrierSegment[]> {
  if (barrierSegments) return barrierSegments
  const h3r4Dir = join(DATA_DIR, DATA_YEAR, 'h3r4')
  const result: BarrierSegment[] = []
  if (!existsSync(h3r4Dir)) { barrierSegments = result; return result }
  const hexDirs = readdirSync(h3r4Dir)
  let filesProcessed = 0
  for (const hex of hexDirs) {
    const fp = join(h3r4Dir, hex, 'structures.arrow')
    if (!existsSync(fp)) continue
    try {
      const buf = await readFile(fp)
      const table = tableFromIPC(buf)
      const kind = table.getChild('kind')
      const geom = table.getChild('geometry_wkb')
      const height = table.getChild('height_m')
      if (!kind || !geom) continue
      for (let i = 0; i < table.numRows; i++) {
        if ((kind.get(i) as number) !== 1) continue // walls only (ObstacleKind::Barrier)
        const wkb = geom.get(i) as Uint8Array | null
        const seg = wkb ? wallSegmentFromWkb(wkb) : null
        if (!seg) continue
        result.push({ ...seg, height: (height?.get(i) as number | null) ?? 3.0 })
      }
    } catch { /* skip corrupt files */ }
    // Yield to event loop every 50 files so server stays responsive
    if (++filesProcessed % 50 === 0) await yieldToEventLoop()
  }
  barrierSegments = result
  console.log(`barriers: loaded ${result.length} segments from ${h3r4Dir}`)
  return result
}

function loadAllBarriers(): BarrierSegment[] {
  return barrierSegments ?? []
}

// Spatial index: barriers bucketed by 1° lat/lon cell for fast tile lookup
let barrierIndex: Map<string, BarrierSegment[]> | null = null

function getBarriersInBbox(south: number, north: number, west: number, east: number): BarrierSegment[] {
  const all = loadAllBarriers()
  if (all.length === 0) return []

  if (!barrierIndex) {
    barrierIndex = new Map()
    for (const seg of all) {
      const midLat = (seg.startLat + seg.endLat) / 2
      const midLon = (seg.startLon + seg.endLon) / 2
      const key = `${Math.floor(midLat)}:${Math.floor(midLon)}`
      let bucket = barrierIndex.get(key)
      if (!bucket) { bucket = []; barrierIndex.set(key, bucket) }
      bucket.push(seg)
    }
  }

  const result: BarrierSegment[] = []
  for (let lat = Math.floor(south); lat <= Math.floor(north); lat++) {
    for (let lon = Math.floor(west); lon <= Math.floor(east); lon++) {
      const bucket = barrierIndex.get(`${lat}:${lon}`)
      if (bucket) {
        for (const seg of bucket) {
          const midLat = (seg.startLat + seg.endLat) / 2
          const midLon = (seg.startLon + seg.endLon) / 2
          if (midLat >= south && midLat <= north && midLon >= west && midLon <= east) {
            result.push(seg)
          }
        }
      }
    }
  }
  return result
}

/// One obstacle footprint from the popup worker's store — the wire shape of
/// sourceModule.queryObstacleFootprints (o = outer ring [[lat,lon]…],
/// h = AS-USED height in m after the low-profile cap, t = height tier
/// 0 mapped/1 floors/2 default/3 city-measured/4 ANBH prior, c = capped).
type FootprintRow = { o: [number, number][]; h: number; t: number; c: boolean }

/// The building-height overlay as MODEL TRUTH (owner ask 2026-08-02): fill
/// the exact obstacle-store footprints the propagation engine screens with,
/// colored by their as-used height (the buildingColor ramp), with capped
/// footprints rendered at their capped height. Even-odd scanline fill on the
/// 256 px tile grid. This is the ONLY building renderer — the 30 m building
/// raster it used to fall back to no longer exists.
export async function renderBuildingVectorTile(
  z: number,
  x: number,
  y: number,
  fetchFootprintsJson: (south: number, west: number, north: number, east: number) => Promise<string>,
): Promise<Buffer> {
  const { latNorth, latSouth, lonWest, lonEast } = tileToLatLonBbox(z, x, y)
  // Deliberately uncaught: an unreachable or missing obstacle store must reach
  // the caller as an error. Answering with an empty tile would draw a
  // building-free city — "no data" rendered as silence.
  const rows = JSON.parse(
    await fetchFootprintsJson(latSouth, lonWest, latNorth, lonEast),
  ) as FootprintRow[]
  // A successful query with zero rows is real emptiness (ocean, desert), not absence.
  if (!rows.length) return getEmptyPng()

  const W = 256
  const pixels = Buffer.alloc(W * W * 4)
  const mercYNorth = Math.log(Math.tan(Math.PI / 4 + (latNorth * Math.PI) / 360))
  const mercYSouth = Math.log(Math.tan(Math.PI / 4 + (latSouth * Math.PI) / 360))
  const toPx = (lat: number, lon: number): [number, number] => {
    const mercY = Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360))
    return [
      ((lon - lonWest) / (lonEast - lonWest)) * W,
      ((mercY - mercYNorth) / (mercYSouth - mercYNorth)) * W,
    ]
  }

  for (const f of rows) {
    if (f.o.length < 3) continue
    const ring = f.o.map(([la, lo]) => toPx(la, lo))
    const [r, g, b] = buildingColor(f.h)
    // Even-odd scanline fill over the ring's pixel bbox (clamped to tile).
    let yMin = Infinity, yMax = -Infinity
    for (const [, py] of ring) { yMin = Math.min(yMin, py); yMax = Math.max(yMax, py) }
    const y0 = Math.max(0, Math.floor(yMin)), y1 = Math.min(W - 1, Math.ceil(yMax))
    for (let sy = y0; sy <= y1; sy++) {
      const cy = sy + 0.5
      const xs: number[] = []
      for (let i = 0; i < ring.length; i++) {
        const [ax, ay] = ring[i]
        const [bx, by] = ring[(i + 1) % ring.length]
        if ((ay > cy) !== (by > cy)) xs.push(ax + ((cy - ay) / (by - ay)) * (bx - ax))
      }
      xs.sort((p, q) => p - q)
      for (let k = 0; k + 1 < xs.length; k += 2) {
        const x0 = Math.max(0, Math.round(xs[k])), x1c = Math.min(W - 1, Math.round(xs[k + 1]))
        for (let sx = x0; sx <= x1c; sx++) {
          const idx = (sy * W + sx) * 4
          pixels[idx] = r; pixels[idx + 1] = g; pixels[idx + 2] = b; pixels[idx + 3] = 210
        }
      }
    }
  }
  return encodePNG(W, W, pixels)
}

async function renderBarrierTile(z: number, x: number, y: number): Promise<Buffer> {
  if (!barrierSegments) await preloadBarriers()
  const { latNorth, latSouth, lonWest, lonEast } = tileToLatLonBbox(z, x, y)
  const barriers = getBarriersInBbox(latSouth, latNorth, lonWest, lonEast)
  if (barriers.length === 0) return getEmptyPng()

  const W = 256
  const pixels = Buffer.alloc(W * W * 4)
  const mercYNorth = Math.log(Math.tan(Math.PI / 4 + (latNorth * Math.PI) / 360))
  const mercYSouth = Math.log(Math.tan(Math.PI / 4 + (latSouth * Math.PI) / 360))

  const lineWidth = Math.max(2, Math.min(6, z - 10))

  for (const seg of barriers) {
    // Convert lat/lon to pixel coordinates
    const mercY1 = Math.log(Math.tan(Math.PI / 4 + (seg.startLat * Math.PI) / 360))
    const mercY2 = Math.log(Math.tan(Math.PI / 4 + (seg.endLat * Math.PI) / 360))
    const px1 = Math.round(((seg.startLon - lonWest) / (lonEast - lonWest)) * W)
    const py1 = Math.round(((mercY1 - mercYNorth) / (mercYSouth - mercYNorth)) * W)
    const px2 = Math.round(((seg.endLon - lonWest) / (lonEast - lonWest)) * W)
    const py2 = Math.round(((mercY2 - mercYNorth) / (mercYSouth - mercYNorth)) * W)

    // Color by height: magenta, brighter = taller
    const h = Math.min(seg.height, 10)
    const intensity = Math.round(150 + (h / 10) * 105) // 150-255
    const r = intensity
    const g = 30
    const b = intensity

    // Draw line with Bresenham + thickness
    drawThickLine(pixels, W, px1, py1, px2, py2, r, g, b, 220, lineWidth)
  }

  return encodePNG(W, W, pixels)
}

function drawThickLine(
  pixels: Buffer, W: number,
  x0: number, y0: number, x1: number, y1: number,
  r: number, g: number, b: number, a: number,
  thickness: number,
) {
  const half = Math.floor(thickness / 2)
  const dx = Math.abs(x1 - x0)
  const dy = Math.abs(y1 - y0)
  const sx = x0 < x1 ? 1 : -1
  const sy = y0 < y1 ? 1 : -1
  let err = dx - dy
  let cx = x0, cy = y0

  for (let step = 0; step <= dx + dy; step++) {
    for (let t = -half; t <= half; t++) {
      if (dx >= dy) {
        setPixel(pixels, W, cx, cy + t, r, g, b, a)
      } else {
        setPixel(pixels, W, cx + t, cy, r, g, b, a)
      }
    }
    if (cx === x1 && cy === y1) break
    const e2 = 2 * err
    if (e2 > -dy) { err -= dy; cx += sx }
    if (e2 < dx) { err += dx; cy += sy }
  }
}

function setPixel(pixels: Buffer, W: number, x: number, y: number, r: number, g: number, b: number, a: number) {
  if (x < 0 || x >= W || y < 0 || y >= W) return
  const off = (y * W + x) * 4
  pixels[off] = r; pixels[off + 1] = g; pixels[off + 2] = b; pixels[off + 3] = a
}

function tileToLatLonBbox(z: number, x: number, y: number) {
  const n = 2 ** z
  const lonWest = (x / n) * 360 - 180
  const lonEast = ((x + 1) / n) * 360 - 180
  const latNorth = Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * (180 / Math.PI)
  const latSouth = Math.atan(Math.sinh(Math.PI * (1 - (2 * (y + 1)) / n))) * (180 / Math.PI)
  return { latNorth, latSouth, lonWest, lonEast }
}

type TileMap = Map<string, SourceTile | null>

async function collectSourceTiles(
  layer: LayerId, latSouth: number, latNorth: number, lonWest: number, lonEast: number,
): Promise<TileMap> {
  const tiles: TileMap = new Map()
  const latMin = Math.floor(latSouth)
  const latMax = Math.floor(latNorth)
  const lonMin = Math.floor(lonWest)
  const lonMax = Math.floor(lonEast)
  const promises: Promise<void>[] = []
  for (let lat = latMin; lat <= latMax; lat++) {
    for (let lon = lonMin; lon <= lonMax; lon++) {
      const key = `${lat}:${lon}`
      promises.push(readSourceTile(layer, lat, lon).then(t => { tiles.set(key, t) }))
    }
  }
  await Promise.all(promises)
  return tiles
}

export async function renderTile(layer: LayerId, z: number, x: number, y: number): Promise<Buffer> {
  if (layer === 'barriers') return renderBarrierTile(z, x, y)

  const { latNorth, latSouth, lonWest, lonEast } = tileToLatLonBbox(z, x, y)
  const mercYNorth = Math.log(Math.tan(Math.PI / 4 + (latNorth * Math.PI) / 360))
  const mercYSouth = Math.log(Math.tan(Math.PI / 4 + (latSouth * Math.PI) / 360))
  const sourceTiles = await collectSourceTiles(layer, latSouth, latNorth, lonWest, lonEast)

  const W = 256
  const pixels = Buffer.alloc(W * W * 4)
  const colorFn = layer === 'dem' ? demColor : forestColor
  const bilinear = layer === 'dem'

  for (let py = 0; py < W; py++) {
    const fracY = (py + 0.5) / W
    const mercY = mercYNorth + fracY * (mercYSouth - mercYNorth)
    const lat = (2 * Math.atan(Math.exp(mercY)) - Math.PI / 2) * (180 / Math.PI)

    for (let px = 0; px < W; px++) {
      const lon = lonWest + ((px + 0.5) / W) * (lonEast - lonWest)
      const latInt = Math.floor(lat)
      const lonInt = Math.floor(lon)
      const tile = sourceTiles.get(`${latInt}:${lonInt}`)
      let value = 0
      if (tile) {
        const fracLat = lat - latInt
        const fracLon = lon - lonInt
        value = bilinear ? sampleBilinear(tile, fracLat, fracLon) : sampleNearest(tile, fracLat, fracLon)
      }
      const [r, g, b, a] = colorFn(value)
      const off = (py * W + px) * 4
      pixels[off] = r
      pixels[off + 1] = g
      pixels[off + 2] = b
      pixels[off + 3] = a
    }
  }

  return encodePNG(W, W, pixels)
}

// 64×64 = 4 KB (u8) / 8 KB (i16) per tile. Matches the PNG bbox so the
// client can sample by pixel coord from either endpoint interchangeably.
const DATA_TILE_SIZE = 64

/**
 * Raw raster values for a map tile — DEM as Int16 big-endian (metres,
 * signed), forest as u8. Nearest-neighbour from the 1° source tile.
 * Caller sets content-type.
 */
export async function renderDataTile(
  layer: 'dem' | 'forest',
  z: number,
  x: number,
  y: number,
): Promise<Buffer> {
  const { latNorth, latSouth, lonWest, lonEast } = tileToLatLonBbox(z, x, y)
  const mercYNorth = Math.log(Math.tan(Math.PI / 4 + (latNorth * Math.PI) / 360))
  const mercYSouth = Math.log(Math.tan(Math.PI / 4 + (latSouth * Math.PI) / 360))
  const sourceTiles = await collectSourceTiles(layer, latSouth, latNorth, lonWest, lonEast)

  const N = DATA_TILE_SIZE
  const bytesPerValue = layer === 'dem' ? 2 : 1
  const buf = Buffer.alloc(N * N * bytesPerValue)

  for (let py = 0; py < N; py++) {
    const fracY = (py + 0.5) / N
    const mercY = mercYNorth + fracY * (mercYSouth - mercYNorth)
    const lat = (2 * Math.atan(Math.exp(mercY)) - Math.PI / 2) * (180 / Math.PI)
    const latInt = Math.floor(lat)
    const fracLat = lat - latInt
    for (let px = 0; px < N; px++) {
      const lon = lonWest + ((px + 0.5) / N) * (lonEast - lonWest)
      const lonInt = Math.floor(lon)
      const tile = sourceTiles.get(`${latInt}:${lonInt}`)
      let value = 0
      if (tile) {
        const fracLon = lon - lonInt
        value = sampleNearest(tile, fracLat, fracLon)
      }
      const idx = py * N + px
      if (layer === 'dem') {
        // Negative values are valid (below sea level) on Copernicus DEM.
        const v = Math.max(-32768, Math.min(32767, Math.round(value)))
        buf[idx * 2] = (v >> 8) & 0xff
        buf[idx * 2 + 1] = v & 0xff
      } else {
        buf[idx] = Math.max(0, Math.min(255, Math.round(value)))
      }
    }
  }

  return buf
}

function encodePNG(width: number, height: number, rgba: Buffer): Buffer {
  const filtered = Buffer.alloc(height * (1 + width * 4))
  for (let y = 0; y < height; y++) {
    filtered[y * (1 + width * 4)] = 0 // filter: none
    rgba.copy(filtered, y * (1 + width * 4) + 1, y * width * 4, (y + 1) * width * 4)
  }
  const compressed = deflateSync(filtered, { level: 1 })

  const chunks: Buffer[] = []

  // PNG signature
  chunks.push(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))

  // IHDR
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(width, 0)
  ihdr.writeUInt32BE(height, 4)
  ihdr[8] = 8  // bit depth
  ihdr[9] = 6  // color type: RGBA
  chunks.push(pngChunk('IHDR', ihdr))

  // IDAT
  chunks.push(pngChunk('IDAT', compressed))

  // IEND
  chunks.push(pngChunk('IEND', Buffer.alloc(0)))

  return Buffer.concat(chunks)
}

function pngChunk(type: string, data: Buffer): Buffer {
  const buf = Buffer.alloc(12 + data.length)
  buf.writeUInt32BE(data.length, 0)
  buf.write(type, 4, 4, 'ascii')
  data.copy(buf, 8)
  const crcData = Buffer.alloc(4 + data.length)
  crcData.write(type, 0, 4, 'ascii')
  data.copy(crcData, 4)
  buf.writeUInt32BE(crc32(crcData) >>> 0, 8 + data.length)
  return buf
}

// Transparent 256×256 PNG cached once
let emptyPng: Buffer | null = null
export function getEmptyPng(): Buffer {
  if (!emptyPng) {
    emptyPng = encodePNG(256, 256, Buffer.alloc(256 * 256 * 4))
  }
  return emptyPng
}

/// Preload barriers at server start (async — doesn't block event loop).
export async function preloadBarriers(): Promise<void> {
  if (!barrierLoadPromise) {
    barrierLoadPromise = loadAllBarriersAsync()
  }
  await barrierLoadPromise
}
