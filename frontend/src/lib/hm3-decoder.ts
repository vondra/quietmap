// Restore HM3's fixed PNG Paeth prediction after native HTTP Brotli decompression.

export const TILE_PX = 512
/** No level: not assessed (outside painted coverage, missing input). A tile absent from the
 *  archive is all this. */
export const HM3_NO_LEVEL = 255
/** Silence: assessed, under 1/16 sone. */
export const HM3_SILENCE = 254
const HEADER_BYTES = 6
const MAGIC = 'HM3 '
export const HM3_VERSION = 7
// A retired decoder keeps its immutable build when its manifest endpoint disappears.
export const HM3_MANIFEST_URL = `/api/tiles-manifest/${HM3_VERSION}`

/** A cell byte's loudness (sone): silence and no level none. The one place the byte decoding
 *  lives — the renderer's palette and the hover read through it. */
export const HM3_BYTE_SONE = (() => {
  const table = new Float32Array(256)
  for (let byte = 0; byte < HM3_SILENCE; byte++) table[byte] = 2 ** ((byte - 80) / 20)
  return table
})()

export interface DecodedHM3Tile {
  /** `TILE_PX * TILE_PX` cells, row-major (py * TILE_PX + px). */
  cells: Uint8Array
  /** Layer discriminator from the header (frontend palette/legend metadata). */
  sourceId: number
}

/**
 * Fetch + decode one HM3 tile. The server sends `Content-Encoding: br`, so the
 * browser decompresses transparently, then we restore the predicted cells.
 * Returns `null` for a tile absent from the archive — every cell without a
 * level — which the server answers as a 200 with an empty body. Throws on an
 * HTTP error or a header/version mismatch: a failed tile is not an absent one.
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

/** PNG's Paeth predictor, including the specified left/above/corner tie order. */
export function paethPrediction(left: number, above: number, upperLeft: number): number {
  const estimate = left + above - upperLeft
  const leftDistance = Math.abs(estimate - left)
  const aboveDistance = Math.abs(estimate - above)
  const cornerDistance = Math.abs(estimate - upperLeft)
  return leftDistance <= aboveDistance && leftDistance <= cornerDistance ? left
    : aboveDistance <= cornerDistance ? above : upperLeft
}

/** Validate the header, reverse fixed Paeth prediction modulo 256, and return the cell grid. */
export function decodeHM3(buf: ArrayBuffer): DecodedHM3Tile {
  const expected = HEADER_BYTES + TILE_PX * TILE_PX
  if (buf.byteLength !== expected) {
    throw new Error(`HM3 size ${buf.byteLength} != ${expected} (Content-Encoding: br missing?)`)
  }
  const bytes = new Uint8Array(buf)
  const magic = String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3])
  if (magic !== MAGIC) throw new Error(`not HM3 (magic = "${magic}")`)
  if (bytes[4] !== HM3_VERSION) throw new Error(`HM3 version ${bytes[4]} is not ${HM3_VERSION}`)
  if (bytes[5] > 6) throw new Error(`unknown HM3 layer ${bytes[5]}`)
  const cells = bytes.subarray(HEADER_BYTES)
  for (let at = 0; at < cells.length; at++) {
    const left = at % TILE_PX ? cells[at - 1] : 0
    const above = at >= TILE_PX ? cells[at - TILE_PX] : 0
    const upperLeft = at >= TILE_PX && at % TILE_PX ? cells[at - TILE_PX - 1] : 0
    cells[at] += paethPrediction(left, above, upperLeft)
  }
  return { cells, sourceId: bytes[5] }
}
