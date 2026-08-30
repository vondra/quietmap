/**
 * Campaign 2026-08-built-up-vector, stage 2: what the swap actually changes.
 *
 * Reads the TSV from `sample-built-up.ts` and prints
 *   2. a threshold grid search for each candidate vector statistic,
 *   3. the old→new confusion overall and per country,
 *   4. the same confusion restricted to segments that actually CONSULT
 *      `built_up` (untagged class 2/3/4/9), plus the km/h the engine would
 *      resolve before and after — a flip on a road that never asks is free,
 *   5. where the flips sit relative to the threshold.
 *
 * The country speed table is parsed from the engine's generated source so this
 * analysis cannot drift from what `resolve_speed_default` really does.
 *
 * Usage:
 *   node_modules/.bin/tsx research/2026-08-built-up-vector/analyze-built-up.ts \
 *     /tmp/built-up-sample.tsv
 */

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const SAMPLE = process.argv[2] ?? '/tmp/built-up-sample.tsv'
const SPEED_TABLE_RS = resolve(
  import.meta.dirname,
  '../../engine/noise-compute/src/country_speed_defaults_generated.rs',
)

/** `normalize/road.rs::default_road_speed` — the table used when the country
 *  arm declines (unknown built_up, no country row, or a 0 bucket). */
const LEGACY_SPEED = [100, 70, 50, 50, 50, 30, 20, 20, 20, 50, 60, 50, 50]

const countrySpeeds = new Map<string, [number, number, number, number]>()
for (const m of readFileSync(SPEED_TABLE_RS, 'utf8').matchAll(
  /\(b"([A-Z]{2})",\s*\[(\d+),\s*(\d+),\s*(\d+),\s*(\d+)\]\)/g,
)) {
  countrySpeeds.set(m[1], [Number(m[2]), Number(m[3]), Number(m[4]), Number(m[5])])
}

/** `defaults.rs::resolve_speed_default` + the caller's legacy fallback. */
function resolvedSpeed(iso: string, roadClass: number, builtUp: number): number {
  const row = countrySpeeds.get(iso)
  let v = 0
  if (row) {
    const [urban, rural, motorway, motorroad] = row
    if (roadClass === 0) v = motorway
    else if (roadClass === 1) v = motorroad > 0 ? motorroad : rural
    else if ([2, 3, 4, 9].includes(roadClass)) v = builtUp === 2 ? urban : builtUp === 1 ? rural : 0
  }
  return v > 0 ? v : LEGACY_SPEED[Math.min(roadClass, LEGACY_SPEED.length - 1)]
}

interface Row {
  country: string
  roadClass: number
  speedLimit: number
  speedTaper: number
  lat: number
  oldStored: number
  count: number
  areaM2: number
  px: number
}

const lines = readFileSync(SAMPLE, 'utf8').trim().split('\n')
const header = lines[0].split('\t')
const col = (name: string) => header.indexOf(name)
const rows: Row[] = lines.slice(1).map((l) => {
  const f = l.split('\t')
  return {
    country: f[col('country')],
    roadClass: Number(f[col('class')]),
    speedLimit: Number(f[col('speed_limit')]),
    speedTaper: Number(f[col('speed_taper')]),
    lat: Number(f[col('lat')]),
    oldStored: Number(f[col('old_stored')]),
    count: Number(f[col('count')]),
    areaM2: Number(f[col('area_m2')]),
    px: Number(f[col('px')]),
  }
})

const countries = [...new Set(rows.map((r) => r.country))].sort()
console.log(`sample: ${rows.length} segments, ${countries.length} countries (${countries.join(', ')})`)

// ── 2. Which vector statistic reproduces the raster's answer? ──────────────
type Stat = (r: Row) => number
const covered = rows.filter((r) => r.oldStored !== 0 && r.px >= 0)
function gridSearch(name: string, stat: Stat, thresholds: number[]) {
  let best = { th: 0, agree: -1 }
  const table: string[] = []
  for (const th of thresholds) {
    let agree = 0
    for (const r of covered) {
      const nw = stat(r) >= th ? 2 : 1
      if (nw === r.oldStored) agree++
    }
    table.push(`${th}:${((100 * agree) / covered.length).toFixed(2)}`)
    if (agree > best.agree) best = { th, agree }
  }
  console.log(`  ${name.padEnd(22)} best th=${best.th} → ${((100 * best.agree) / covered.length).toFixed(2)} %`)
  console.log(`      ${table.join('  ')}`)
  return best
}
console.log(`\n=== 2. threshold grid search (${covered.length} covered segments) ===`)
gridSearch('footprint count', (r) => r.count, [5, 10, 15, 20, 25, 30, 40, 50, 60, 80, 100])
gridSearch('footprint area m2', (r) => r.areaM2, [1000, 2000, 3000, 4000, 5000, 6000, 8000, 10000, 15000])
const bestPx = gridSearch(
  'estimated built px',
  (r) => r.px,
  [2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 20, 24],
)

// ── 3. Old → new confusion, at the shipped threshold ───────────────────────
const SHIPPED_THRESHOLD = Number(process.argv[3] ?? 8)
const classify = (r: Row) => (r.px < 0 ? 0 : r.px >= SHIPPED_THRESHOLD ? 2 : 1)
const NAME = ['UNKNOWN', 'RURAL', 'URBAN']

