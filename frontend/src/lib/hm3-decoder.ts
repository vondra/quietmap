// HM3 binary tile decoder (browser-side).
//
// HM3 tiles (the 512@z13 world) are served with `Content-Encoding: br`, so
// the browser has already Brotli-decompressed the response — off the main
// thread, in its network stack — by the time `fetch().arrayBuffer()` resolves.
// This decoder therefore does NO decompression: it validates the 6-byte header
// and returns the raw cells. Mirror of `engine/tile-painter/src/hm3.rs`:
//   0:4  magic "HM3 "   4:1  version = 4   5:1  source_id   6:…  512×512 u8 cells
// Cell bytes 0–253 are 2·Lden (0–126.5 dB); a building cell carries the level
// at the building's noisiest façade.

export const TILE_PX = 512
/** Computed, and no modelled source reaches 0 dB here: quiet, zero energy. */
export const HM3_COMPUTED_SILENCE = 254
/** Not assessed: outside painted coverage, missing input, or a building
 *  without an exposed façade. A tile absent from the archive is all this. */
export const HM3_NOT_ASSESSED = 255
const HEADER_BYTES = 6
const MAGIC = 'HM3 '
const VERSION = 4

export interface DecodedHM3Tile {
  /** `TILE_PX * TILE_PX` cells, row-major (py * TILE_PX + px). */
  cells: Uint8Array
  /** Layer discriminator from the header (frontend palette/legend metadata). */
  sourceId: number
}

/**
 * Fetch + decode one HM3 tile. The server sends `Content-Encoding: br`, so the
 * browser decompresses transparently and we only read the header + cells.
 * Returns `null` for a tile absent from the archive — every cell not assessed —
 * which the server answers as a 200 with an empty body. Throws on an HTTP
 * error or a header/version mismatch: a failed tile is not an absent one.
 */
export async function fetchAndDecodeHM3(
  url: string,
  signal?: AbortSignal,
  priority?: 'low' | 'high' | 'auto',
): Promise<DecodedHM3Tile | null> {
  // `priority` is a Chromium fetch hint (ancestor previews yield to sharp
  // tiles); browsers without it ignore the field.
  const res = await fetch(url, { signal, priority } as RequestInit)
  if (!res.ok) throw new Error(`fetch ${url}: ${res.status}`)
  const buf = await res.arrayBuffer()
  if (buf.byteLength === 0) return null
  return decodeHM3(buf)
}

/** Validate the (already-decompressed) HM3 bytes and return the cell grid. */
export function decodeHM3(buf: ArrayBuffer): DecodedHM3Tile {
  const expected = HEADER_BYTES + TILE_PX * TILE_PX
  if (buf.byteLength !== expected) {
    throw new Error(`HM3 size ${buf.byteLength} != ${expected} (Content-Encoding: br missing?)`)
  }
  const bytes = new Uint8Array(buf)
  const magic = String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3])
  if (magic !== MAGIC) throw new Error(`not HM3 (magic = "${magic}")`)
  if (bytes[4] !== VERSION) throw new Error(`HM3 version ${bytes[4]} != ${VERSION}`)
  return { cells: bytes.subarray(HEADER_BYTES), sourceId: bytes[5] }
}
