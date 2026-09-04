/**
 * Noise level → display colour for UI swatches and text labels.
 *
 * Single source of truth is the noise palette (`heatmap-palette.ts`):
 * `ldenToColor` is the opaque `#rrggbb` for that Lden, so popup / table
 * swatches share one ramp.
 */
import { paletteHex } from '../lib/heatmap-palette'

/** Lden (dBA) → opaque hex swatch colour. */
export function ldenToColor(lden: number): string {
  return paletteHex(lden)
}