function confusion(subset: Row[]) {
  const m = [
    [0, 0, 0],
    [0, 0, 0],
    [0, 0, 0],
  ]
  for (const r of subset) m[r.oldStored][classify(r)]++
  return m
}
function printConfusion(label: string, subset: Row[]) {
  if (subset.length === 0) return
  const m = confusion(subset)
  const pct = (n: number) => `${((100 * n) / subset.length).toFixed(2)} %`
  const flips = m[1][2] + m[2][1] + m[1][0] + m[2][0] + m[0][1] + m[0][2]
  console.log(
    `  ${label.padEnd(10)} n=${String(subset.length).padStart(6)}  ` +
      `stay URBAN ${String(m[2][2]).padStart(6)}  stay RURAL ${String(m[1][1]).padStart(6)}  ` +
      `U→R ${String(m[2][1]).padStart(5)}  R→U ${String(m[1][2]).padStart(5)}  ` +
      `→UNK ${String(m[1][0] + m[2][0]).padStart(4)}  UNK→ ${String(m[0][1] + m[0][2]).padStart(4)}  ` +
      `stay UNK ${String(m[0][0]).padStart(4)}  | changed ${String(flips).padStart(5)} (${pct(flips)})`,
  )
}

console.log(`\n=== 3. old → new confusion at threshold ${SHIPPED_THRESHOLD} ===`)
printConfusion('ALL', rows)
for (const c of countries) printConfusion(c, rows.filter((r) => r.country === c))

// ── 4. Do the flips reach the engine? ──────────────────────────────────────
// A segment consults built_up only when it is class 2/3/4/9 AND has no OSM
// maxspeed AND no taper (normalize/road.rs), and the answer only differs when
// the country's table separates urban from rural.
const consults = (r: Row) =>
  [2, 3, 4, 9].includes(r.roadClass) && r.speedLimit === 0 && r.speedTaper === 0
console.log(`\n=== 4. flips restricted to segments that CONSULT built_up ===`)
printConfusion('ALL', rows.filter(consults))
for (const c of countries) printConfusion(c, rows.filter((r) => r.country === c && consults(r)))

const changedSpeed = new Map<string, number>()
let consulting = 0
let speedChanged = 0
for (const r of rows) {
  if (!consults(r)) continue
  consulting++
  const before = resolvedSpeed(r.country, r.roadClass, r.oldStored)
  const after = resolvedSpeed(r.country, r.roadClass, classify(r))
  if (before !== after) {
    speedChanged++
    const k = `${r.country} class${r.roadClass} ${before}→${after}`
    changedSpeed.set(k, (changedSpeed.get(k) ?? 0) + 1)
  }
}
console.log(
  `\n  segments consulting built_up: ${consulting} / ${rows.length} (${((100 * consulting) / rows.length).toFixed(2)} %)`,
)
console.log(
  `  of those, resolved default speed CHANGES for: ${speedChanged} (${consulting ? ((100 * speedChanged) / consulting).toFixed(2) : '0'} % of consulting, ` +
    `${((100 * speedChanged) / rows.length).toFixed(3)} % of all sampled segments)`,
)
for (const [k, v] of [...changedSpeed].sort((a, b) => b[1] - a[1])) console.log(`    ${k}: ${v}`)

const noTable = countries.filter((c) => !countrySpeeds.has(c))
if (noTable.length) {
  console.log(`\n  countries with NO legal-speed row (built_up can never matter there): ${noTable.join(', ')}`)
}

// ── 5. What kind of segment flips? ─────────────────────────────────────────
// If the two probes disagreed about the DATA we would see flips spread across
// the whole density range; if they agree and only the cut differs, the flips
// crowd the threshold. Tile-edge segments are called out separately: the
// raster CLAMPED its window at a 1° boundary and the vector probe does not.
{
  const flipped = rows.filter((r) => r.px >= 0 && r.oldStored !== 0 && classify(r) !== r.oldStored)
  const near = (lo: number, hi: number) => flipped.filter((r) => r.px >= lo && r.px < hi).length
  const HALF = 8.5 / 3600
  const atEdge = (r: Row) => Math.abs(r.lat - Math.round(r.lat)) < HALF
  console.log(`\n=== 5. where the ${flipped.length} flips sit on the density scale ===`)
  console.log(
    `  px < 4: ${near(0, 4)}   4–6: ${near(4, 6)}   6–8: ${near(6, 8)}   ` +
      `8–10: ${near(8, 10)}   10–12: ${near(10, 12)}   12–16: ${near(12, 16)}   ≥16: ${flipped.filter((r) => r.px >= 16).length}`,
  )
  const half = flipped.filter((r) => r.px >= 4 && r.px < 16).length
  console.log(`  within a factor 2 of the threshold: ${half} (${((100 * half) / flipped.length).toFixed(1)} %)`)
  const edgeAll = rows.filter(atEdge).length
  const edgeFlipped = flipped.filter(atEdge).length
  console.log(
    `  within half a window of a 1° latitude line (where the raster clamped): ` +
      `${edgeFlipped}/${edgeAll} such segments flipped, ${((100 * edgeFlipped) / Math.max(flipped.length, 1)).toFixed(1)} % of all flips`,
  )
}
