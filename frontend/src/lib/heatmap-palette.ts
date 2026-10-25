/**
 * Loudness (sone) → RGBA palette of the map and its swatches. See [`STOPS`].
 */

import { HM3_SILENCE, HM3_BYTE_SONE } from './hm3-decoder.ts'

// The air-quality index's colours in its order, one a doubling of loudness: clear under 1 sone (a
// quiet room), then yellow from 3 sone, orange 6, red 12, purple 24, and dark purple from 48 (US EPA's
// AQI, its yellow darkened to read on a white map, its top colour a darker purple). Quiet stays clear
// on every basemap, satellite and terrain too: the index's green is the map itself. The darkest marks
// the loudest: in the preview of 2026-10-25, 48 sone and more is 0.27 % of Prague's painted cells
// (the trunk road's lanes) and purple from 24 sone its loudest 3.6 % (main roads, the D1, Heathrow's
// runway ends). The lightness falls with loudness, so the ramp still reads in grey and to deuteranope
// and protanope eyes. Interpolated in CIELAB by log2(sone); opacity rises with loudness.
const STOPS: { sone: number; lab: Lab; op: number }[] = [
  { sone: 1, lab: lch(88, 72, 90), op: 0 },
  { sone: 3, lab: lch(88, 72, 90), op: 0.45 },
  { sone: 6, lab: lch(67, 82, 58), op: 0.65 },
  { sone: 12, lab: lch(50, 82, 36), op: 0.8 },
  { sone: 24, lab: lch(41, 57, 325), op: 0.88 },
  { sone: 48, lab: lch(22, 45, 318), op: 0.94 },
]

type Lab = readonly [number, number, number]
type Rgb = [number, number, number]

function lch(lightness: number, chroma: number, hueDeg: number): Lab {
  const hue = (hueDeg * Math.PI) / 180
  return [lightness, chroma * Math.cos(hue), chroma * Math.sin(hue)]
}

/** CIELAB (D65) → sRGB 0-255. */
function rgbOfLab([lightness, a, b]: Lab): Rgb {
  const fy = (lightness + 16) / 116
  const inverse = (t: number) => (t > 6 / 29 ? t ** 3 : 3 * (6 / 29) ** 2 * (t - 4 / 29))
  const [x, y, z] = [0.95047 * inverse(fy + a / 500), inverse(fy), 1.08883 * inverse(fy - b / 200)]
  const linear = [
    3.2406 * x - 1.5372 * y - 0.4986 * z,
    -0.9689 * x + 1.8758 * y + 0.0415 * z,
    0.0557 * x - 0.204 * y + 1.057 * z,
  ]
  const encode = (c: number) => {
    const clamped = Math.min(Math.max(c, 0), 1)
    return clamped <= 0.0031308 ? 12.92 * clamped : 1.055 * clamped ** (1 / 2.4) - 0.055
  }
  return linear.map((c) => Math.round(255 * encode(c))) as Rgb
}

/** The ramp at `sone` (clamped into the stops): RGB and opacity. */
function lerpStop(sone: number): { rgb: Rgb; op: number } {
  const x = Math.log2(Math.min(Math.max(sone, STOPS[0].sone), STOPS[STOPS.length - 1].sone))
  for (let i = 1; i < STOPS.length; i++) {
    const [prev, next] = [STOPS[i - 1], STOPS[i]]
    if (x <= Math.log2(next.sone)) {
      const t = (x - Math.log2(prev.sone)) / (Math.log2(next.sone) - Math.log2(prev.sone))
      const lab = prev.lab.map((v, k) => v + t * (next.lab[k] - v)) as unknown as Lab
      return { rgb: rgbOfLab(lab), op: prev.op + t * (next.op - prev.op) }
    }
  }
  const last = STOPS[STOPS.length - 1]
  return { rgb: rgbOfLab(last.lab), op: last.op }
}

// Transparent pixels carry the first stop's colour at alpha 0, NOT black: MapLibre's bilinear
// resample blends RGB before applying alpha, so a transparent-black neighbour bleeds a grey rim
// into every colour↔floor edge (seen live 2026-07-21); the floor colour fades those edges in the
// scale's own hue.
const TRANSPARENT: [number, number, number, number] = [...rgbOfLab(STOPS[0].lab), 0]

/** Solid `#rrggbb` at a loudness (sone), for swatches; under 1 sone the first stop's colour. */
export function paletteHex(sone: number): string {
  return '#' + paletteRgb(sone).map((c) => c.toString(16).padStart(2, '0')).join('')
}

/** [`paletteHex`]'s triple for consumers that need numbers (deck.gl colors). */
export function paletteRgb(sone: number): [number, number, number] {
  return lerpStop(Number.isFinite(sone) ? sone : STOPS[0].sone).rgb
}

/**
 * Pre-computed cell byte → RGBA lookup (`LUT[byte * 4 + c]`), one memory hit per pixel of the
 * 65,536 a tile paints. Under 1 sone, silence (254) and no level (255) are transparent.
 */
export const PALETTE_LUT: Uint8ClampedArray = (() => {
  const lut = new Uint8ClampedArray(256 * 4)
  for (let byte = 0; byte < 256; byte++) {
    const sone = HM3_BYTE_SONE[byte]
    let rgba = TRANSPARENT
    if (byte < HM3_SILENCE && sone >= STOPS[0].sone) {
      const { rgb, op } = lerpStop(sone)
      rgba = [rgb[0], rgb[1], rgb[2], Math.round(255 * op)]
    }
    lut.set(rgba, byte * 4)
  }
  return lut
})()
