// Several noise layers' level at a cell from each layer's own. Lden adds as energy, exactly.
// Loudness does not add over layers, so two to five layers (no tile of their own) are estimated as
// N = (Σ N_i^p)^(1/p) sone, p rising with the loudest layer's level as fitted against the painted
// total of all six (sound adds nearly as loudness near the threshold, nearly as energy when loud).
// Against the painted total the estimate is within 1 phon at 90 % of cells with two layers or more
// and within 2 phon at 98 % (1,536 painted tiles of zoom 12, October 2026). Every layer on reads
// the total, one alone its own.

import { fetchAndDecodeHM3, HM3_NO_LEVEL, HM3_SILENCE, type Metric } from './hm3-decoder.ts'
import { LAYER_URL_SEPARATOR } from './tile-urls.ts'

/** p at the loudest layer's loudness level (phon), linear between the fitted bands' centres. */
const P_NODES: readonly [number, number][] = [
  [22.5, 1.023], [27.5, 1.116], [32.5, 1.188], [37.5, 1.263], [42.5, 1.347], [47.5, 1.453],
  [52.5, 1.57], [57.5, 1.723], [62.5, 1.904], [67.5, 2.123], [72.5, 2.388], [77.5, 2.619],
  [82.5, 2.811], [87.5, 3.13],
]

function pAt(phon: number): number {
  if (phon <= P_NODES[0][0]) return P_NODES[0][1]
  for (let k = 1; k < P_NODES.length; k++) {
    const [phon1, p1] = P_NODES[k]
    if (phon <= phon1) {
      const [phon0, p0] = P_NODES[k - 1]
      return p0 + ((phon - phon0) / (phon1 - phon0)) * (p1 - p0)
    }
  }
  return P_NODES[P_NODES.length - 1][1]
}

// Per loudest cell byte (2 x phon), its p and each byte's loudness to that power: a cell's
// combination is then a few lookups and one logarithm.
const LEVELS = HM3_SILENCE
let powers: Float64Array | null = null
const pOfLoudest = Float64Array.from({ length: LEVELS }, (_, byte) => pAt(byte / 2))

function powerTable(): Float64Array {
  if (powers === null) {
    powers = new Float64Array(LEVELS * LEVELS)
    for (let loudest = 0; loudest < LEVELS; loudest++) {
      for (let byte = 0; byte < LEVELS; byte++) {
        powers[loudest * LEVELS + byte] = 2 ** (((byte - 80) / 20) * pOfLoudest[loudest])
      }
    }
  }
  return powers
}

// A half-decibel cell byte's energy, 10^(byte/20).
const ENERGY = Float64Array.from({ length: LEVELS }, (_, byte) => 10 ** (byte / 20))

/** The cells of several layers' grids of `metric` combined: no level where any layer has none,
 *  silence where every layer is silent or the combination stays under the quantity's floor. */
export function combineLayerCells(grids: readonly Uint8Array[], metric: Metric): Uint8Array {
  return metric === 'lden' ? combineEnergies(grids) : combineLoudness(grids)
}

function combineEnergies(grids: readonly Uint8Array[]): Uint8Array {
  const out = new Uint8Array(grids[0].length)
  for (let at = 0; at < out.length; at++) {
    let sum = 0
    for (const grid of grids) {
      const byte = grid[at]
      if (byte === HM3_NO_LEVEL) {
        sum = -1
        break
      }
      if (byte !== HM3_SILENCE) sum += ENERGY[byte]
    }
    out[at] = sum < 0 ? HM3_NO_LEVEL
      : sum < 1 ? HM3_SILENCE
        : Math.min(Math.round(20 * Math.log10(sum)), HM3_SILENCE - 1)
  }
  return out
}

function combineLoudness(grids: readonly Uint8Array[]): Uint8Array {
  const table = powerTable()
  const out = new Uint8Array(grids[0].length)
  for (let at = 0; at < out.length; at++) {
    let loudest = -1
    let missing = false
    for (const grid of grids) {
      const byte = grid[at]
      if (byte === HM3_NO_LEVEL) missing = true
      else if (byte !== HM3_SILENCE && byte > loudest) loudest = byte
    }
    if (missing) {
      out[at] = HM3_NO_LEVEL
      continue
    }
    if (loudest < 0) {
      out[at] = HM3_SILENCE
      continue
    }
    let sum = 0
    const row = loudest * LEVELS
    for (const grid of grids) {
      const byte = grid[at]
      if (byte !== HM3_SILENCE) sum += table[row + byte]
    }
    const byte = Math.round(80 + (20 / pOfLoudest[loudest]) * Math.log2(sum))
    out[at] = byte < 0 ? HM3_SILENCE : Math.min(byte, HM3_SILENCE - 1)
  }
  return out
}

/** A source's cells at one tile and the quantity they hold. */
export interface SourceTile {
  cells: Uint8Array
  metric: Metric
}

/** One tile of a source from its address (`tileUrl`): a combination's layers fetched at once and
 *  combined, a tile absent from a layer's archive read as without levels; `null` when every one
 *  is absent. */
export async function fetchSourceTile(
  url: string,
  signal?: AbortSignal,
  priority?: 'low' | 'high' | 'auto',
): Promise<SourceTile | null> {
  if (!url.includes(LAYER_URL_SEPARATOR)) return fetchAndDecodeHM3(url, signal, priority)
  const tiles = await Promise.all(
    url.split(LAYER_URL_SEPARATOR).map(part => fetchAndDecodeHM3(part, signal, priority)),
  )
  const present = tiles.filter(tile => tile !== null)
  if (present.length === 0) return null
  const metric = present[0].metric
  if (present.some(tile => tile.metric !== metric)) throw new Error(`${url}: layers of two quantities`)
  const none = new Uint8Array(present[0].cells.length).fill(HM3_NO_LEVEL)
  return { cells: combineLayerCells(tiles.map(tile => tile?.cells ?? none), metric), metric }
}
