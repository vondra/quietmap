/**
 * Loudness → display colour for UI swatches and text labels.
 *
 * Single source of truth is the map's palette (`heatmap-palette.ts`):
 * `soneToColor` is the opaque `#rrggbb` the map paints that loudness in, so the
 * popup's swatch and the map share one scale.
 */
import { paletteHex } from '../lib/heatmap-palette'

/** Loudness (sone) → opaque hex swatch colour. */
export function soneToColor(sone: number): string {
  return paletteHex(sone)
}
