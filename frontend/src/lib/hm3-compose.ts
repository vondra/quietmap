/// Colour a decoded HM3 cell grid into a palette-mapped RGBA `ImageData`, and the ancestor preview
/// a child paints while its own tile loads. One grid a tile: loudness does not add over layers, so
/// the map shows the total or one layer, each a tile of its own.

import { HM3_SILENCE } from './hm3-decoder.ts'
import { PALETTE_LUT } from './heatmap-palette.ts'

/** Colour a cell grid into a row-major RGBA `ImageData` via the palette LUT. */
function palette(cells: Uint8Array, width: number, height: number): ImageData {
  const rgba = new Uint8ClampedArray(width * height * 4)
  for (let i = 0; i < cells.length; i++) {
    const lutBase = cells[i] * 4
    const off = i * 4
    rgba[off] = PALETTE_LUT[lutBase]
    rgba[off + 1] = PALETTE_LUT[lutBase + 1]
    rgba[off + 2] = PALETTE_LUT[lutBase + 2]
    rgba[off + 3] = PALETTE_LUT[lutBase + 3]
  }
  return new ImageData(rgba, width, height)
}

/** Palette-map one grid to a `width × height` RGBA `ImageData`. */
export function composeToImageData(cells: Uint8Array, width: number, height: number): ImageData {
  return palette(cells, width, height)
}

// The preview ancestor sits PREVIEW_DELTA zooms above its child: it covers
// 2^Δ × 2^Δ children, one child spans a (512/2^Δ)² sub-block of its grid.
export const PREVIEW_DELTA = 3

/**
 * Upsample one child's sub-block of its z−Δ ancestor grid to a full 512² grid
 * — the preview painted while the child's real tile loads. Bilinear in the
 * half-phon byte space so the preview reads as an out-of-focus map rather than
 * hard blocks; a corner without a level (silence or not assessed) falls back to
 * nearest-neighbour so transparency edges stay crisp instead of bleeding.
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
      if (Math.max(c00, c10, c01, c11) >= HM3_SILENCE) {
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
