/// Compose decoded HM3 cell grids into a palette-mapped RGBA `ImageData`.
///
/// Shared by the heatmap `TileLayer` overlay. Noise from independent sources adds
/// in the LINEAR domain, so multiple layers are energy-summed BEFORE palette
/// mapping — alpha-compositing pre-coloured layers would be physically wrong and
/// would break popup↔heatmap parity (the popup energy-sums the same way).

import { NO_DATA } from './hm3-decoder'
import { PALETTE_LUT } from './heatmap-palette'

// Linear energy 10^(byte·0.5/10) per quantised dB byte (0..254), precomputed so
// the per-cell sum is a table read instead of a Math.pow.
const ENERGY = (() => {
  const t = new Float32Array(255)
  for (let b = 0; b < 255; b++) t[b] = 10 ** ((b * 0.5) / 10)
  return t
})()

/**
 * Per-cell linear-energy sum of N `u8 × 0.5 dB` grids of equal length. Each byte
 * decodes to dB via `b/2`; `NO_DATA` contributes zero energy. The sum lands back
 * in the same encoding, clamped at 254 so the palette saturates instead of
 * wrapping.
 */
function sumEnergy(grids: Uint8Array[]): Uint8Array {
  const n = grids[0].length
  const out = new Uint8Array(n).fill(NO_DATA)
  for (let i = 0; i < n; i++) {
    let sumLin = 0
    let anyData = false
    for (const grid of grids) {
      const b = grid[i]
      if (b === NO_DATA) continue
      anyData = true
      sumLin += ENERGY[b]
    }
    if (!anyData) continue
    const q = Math.round(10 * Math.log10(sumLin) * 2)
    out[i] = q < 0 ? NO_DATA : q > 254 ? 254 : q
  }
  return out
}

/** Colour a cell grid into a row-major RGBA `ImageData` via the palette LUT. */
function palette(cells: Uint8Array, width: number, height: number): ImageData {
  const rgba = new Uint8ClampedArray(width * height * 4)
  for (let i = 0; i < cells.length; i++) {
    const byte = cells[i]
    if (byte === NO_DATA) continue // leave fully transparent
    const lutBase = byte * 4
    const off = i * 4
    rgba[off] = PALETTE_LUT[lutBase]
    rgba[off + 1] = PALETTE_LUT[lutBase + 1]
    rgba[off + 2] = PALETTE_LUT[lutBase + 2]
    rgba[off + 3] = PALETTE_LUT[lutBase + 3]
  }
  return new ImageData(rgba, width, height)
}

/**
 * Energy-sum the given source grids (single grid = passthrough) and palette-map
 * the result to one `width × height` RGBA `ImageData`.
 */
export function composeToImageData(grids: Uint8Array[], width: number, height: number): ImageData {
  const combined = grids.length === 1 ? grids[0] : sumEnergy(grids)
  return palette(combined, width, height)
}

// The preview ancestor sits PREVIEW_DELTA zooms above its child: it covers
// 2^Δ × 2^Δ children, one child spans a (512/2^Δ)² sub-block of its grid.
export const PREVIEW_DELTA = 3

/**
 * Upsample one child's sub-block of its z−Δ ancestor grid to a full 512² grid
 * — the preview painted while the child's real layers load. Bilinear in the
 * half-dB byte space so the preview reads as an out-of-focus map rather than
 * hard blocks; any NO_DATA corner falls back to nearest-neighbour so
 * transparency edges stay crisp instead of bleeding.
 */
export function upsampleAncestorBlock(ancestor: Uint8Array, blockX: number, blockY: number): Uint8Array {
  const size = 512
  const scale = 2 ** PREVIEW_DELTA
  const block = size / scale
  const ox = blockX * block
  const oy = blockY * block
  const out = new Uint8Array(size * size)
  for (let py = 0; py < size; py++) {
    const fy = oy + (py + 0.5) / scale - 0.5
    const y0 = Math.min(Math.max(Math.floor(fy), 0), size - 1)
    const y1 = Math.min(y0 + 1, size - 1)
    const ty = Math.min(Math.max(fy - y0, 0), 1)
    const outRow = py * size
    for (let px = 0; px < size; px++) {
      const fx = ox + (px + 0.5) / scale - 0.5
      const x0 = Math.min(Math.max(Math.floor(fx), 0), size - 1)
      const x1 = Math.min(x0 + 1, size - 1)
      const tx = Math.min(Math.max(fx - x0, 0), 1)
      const c00 = ancestor[y0 * size + x0]
      const c10 = ancestor[y0 * size + x1]
      const c01 = ancestor[y1 * size + x0]
      const c11 = ancestor[y1 * size + x1]
      if (c00 === NO_DATA || c10 === NO_DATA || c01 === NO_DATA || c11 === NO_DATA) {
        out[outRow + px] = ty < 0.5 ? (tx < 0.5 ? c00 : c10) : (tx < 0.5 ? c01 : c11)
      } else {
        const top = c00 + (c10 - c00) * tx
        const bottom = c01 + (c11 - c01) * tx
        out[outRow + px] = Math.round(top + (bottom - top) * ty)
      }
    }
  }
  return out
}
