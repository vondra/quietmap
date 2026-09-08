/** Grid raster tiles (DEM elevation, forest cover, imperviousness) out of the
 * per-square native-lattice files (`<prepared>/z9/<x>/<y>/{dem.i16be,
 * forest.u8,imd.u8}`). Port of the proven dev1 palettes; the z9 server had
 * dropped these renderers, leaving 404 overlay switches.
 *
 * File layout mirrors `engine/grid/src/raster.rs` exactly: row 0 is north,
 * row-major nodes (not cells), 3600 nodes/degree, z9 square edges from the
 * same integer arithmetic. Squares load whole into an LRU (a metro square's
 * dem is ~13 MB); tiles sample per pixel, so one square serves every zoom.
 */

import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { encodePNG, getEmptyPng } from './raster-tile-renderer.js'

export type GridChannel = 'dem' | 'forest' | 'imd'

const NODES_PER_DEGREE = 3600
const LONGITUDE_NODES = 360 * NODES_PER_DEGREE
const POLE_NODE = 90 * NODES_PER_DEGREE
const Z9_AXIS = 512
const TILE_PX = 256

const SQUARE_CACHE_MAX = 8

type SquareGrid = {
  columns: number
  rows: number
  westNode: number
  northNode: number
  buf: Buffer
  bytesPerNode: number
}

const cache = new Map<string, SquareGrid>()

function latitudeEdgeNode(y: number): number {
  const mercator = Math.PI * (1 - (2 * y) / Z9_AXIS)
  // Exact port of Rust `latitude_edge_node` (mercator.sinh().atan()):
  // inverse-mercator latitude of the square edge, in nodes.
  return (Math.atan(Math.sinh(mercator)) * 180) / Math.PI * NODES_PER_DEGREE
}
/** Test seam: exact port of `RasterWindow::for_square`. */
export function squareWindow(x: number, y: number): { westNode: number; northNode: number; rows: number; columns: number } {
  const westNode = Math.floor((x * LONGITUDE_NODES) / Z9_AXIS) - LONGITUDE_NODES / 2
  const eastNode = Math.floor(((x + 1) * LONGITUDE_NODES + Z9_AXIS - 1) / Z9_AXIS) - LONGITUDE_NODES / 2
  const northNode = y === 0 ? POLE_NODE : Math.ceil(latitudeEdgeNode(y))
  const southNode = y === Z9_AXIS - 1 ? -POLE_NODE : Math.floor(latitudeEdgeNode(y + 1))
  return { westNode, northNode, rows: northNode - southNode + 1, columns: eastNode - westNode + 1 }
}

function normalizeLongitude(lon: number): number {
  let v = lon % 360
  if (v < -180) v += 360
  else if (v >= 180) v -= 360
  return v
}

function channelFile(channel: GridChannel): { file: string; bytesPerNode: number } {
  return channel === 'dem'
    ? { file: 'dem.i16be', bytesPerNode: 2 }
    : { file: channel === 'forest' ? 'forest.u8' : 'imd.u8', bytesPerNode: 1 }
}

async function loadSquare(
  preparedYearDir: string, channel: GridChannel, sx: number, sy: number,
): Promise<SquareGrid | null> {
  // Keyed by data root (gg finding 9): two checkouts must never share squares.
  const key = `${preparedYearDir}|${channel}:${sx}:${sy}`
  const hit = cache.get(key)
  if (hit) {
    cache.delete(key)
    cache.set(key, hit)
    return hit
  }
  const { file, bytesPerNode } = channelFile(channel)
  const w = squareWindow(sx, sy)
  let buf: Buffer
  try {
    buf = await readFile(join(preparedYearDir, 'z9', String(sx), String(sy), file))
  } catch (err: unknown) {
    if ((err as NodeJS.ErrnoException).code === 'ENOENT') return null
    throw err
  }
  if (buf.length < w.rows * w.columns * bytesPerNode) return null
  const grid: SquareGrid = { ...w, buf, bytesPerNode }
  cache.delete(key)
  cache.set(key, grid)
  while (cache.size > SQUARE_CACHE_MAX) cache.delete(cache.keys().next().value!)
  return grid
}

/** Nearest node value at lat/lon, or null outside coverage/void. Mirrors
 * `RasterWindow::sample_position` (nearest branch). */
function sampleNearest(grid: SquareGrid, channel: GridChannel, lat: number, lon: number): number | null {
  if (!Number.isFinite(lat) || !Number.isFinite(lon) || lat < -90 || lat > 90) return null
  const lonN = normalizeLongitude(lon)
  const sourceLat = Math.min(89, Math.floor(lat))
  const sourceLon = Math.floor(lonN)
  const sourceRow = (1 - (lat - sourceLat)) * NODES_PER_DEGREE
  const sourceCol = (lonN - sourceLon) * NODES_PER_DEGREE
  const row = grid.northNode - (sourceLat + 1) * NODES_PER_DEGREE + Math.round(sourceRow)
  const col = sourceLon * NODES_PER_DEGREE - grid.westNode + Math.round(sourceCol)
  if (row < 0 || row >= grid.rows || col < 0 || col >= grid.columns) return null
  const off = (row * grid.columns + col) * grid.bytesPerNode
  if (channel === 'dem') {
    const v = grid.buf.readInt16BE(off)
    return v === -32768 ? null : v
  }
  const v = grid.buf[off]
  return v > 100 ? null : v
}

