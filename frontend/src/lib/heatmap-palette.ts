/**
 * Each quantity's RGBA palette of the map and its swatches: Nden in sone, Lden in dB. See
 * [`COLOURS`].
 */

import { cellByte, HM3_SILENCE, type Metric } from './hm3-decoder.ts'

// The air-quality index's colours in its order, one a doubling of loudness: clear under 1 sone (a
// quiet room), then yellow from 3 sone, orange 6, red 12, purple 24, and dark purple from 48 (US EPA's
// AQI, its yellow darkened to read on a white map, its top colour a darker purple). Quiet stays clear
// on every basemap, satellite and terrain too: the index's green is the map itself. The darkest marks
// the loudest: in the painted map of 2026-10-25, 48 sone and more is 0.29 % of London's cells (the
// busiest roads' lanes and junctions) and purple from 24 sone its loudest 6.7 % (main roads,
// Heathrow's runway ends). The lightness falls with loudness, so the ramp still reads in grey and to
// deuteranope and protanope eyes. Interpolated in CIELAB by the cell's level (phon, so log2(sone), or
// dB); opacity rises with it.
const COLOURS: { lab: Lab; op: number }[] = [
  { lab: lch(88, 72, 90), op: 0 },
  { lab: lch(88, 72, 90), op: 0.45 },
  { lab: lch(67, 82, 58), op: 0.65 },
  { lab: lch(50, 82, 36), op: 0.8 },
  { lab: lch(41, 57, 325), op: 0.88 },
  { lab: lch(22, 45, 318), op: 0.94 },
]

// Where each colour stands, as a cell byte (twice the level). Lden's are the levels of the EU's noise
// maps and the WHO's guidelines: clear under 35 dB, yellow from 45 (WHO 2018's aircraft guideline,
// its lowest), orange 55 (the lowest band Directive 2002/49 maps report; WHO's road guideline is
// 53), red 65, purple 70 and dark purple from 75 (the Directive's top band).
const STOP_BYTES: Record<Metric, number[]> = {
  nden: [1, 3, 6, 12, 24, 48].map(sone => cellByte('nden', sone)),
  lden: [35, 45, 55, 65, 70, 75].map(db => cellByte('lden', db)),
}

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

/** The ramp at cell byte `byte` of `metric` (clamped into its stops): RGB and opacity. */
function lerpStop(metric: Metric, byte: number): { rgb: Rgb; op: number } {
  const stops = STOP_BYTES[metric]
  const x = Math.min(Math.max(byte, stops[0]), stops[stops.length - 1])
  for (let i = 1; i < stops.length; i++) {
    if (x <= stops[i]) {
      const t = (x - stops[i - 1]) / (stops[i] - stops[i - 1])
      const [prev, next] = [COLOURS[i - 1], COLOURS[i]]
      const lab = prev.lab.map((v, k) => v + t * (next.lab[k] - v)) as unknown as Lab
      return { rgb: rgbOfLab(lab), op: prev.op + t * (next.op - prev.op) }
    }
  }
  const last = COLOURS[COLOURS.length - 1]
  return { rgb: rgbOfLab(last.lab), op: last.op }
}

// Transparent pixels carry the first stop's colour at alpha 0, NOT black: MapLibre's bilinear
// resample blends RGB before applying alpha, so a transparent-black neighbour bleeds a grey rim
// into every colour↔floor edge (seen live 2026-07-21); the floor colour fades those edges in the
// scale's own hue.
const TRANSPARENT: [number, number, number, number] = [...rgbOfLab(COLOURS[0].lab), 0]

/** Solid `#rrggbb` at a level of `metric` (sone for Nden, dB for Lden), for swatches; under the
 *  first stop its colour. */
export function paletteHex(metric: Metric, level: number): string {
  const byte = cellByte(metric, level)
  const { rgb } = lerpStop(metric, Number.isFinite(byte) ? byte : 0)
  return '#' + rgb.map((c) => c.toString(16).padStart(2, '0')).join('')
}

/**
 * Each quantity's pre-computed cell byte → RGBA lookup (`LUT[byte * 4 + c]`), one memory hit per
 * pixel of the 65,536 a tile paints. Under the first stop, silence (254) and no level (255) are
 * transparent.
 */
export const PALETTE_LUTS: Record<Metric, Uint8ClampedArray> = {
  nden: paletteLut('nden'),
  lden: paletteLut('lden'),
}

function paletteLut(metric: Metric): Uint8ClampedArray {
  const lut = new Uint8ClampedArray(256 * 4)
  for (let byte = 0; byte < 256; byte++) {
    let rgba = TRANSPARENT
    if (byte < HM3_SILENCE && byte >= STOP_BYTES[metric][0]) {
      const { rgb, op } = lerpStop(metric, byte)
      rgba = [rgb[0], rgb[1], rgb[2], Math.round(255 * op)]
    }
    lut.set(rgba, byte * 4)
  }
  return lut
}

/** The quiet zones' green (Tailwind's green-600 #16a34a), washed at 43 % over the map wherever the
 *  total is at or under the chosen loudness: green is quiet. The logo's ring around its 0
 *  (`scripts/logo.mjs`) is this green; the sign-in mail's and page's buttons a step darker. */
export const QUIET_RGB = [22, 163, 74] as const
