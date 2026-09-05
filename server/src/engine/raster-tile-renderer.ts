/** Paint the popup engine's as-used building footprints with the proven dev1 palette. */

import { deflateSync, crc32 } from 'node:zlib'

type FootprintRow = { p: [number, number][][][]; h: number; t: number; c: boolean }
export type QueryObstacleFootprints = (
  south: number, west: number, north: number, east: number,
) => Promise<string>

function buildingColor(height: number): [number, number, number] {
  if (height === 0) return [0, 0, 0]
  if (height <= 5) return [0xfe, 0xe0, 0x8b]
  if (height <= 15) return [0xfd, 0xae, 0x61]
  if (height <= 30) return [0xf4, 0x6d, 0x43]
  if (height <= 60) return [0xd7, 0x30, 0x27]
  return [0xa5, 0x00, 0x26]
}

export async function renderBuildingVectorTile(
  z: number, x: number, y: number, queryObstacleFootprints: QueryObstacleFootprints,
): Promise<Buffer> {
  const n = 2 ** z
  const lonWest = (x / n) * 360 - 180
  const lonEast = ((x + 1) / n) * 360 - 180
  const latNorth = Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * (180 / Math.PI)
  const latSouth = Math.atan(Math.sinh(Math.PI * (1 - (2 * (y + 1)) / n))) * (180 / Math.PI)
  // Only a successful empty query means no buildings. Worker/storage errors
  // must reach the route as errors, never as a transparent city.
  const rows = JSON.parse(
    await queryObstacleFootprints(latSouth, lonWest, latNorth, lonEast),
  ) as FootprintRow[]
  if (!Array.isArray(rows)) throw new Error('Invalid footprint response')
  if (!rows.length) return getEmptyPng()

  const width = 256
  const pixels = Buffer.alloc(width * width * 4)
  const mercYNorth = Math.log(Math.tan(Math.PI / 4 + (latNorth * Math.PI) / 360))
  const mercYSouth = Math.log(Math.tan(Math.PI / 4 + (latSouth * Math.PI) / 360))
  const toPixel = (lat: number, lon: number): [number, number] => {
    const mercY = Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360))
    const longitudeDelta = lon - lonWest
    const localDelta = longitudeDelta < -180 ? longitudeDelta + 360
      : longitudeDelta >= 180 ? longitudeDelta - 360 : longitudeDelta
    return [(localDelta / (lonEast - lonWest)) * width,
      ((mercY - mercYNorth) / (mercYSouth - mercYNorth)) * width]
  }
  for (const footprint of rows) {
    const [red, green, blue] = buildingColor(footprint.h)
    for (const polygon of footprint.p) {
      if (!polygon.length || polygon[0].length < 3) continue
      const rings = polygon.map(ring => ring.map(([lat, lon]) => toPixel(lat, lon)))
      let yMin = Infinity, yMax = -Infinity
      for (const [, py] of rings[0]) { yMin = Math.min(yMin, py); yMax = Math.max(yMax, py) }
      const y0 = Math.max(0, Math.floor(yMin)), y1 = Math.min(width - 1, Math.ceil(yMax))
      const covered = new Uint8Array(width)
      for (let sy = y0; sy <= y1; sy++) {
        covered.fill(0)
        const cy = sy + 0.5
        for (const [ringIndex, ring] of rings.entries()) {
          const intersections: number[] = []
          for (let i = 0; i < ring.length; i++) {
            const [ax, ay] = ring[i]
            const [bx, by] = ring[(i + 1) % ring.length]
            if ((ay > cy) !== (by > cy)) intersections.push(ax + ((cy - ay) / (by - ay)) * (bx - ax))
          }
          intersections.sort((a, b) => a - b)
          for (let k = 0; k + 1 < intersections.length; k += 2) {
            const x0 = Math.max(0, Math.round(intersections[k]))
            const x1 = Math.min(width - 1, Math.round(intersections[k + 1]))
            for (let sx = x0; sx <= x1; sx++) covered[sx] = ringIndex === 0 ? 1 : 0
          }
        }
        // Subtract holes within this polygon, never from already painted buildings.
        for (let sx = 0; sx < width; sx++) {
          if (!covered[sx]) continue
          const offset = (sy * width + sx) * 4
          pixels[offset] = red; pixels[offset + 1] = green
          pixels[offset + 2] = blue; pixels[offset + 3] = 210
        }
      }
    }
  }
  return encodePNG(width, pixels)
}

function encodePNG(width: number, rgba: Buffer): Buffer {
  const filtered = Buffer.alloc(width * (1 + width * 4))
  for (let y = 0; y < width; y++) {
    rgba.copy(filtered, y * (1 + width * 4) + 1, y * width * 4, (y + 1) * width * 4)
  }
  const header = Buffer.alloc(13)
  header.writeUInt32BE(width, 0)
  header.writeUInt32BE(width, 4)
  header[8] = 8
  header[9] = 6
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk('IHDR', header),
    pngChunk('IDAT', deflateSync(filtered, { level: 1 })),
    pngChunk('IEND', Buffer.alloc(0)),
  ])
}

function pngChunk(type: string, data: Buffer): Buffer {
  const chunk = Buffer.alloc(12 + data.length)
  chunk.writeUInt32BE(data.length, 0)
  chunk.write(type, 4, 4, 'ascii')
  data.copy(chunk, 8)
  chunk.writeUInt32BE(crc32(chunk.subarray(4, 8 + data.length)) >>> 0, 8 + data.length)
  return chunk
}

let emptyPng: Buffer | null = null
function getEmptyPng(): Buffer {
  return emptyPng ??= encodePNG(256, Buffer.alloc(256 * 256 * 4))
}
