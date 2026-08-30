/**
 * Campaign 2026-08-built-up-vector, stage 1: collect the evidence.
 *
 * Walks a stratified sample of road segments (per country, spread over many
 * H3R4 cells so one metro cannot dominate) and writes one TSV row per segment
 * with everything the flip analysis needs:
 *
 *   country  cell  class  speed_limit  speed_taper  lat  lon
 *   old_stored   built_up as production wrote it from the building raster
 *   count        footprint centroids in the sampling window (vector store)
 *   area_m2      their footprint area, holes subtracted
 *   px           area / raster pixel area at this latitude
 *
 * Stage 2 (`analyze-built-up.ts`) does the threshold grid search and the
 * confusion matrix off this file — so a re-analysis costs no I/O.
 *
 * `old_stored` is trusted as the raster's answer: on 2026-08-30, before the
 * raster reader was deleted, re-running it over /data1/prepared/rasters/building
 * reproduced the stored column for 27 951 of 27 951 sampled segments.
 *
 * Usage:
 *   node_modules/.bin/tsx research/2026-08-built-up-vector/sample-built-up.ts \
 *     --out /tmp/built-up-sample.tsv --per-country 6000
 */

import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import { BuildingFootprintSampler } from '../../pipeline/lib/building-footprints.js'

const H3R4_DIR = resolve(import.meta.dirname, '../../data/prepared/2026/h3r4')

/** Rough land bboxes, only to pick candidate cells fast — every sampled ROW is
 *  then confirmed against its own baked `country_iso`, so a bbox that spills
 *  over a border costs nothing. */
const COUNTRIES: Record<string, [number, number, number, number]> = {
  CZ: [48.5, 12.0, 51.1, 18.9],
  DE: [47.2, 5.8, 55.1, 15.1],
  GB: [49.9, -8.2, 58.7, 1.8],
  FR: [42.3, -4.8, 51.1, 8.3],
  US: [24.5, -125.0, 49.0, -66.9],
  BR: [-33.8, -74.0, -5.0, -34.8],
}

const arg = (name: string, dflt: string): string => {
  const i = process.argv.indexOf(`--${name}`)
  return i >= 0 ? process.argv[i + 1] : dflt
}
const OUT = arg('out', '/tmp/built-up-sample.tsv')
const PER_COUNTRY = Number(arg('per-country', '6000'))
const CELLS_PER_COUNTRY = Number(arg('cells', '60'))

/** Deterministic PRNG so a re-run samples the same segments (mulberry32). */
function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function shuffle<T>(items: T[], rand: () => number): T[] {
  const out = items.slice()
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1))
    ;[out[i], out[j]] = [out[j], out[i]]
  }
  return out
}

function isoOf(packed: number): string {
  return packed === 0 ? '??' : String.fromCharCode(packed & 0xff, packed >> 8)
}

async function main() {
  const rand = rng(20260830)
  const allCells = readdirSync(H3R4_DIR).filter((d) => !d.startsWith('.'))
  console.log(`h3r4 cells: ${allCells.length}`)

  const byCountry = new Map<string, string[]>()
  for (const iso of Object.keys(COUNTRIES)) byCountry.set(iso, [])
  for (const cell of allCells) {
    let lat: number
    let lon: number
    try {
      ;[lat, lon] = cellToLatLng(cell)
    } catch {
      continue
    }
    for (const [iso, [s, w, n, e]] of Object.entries(COUNTRIES)) {
      if (lat >= s && lat <= n && lon >= w && lon <= e) byCountry.get(iso)!.push(cell)
    }
  }
  for (const [iso, cells] of byCountry) console.log(`  ${iso}: ${cells.length} candidate cells`)

  const vector = new BuildingFootprintSampler(undefined, undefined, 4)

  const rows: string[] = [
    ['country', 'cell', 'class', 'speed_limit', 'speed_taper', 'lat', 'lon', 'old_stored', 'count', 'area_m2', 'px'].join('\t'),
  ]
  for (const [iso, cells] of byCountry) {
    const picked = shuffle(cells, rand).slice(0, CELLS_PER_COUNTRY)
    const perCell = Math.ceil(PER_COUNTRY / picked.length)
    let taken = 0
    let cellsUsed = 0
    for (const cell of picked) {
      if (taken >= PER_COUNTRY) break
      const path = resolve(H3R4_DIR, cell, 'roads.arrow')
      if (!existsSync(path)) continue
      const table = tableFromIPC(readFileSync(path))
      const n = table.numRows
      if (n === 0) continue
      const sLat = table.getChild('start_lat')!
      const sLon = table.getChild('start_lon')!
      const eLat = table.getChild('end_lat')!
      const eLon = table.getChild('end_lon')!
      const cls = table.getChild('road_class')!
      const spd = table.getChild('speed_limit')!
      const taper = table.getChild('speed_taper')
      const builtUp = table.getChild('built_up')
      const country = table.getChild('country_iso')!
      if (!builtUp) continue // pre-flag hex — nothing to compare against

      const idx = shuffle(
        Array.from({ length: n }, (_, i) => i),
        rand,
      ).slice(0, perCell)
      let usedHere = 0
      for (const i of idx) {
        if (isoOf(country.get(i) as number) !== iso) continue
        const midLat = ((sLat.get(i) as number) + (eLat.get(i) as number)) / 2
        const midLon = ((sLon.get(i) as number) + (eLon.get(i) as number)) / 2
        const w = vector.windowFootprints(midLat, midLon)
        const px = vector.estimatedBuiltPixels(midLat, midLon)
        rows.push(
          [
            iso,
            cell,
            cls.get(i) as number,
            spd.get(i) as number,
            (taper?.get(i) as number) ?? 0,
            midLat.toFixed(6),
            midLon.toFixed(6),
            builtUp.get(i) as number,
            w === null ? -1 : w.count,
            w === null ? -1 : w.areaM2.toFixed(1),
            px === null ? -1 : px.toFixed(3),
          ].join('\t'),
        )
        taken++
        usedHere++
        if (taken >= PER_COUNTRY) break
      }
      if (usedHere > 0) cellsUsed++
    }
    console.log(`  ${iso}: ${taken} segments from ${cellsUsed} cells`)
  }

  writeFileSync(OUT, rows.join('\n') + '\n')
  console.log(`\nwrote ${rows.length - 1} rows to ${OUT}`)
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