// DEM colormap green → tan → brown → gray → white (proven dev1 palette).
const DEM_STOPS: [number, number, number, number][] = [
  [0, 0x2d, 0x6a, 0x4f],
  [200, 0x74, 0xc6, 0x9d],
  [500, 0xd4, 0xa3, 0x73],
  [1000, 0xbc, 0x6c, 0x25],
  [2000, 0x8b, 0x8b, 0x8b],
  [4000, 0xf0, 0xf0, 0xf0],
]

export function demColor(elev: number): [number, number, number, number] {
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

export function forestColor(v: number): [number, number, number, number] {
  if (v <= 0) return [0, 0, 0, 0]
  const alpha = Math.round((150 * Math.min(v, 100)) / 100)
  return [0x2d, 0x6a, 0x4f, Math.max(alpha, 40)]
}

export function imdColor(v: number): [number, number, number, number] {
  if (v <= 0) return [0, 0, 0, 0]
  // Sealed-surface ramp: pale yellow → orange → deep red, alpha with cover.
  const t = Math.min(v, 100) / 100
  const r = Math.round(0xfd + t * (0xd7 - 0xfd))
  const g = Math.round(0xe0 + t * (0x30 - 0xe0))
  const b = Math.round(0x8b + t * (0x27 - 0x8b))
  return [r, g, b, Math.round(60 + 120 * t)]
}

function paint(channel: GridChannel, v: number): [number, number, number, number] {
  return channel === 'dem' ? demColor(v) : channel === 'forest' ? forestColor(v) : imdColor(v)
}

function squareFor(lat: number, lon: number): [number, number] {
  const sx = Math.min(Z9_AXIS - 1, Math.max(0, Math.floor(((normalizeLongitude(lon) + 180) / 360) * Z9_AXIS)))
  const mercY = Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360))
  const sy = Math.min(Z9_AXIS - 1, Math.max(0, Math.floor(((1 - mercY / Math.PI) / 2) * Z9_AXIS)))
  return [sx, sy]
}

export async function renderGridTile(
  preparedYearDir: string,
  channel: GridChannel,
  z: number,
  x: number,
  y: number,
): Promise<Buffer> {
  const n = 2 ** z
  const lonWest = (x / n) * 360 - 180
  const lonEast = ((x + 1) / n) * 360 - 180
  const invMerc = (ty: number): number =>
    (Math.atan(Math.sinh(Math.PI * (1 - (2 * ty) / n))) * 180) / Math.PI
  const pixels = Buffer.alloc(TILE_PX * TILE_PX * 4)
  // Per-render memo: one square loads once per tile however many pixels
  // need it — including the MISSING outcome, so an ocean tile issues one
  // lookup per square, not 65k (gg finding 2). Keyed by data root: two
  // checkouts must never share squares (gg finding 9).
  const local = new Map<string, Promise<SquareGrid | null>>()
  const squareAt = (sx: number, sy: number): Promise<SquareGrid | null> => {
    const key = `${preparedYearDir}|${channel}:${sx}:${sy}`
    let pending = local.get(key)
    if (!pending) {
      pending = loadSquare(preparedYearDir, channel, sx, sy)
      local.set(key, pending)
    }
    return pending
  }
  let any = false
  for (let py = 0; py < TILE_PX; py++) {
    // Exact inverse-Mercator per row: linear latitude interpolation shifts
    // up to 3 px inside a tile (gg finding 5).
    const lat = invMerc(y + (py + 0.5) / TILE_PX)
    const jobs: Promise<void>[] = []
    for (let px = 0; px < TILE_PX; px++) {
      const lon = lonWest + ((lonEast - lonWest) * (px + 0.5)) / TILE_PX
      const o = (py * TILE_PX + px) * 4
      jobs.push((async () => {
        const [sx, sy] = squareFor(lat, lon)
        const grid = await squareAt(sx, sy)
        const v = grid ? sampleNearest(grid, channel, lat, lon) : null
        if (v === null) return
        const [r, g, b, a] = paint(channel, v)
        if (a === 0) return
        pixels[o] = r
        pixels[o + 1] = g
        pixels[o + 2] = b
        pixels[o + 3] = a
        any = true
      })())
    }
    // eslint-disable-next-line no-await-in-loop
    await Promise.all(jobs)
  }
  if (!any) return getEmptyPng()
  return encodePNG(TILE_PX, pixels)
}
