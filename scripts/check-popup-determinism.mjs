#!/usr/bin/env node
// check-popup-determinism — the popup is the project's acoustic reference,
// so the same click must return bit-identical numbers. This asserts that
// against REAL prepared data, across every layer at once.
//
// MANUAL CHECK — deliberately wired into NO gate. Nothing runs this for you:
// not `scripts/check-fast.sh`, not CI, not a package.json script. It needs a
// real `data/prepared/…/h3r4` (the São Paulo point wants the WORLD set) and a
// built napi cdylib, and check-fast.sh is data-free by design — CI has neither.
// The data-free half of the guard IS gated, as two Rust tests:
// `noise_compute::compute::aircraft_v6::tests::repeated_identical_clicks_are_bit_identical`
// plus `..::airborne::tests::aircraft_detail_ignores_map_iteration_order`.
// Run this one by hand when you touch popup summation, accumulator iteration,
// or any `HashMap`/`HashSet` on the query path — that is what those two tests
// cannot see, because the defect only bites once a click touches thousands of
// roads / flights / microsegments.
//
// Background: `std::collections::HashMap`'s default `RandomState` re-seeds
// on every `HashMap::new()` — i.e. on every query, not merely every process
// — and the kernels sum f64 energies across whole accumulator maps. f64
// addition is not associative, so the walk order was part of the number:
// one Praha click returned three distinct `total_lden` values in four runs
// (2026-08-05). Ten repeats per point is enough to catch a reintroduction;
// the drift showed up within four.
//
// Usage — build the addon first, then point the script at it:
//   cargo build --release --manifest-path engine/source-reader/Cargo.toml --features node
//   node scripts/check-popup-determinism.mjs \
//     --so engine/target/release/libsource_reader.so \
//     [--h3r4 <dir>] [--runs 10]
// `--h3r4` defaults to `data/prepared/2026/h3r4` relative to the addon (the
// same path the server derives in runtime-paths.ts); pass it when your prepared
// set lives elsewhere. Points whose cells are missing from that set still run —
// they simply exercise less.
//
// Exits non-zero on the first point whose output is not bit-stable.

import { createRequire } from 'node:module'
import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const require_ = createRequire(import.meta.url)
const argv = process.argv.slice(2)
const arg = (k, d) => {
  const i = argv.indexOf(k)
  return i >= 0 ? argv[i + 1] : d
}

const soArg = arg('--so')
if (!soArg) {
  console.error('usage: check-popup-determinism.mjs --so <libsource_reader.so> [--h3r4 dir] [--runs 10]')
  process.exit(2)
}
const so = resolve(soArg)
if (!existsSync(so)) {
  console.error(`addon not found: ${so}`)
  process.exit(2)
}
const runs = Number(arg('--runs', '10'))
// Same default the server derives (runtime-paths.ts H3R4_DIR).
const h3r4Dir = resolve(arg('--h3r4', resolve(so, '../../../../../data/prepared/2026/h3r4')))
if (!existsSync(h3r4Dir)) {
  console.error(`h3r4 dir not found: ${h3r4Dir}`)
  process.exit(2)
}

// The five reference receivers the popup is tuned and reviewed against:
// a small town under an approach corridor, a behind-the-wall point, dense
// Praha, open countryside with no buildings, and the densest obstacle cell
// measured worldwide (São Paulo, 18.4 M edges).
const POINTS = [
  { key: 'dobris', label: 'Dobříš', lat: 49.781, lng: 14.214 },
  { key: 'voznice', label: 'Voznice behind-wall', lat: 49.8135, lng: 14.2155 },
  { key: 'praha', label: 'Praha dense', lat: 50.083, lng: 14.43 },
  { key: 'rural', label: 'Rural (Brdy, no buildings)', lat: 49.7005, lng: 13.9205 },
  { key: 'saopaulo', label: 'São Paulo (densest cell)', lat: -23.5505, lng: -46.6333 },
]

// Node only dlopens files named `*.node`; mirror what the server's
// source-reader-addon.ts does.
const addonDir = join(tmpdir(), 'popup-determinism-addons')
mkdirSync(addonDir, { recursive: true })
const variantHash = createHash('sha1').update(`${so}:${process.pid}`).digest('hex').slice(0, 12)
const addonPath = join(addonDir, `libsource_reader.${variantHash}.node`)
copyFileSync(so, addonPath)
const mod = require_(addonPath)
console.log(`# ${mod.sourceInit(h3r4Dir)}`)
console.log(`# so=${so}  runs=${runs}`)

const f64Bits = (v) => {
  if (typeof v !== 'number') return String(v)
  const b = Buffer.alloc(8)
  b.writeDoubleLE(v)
  return b.toString('hex')
}

let failed = 0
console.log('point\ttotal_lden\tbits\tdistinct_lden\tdistinct_body')
for (const point of POINTS) {
  const ldenBits = new Set()
  const bodyHashes = new Set()
  let lastLden = null
  let firstBody = null
  let firstDiffAt = null
  for (let i = 0; i < runs; i += 1) {
    const raw = mod.queryNoiseAtPoint(point.lat, point.lng)
    const parsed = typeof raw === 'string' ? JSON.parse(raw) : raw
    lastLden = parsed.total_lden ?? parsed.lden ?? parsed.total?.lden_db ?? null
    ldenBits.add(f64Bits(lastLden))
    // `timings` is wall-clock instrumentation and is legitimately
    // different every run; everything else must be reproducible.
    delete parsed.timings
    const body = JSON.stringify(parsed)
    if (firstBody === null) firstBody = body
    else if (body !== firstBody && firstDiffAt === null) {
      firstDiffAt = [...body].findIndex((c, k) => c !== firstBody[k])
    }
    bodyHashes.add(createHash('sha256').update(body).digest('hex'))
  }
  const ok = ldenBits.size === 1 && bodyHashes.size === 1
  if (!ok) failed += 1
  console.log(
    `${point.key}\t${lastLden === null ? '-' : lastLden.toPrecision(17)}\t${[...ldenBits][0]}\t${
      ldenBits.size
    }\t${bodyHashes.size}${ok ? '' : '\tNOT REPRODUCIBLE'}`,
  )
  if (!ok && firstDiffAt !== null) {
    const from = Math.max(0, firstDiffAt - 90)
    console.log(`    first divergence at byte ${firstDiffAt}: …${firstBody.slice(from, firstDiffAt + 40)}`)
  }
}

if (failed) {
  console.error(
    `\nFAIL: ${failed}/${POINTS.length} reference points are not bit-reproducible.\n` +
      'Something sums f64 (or breaks a tie) over a HashMap walk — see ' +
      'noise_compute::compute::key_sorted.',
  )
  process.exit(1)
}
console.log(`\nOK: all ${POINTS.length} reference points bit-reproducible over ${runs} repeats each.`)
