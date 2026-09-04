/**
 * dBA → RGBA palette for noise-level colours (map swatches, pins, popups).
 * See [`STOPS`].
 */

// Beate Tomio (Weninger), the "new color scheme" v5.b for noise maps —
// coloringnoise.com/theoretical_background/new-color-scheme (CC BY-NC-ND 4.0;
// the 2015 EuroNoise paper tested 232 respondents). The ELEVEN class colors
// below are reused unmodified, hex for hex, dB boundary for dB boundary:
// 30-80 dB — below 30 dB is "no color" (renders transparent here, via the
// sub-floor check in paletteColor), and 80 dB is the terminal color, clamped
// flat above it. (2026-07-17 we had scoped the ramp to 35-80 believing the
// original 10-class paper; v5.b adds the 30-35 dark blue-green class — the
// other ten classes matched ours exactly.)
//
// Opacity is OUR rendering adaptation, not part of the scheme: the paper is
// opaque, but we overlay a legible basemap, so alpha rises with dB (the
// paper's own "loud = salient" intent executed via alpha). 2026-07-21 the
// ramp was raised 0.20-0.80 → 0.40-0.90 — the owner found the map washed
// out vs the paper; tune these `op` values only, never the hex/dB classes.
const STOPS: { db: number; rgb: readonly [number, number, number]; op: number }[] = [
  { db: 30, rgb: [0x82, 0xA6, 0xAD], op: 0.40 },
  { db: 35, rgb: [0xA0, 0xBA, 0xBF], op: 0.45 },
  { db: 40, rgb: [0xB8, 0xD6, 0xD1], op: 0.50 },
  { db: 45, rgb: [0xCE, 0xE4, 0xCC], op: 0.55 },
  { db: 50, rgb: [0xE2, 0xF2, 0xBF], op: 0.60 },
  { db: 55, rgb: [0xF3, 0xC6, 0x83], op: 0.65 },
  { db: 60, rgb: [0xE8, 0x7E, 0x4D], op: 0.70 },
  { db: 65, rgb: [0xCD, 0x46, 0x3E], op: 0.75 },
  { db: 70, rgb: [0xA1, 0x1A, 0x4D], op: 0.80 },
  { db: 75, rgb: [0x75, 0x08, 0x5C], op: 0.85 },
  { db: 80, rgb: [0x43, 0x0A, 0x4A], op: 0.90 },
]

// Transparent pixels carry the lightest class color at alpha 0, NOT black:
// MapLibre's bilinear resample blends RGB before applying alpha, so a
// transparent-black neighbor bleeds a grey rim into every color↔floor edge
// (30↔29 dB and NO_DATA edges, seen live 2026-07-21). Bleeding the floor
// color instead fades those edges in the scheme's own hue.
const TRANSPARENT: [number, number, number, number] = [
  STOPS[0].rgb[0], STOPS[0].rgb[1], STOPS[0].rgb[2], 0,
]

/** Interpolate the STOPS ramp at `db` (caller clamps to ≥ STOPS[0].db).
 *  Returns RGB plus the 0–1 opacity so callers take what they need. */
function lerpStop(db: number): { rgb: [number, number, number]; op: number } {
  for (let i = 1; i < STOPS.length; i++) {
    const next = STOPS[i]
    if (db <= next.db) {
      const prev = STOPS[i - 1]
      const t = (db - prev.db) / (next.db - prev.db)
      return {
        rgb: [
          Math.round(prev.rgb[0] + t * (next.rgb[0] - prev.rgb[0])),
          Math.round(prev.rgb[1] + t * (next.rgb[1] - prev.rgb[1])),
          Math.round(prev.rgb[2] + t * (next.rgb[2] - prev.rgb[2])),
        ],
        op: prev.op + t * (next.op - prev.op),
      }
    }
  }
  const last = STOPS[STOPS.length - 1]
  return { rgb: [...last.rgb], op: last.op }
}

/** Interpolated dB → [r, g, b, a] (0-255 each). Sub-floor renders transparent. */
function paletteColor(db: number): [number, number, number, number] {
  if (!Number.isFinite(db)) return TRANSPARENT
  // Sub-floor cells render transparent (floor-hued, see [`TRANSPARENT`]).
  if (db < STOPS[0].db) return TRANSPARENT
  const { rgb, op } = lerpStop(db)
  return [rgb[0], rgb[1], rgb[2], Math.round(255 * op)]
}

/**
 * Solid `#rrggbb` at a dB level. Opacity is dropped (swatches are opaque);
 * sub-floor / non-finite clamp to the lightest stop so a swatch always has
 * a colour.
 */
export function paletteHex(db: number): string {
  return '#' + paletteRgb(db).map((c) => c.toString(16).padStart(2, '0')).join('')
}

/** [`paletteHex`]'s triple for consumers that need numbers (deck.gl colors),
 *  not a CSS string. Same clamping. */
export function paletteRgb(db: number): [number, number, number] {
  const clamped = Number.isFinite(db) ? Math.max(db, STOPS[0].db) : STOPS[0].db
  return lerpStop(clamped).rgb
}

/**
 * Pre-computed `u8 × 0.5 dB → RGBA` lookup. Indexing `LUT[byte * 4 + c]`
 * is one memory hit per pixel; the per-tile decoder loop hits it
 * `TILE_PX² = 65 536` times so the avoided palette interpolation is
 * the bulk of decode cost. Size: 256 × 4 = 1 KB, cache-resident.
 *
 * Byte `255` (no-data sentinel) maps to floor-hued alpha-0 (see [`TRANSPARENT`]).
 */
export const PALETTE_LUT: Uint8ClampedArray = (() => {
  const lut = new Uint8ClampedArray(256 * 4)
  for (let byte = 0; byte < 255; byte++) {
    const db = byte / 2
    const [r, g, b, a] = paletteColor(db)
    lut[byte * 4] = r
    lut[byte * 4 + 1] = g
    lut[byte * 4 + 2] = b
    lut[byte * 4 + 3] = a
  }
  // byte 255 = NO_DATA → transparent, but floor-hued so resampling can't bleed grey.
  lut[255 * 4] = TRANSPARENT[0]
  lut[255 * 4 + 1] = TRANSPARENT[1]
  lut[255 * 4 + 2] = TRANSPARENT[2]
  return lut
})()
