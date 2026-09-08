// HM3 binary tile decoder (browser-side).
//
// HM3 v3 tiles (the 512@z13 world) are served with `Content-Encoding: br`, so
// the browser has already Brotli-decompressed the response — off the main
// thread, in its network stack — by the time `fetch().arrayBuffer()` resolves.
// This decoder therefore does NO decompression: it validates the 6-byte header
// and returns the raw cells. Mirror of `engine/tile-painter/src/wire_hm3.rs`:
//   0:4  magic "HM3 "   4:1  version = 3   5:1  source_id   6:…  512×512 u8 cells

export const TILE_PX = 512
export const NO_DATA = 255
const HEADER_BYTES = 6
const MAGIC = 'HM3 '
const VERSION = 3

export interface DecodedHM3Tile {
  /** `TILE_PX * TILE_PX` cells, row-major (py * TILE_PX + px). 255 = no data. */
  cells: Uint8Array
  /** Layer discriminator from the header (frontend palette/legend metadata). */
  sourceId: number
}

/**
 * Fetch + decode one HM3 tile. The server sends `Content-Encoding: br`, so the
 * browser decompresses transparently and we only read the header + cells.
 * Returns `null` for "no tile" — a 200 with an empty body (chosen over 204
 * because Cloudflare doesn't cache 204s by default). Throws on a
 * header/version mismatch so the caller can catch and render an empty tile.
 */
export async function fetchAndDecodeHM3(
  url: string,
  signal?: AbortSignal,
  priority?: 'low' | 'high' | 'auto',
): Promise<DecodedHM3Tile | null> {
  // `priority` is a Chromium fetch hint (ancestor previews yield to sharp
  // tiles); browsers without it ignore the field.
  const res = await fetch(url, { signal, priority } as RequestInit)
  if (res.status === 204) return null
  if (!res.ok) throw new Error(`fetch ${url}: ${res.status}`)
  const buf = await res.arrayBuffer()
  if (buf.byteLength === 0) return null
  return decodeHM3(buf)
}

/** Validate the (already-decompressed) HM3 v3 bytes and return the cell grid. */
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
