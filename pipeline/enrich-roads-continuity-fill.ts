//! Road AADT continuity fill by flow-redistribution.
//!
//! Where a MEASURED AADT covers only part of a road network, the unmeasured
//! segments fall to the engine's class default — often a physically-impossible
//! jump, because cars don't vanish at a junction, they REDISTRIBUTE. Live case:
//! ref 0033 "K Šeberovu" measured at 15620; at the junction with the small road
//! 10114 the continuation dropped to the tertiary default 800 — i.e. ~14000 cars
//! "disappeared" with only a minor side road to absorb them.
//!
//! Instead of copying-and-stopping, this propagates a measured value as a vehicle
//! VECTOR and at each junction redistributes it (Ondra's rule, 2026-06-24):
//!   • a continuation of the SAME road number keeps the flow MINUS what the side
//!     branches draw off (each branch draws its measured value, or its class
//!     default if unmeasured);
//!   • if the road splits into DIFFERENT numbers, the flow divides in proportion
//!     to the branches' class defaults.
//! The value never INCREASES away from the anchor (so it can't inflate a distant
//! network); conservation is attempted at each node, clamped to ≥0 when the side
//! branches' estimated draw exceeds the inflow. NOT a traffic-assignment solver —
//! just enough conservation to replace a worse class-default jump.
//!
//! The stop threshold uses the WORLD class default; the engine itself applies a
//! city/country/continent cascade, so in a few up-scaled metros (Bangkok, BR) a
//! fill could in theory land below the engine's local default. Acceptable: those
//! metros carry their own city census, and this is a heuristic the engine
//! down-weights via access_factor anyway (/gg Codex, deferred).
//!
//! Stamped heuristic (id 12) → any real measurement still wins; engine re-applies
//! access_factor. Runs AFTER measured national/city enrichers (needs their
//! anchors), independent of service-tree (local classes 5-9). Per-hex,
//! self-contained → SHARD=i/n parallelizes it like service-tree.
//!
//! Usage:
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-continuity-fill.ts
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-continuity-fill.ts --bbox 49.7,13.9,50.4,15.0
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-continuity-fill.ts --prefix 841e355
//!   SHARD=0/96 DATA_YEAR=2026 node_modules/.bin/tsx pipeline/enrich-roads-continuity-fill.ts

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { SOURCE_ID_ROAD_CONTINUITY_HEURISTIC } from './lib/source-ids.generated.js'
import { isMeasured } from './lib/sources.js'
import { classDefaultTotal } from './lib/road-class-defaults.js'
import { writeRoadAadt, iterateCountryHexes } from './lib/roads-arrow.js'
import { nodeKey } from './lib/spatial.js'
import { H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_ROAD_CONTINUITY_HEURISTIC

const PREFIX = process.argv.includes('--prefix') ? process.argv[process.argv.indexOf('--prefix') + 1] : ''
const bboxArg = process.argv.includes('--bbox') ? process.argv[process.argv.indexOf('--bbox') + 1] : ''
const BBOX = bboxArg ? (bboxArg.split(',').map(Number) as [number, number, number, number]) : null
if (BBOX && (BBOX.length !== 4 || BBOX.some((x) => !Number.isFinite(x)))) {
  console.error(`ERROR: --bbox must be minLat,minLon,maxLat,maxLon (got "${bboxArg}")`)
  process.exit(1)
}

// No distance cap (Ondra 2026-06-25): the per-hex scope (~22 km) already bounds
// the reach, and the flow DECREASES at every junction — it dies to the class
// default on its own wherever side roads exist. A hard cap only chopped LEGITIMATE
// flow on long junction-free stretches (a motorway between exits really does carry
// a constant volume; cars don't vanish). A wrong anchor's reach is bounded by the
// hex and caught by the R13 audit, not by an arbitrary metre count.
/** Two anchors redistributing into the same segment whose totals differ by more
 *  than this are a real disagreement → skip it. Within it, take the lower
 *  (conservative). For choosing/vetoing here, NOT audit severity. */
const CONFLICT_RATIO = 3
/** Classes the fill may WRITE — major roads + links. Local roads (5-9) are the
 *  service-tree's job, track/path never. A local road still DRAWS its share at a
 *  junction (so the main road loses that flow), it just never receives a fill.
 *  Mirrors `roadCoverage` on the dataset + the `coverage` set to writeRoadAadt. */
const FILLABLE = new Set([0, 1, 2, 3, 4, 10, 11, 12])

/** Canonical ref token for same-road matching: trim + uppercase + collapse
 *  whitespace. NO leading-zero / prefix stripping ("0033" stays "0033") — the
 *  topological walk (shared physical endpoint) already prevents merging two
 *  disconnected roads that share a number, so an exact canonical match is correct
 *  and simplest (aligns with R5 / R13). */
function canonRef(r: string | null): string {
  return r ? r.trim().toUpperCase().replace(/\s+/g, '') : ''
}

interface Seg {
  i: number
  cls: number
  ref: string
  source: number
  light: number
  medium: number
  heavy: number
  moto: number
  total: number
  a: string
  b: string
}

/** A vehicle-class flow vector + provenance (which anchor produced it). */
interface Flow {
  light: number
  medium: number
  heavy: number
  moto: number
  total: number
  anchor: number
}

/** Re-scale a flow vector to a new total, keeping the vehicle-class proportions
 *  of the source measurement (the branches' real split is unknown). */
function scaleFlow(v: Flow, newTotal: number, anchor: number): Flow {
  if (v.total <= 0 || newTotal <= 0) return { light: 0, medium: 0, heavy: 0, moto: 0, total: 0, anchor }
  const f = newTotal / v.total
  return { light: v.light * f, medium: v.medium * f, heavy: v.heavy * f, moto: v.moto * f, total: newTotal, anchor }
}

/** What a segment "draws" at a junction: a MEASURED segment draws its real flow
 *  (the truth); everything else — default, service-tree, and crucially our OWN
 *  previous fill (id 12) — draws only its class default. Using a fill's output as
 *  a "known" draw on re-run would over-drain the main road and let heuristic
 *  output masquerade as truth, breaking idempotence (/gg Codex). */
function drawEstimate(s: Seg): number {
  return isMeasured(s.source) ? s.total : classDefaultTotal(s.cls)
}

/** One hex: read roads.arrow, build the endpoint graph, redistribute each measured
 *  anchor's flow through the network, write via the shared provenance-gated
 *  writer. Returns counts for the run summary. */
async function processHex(arrowPath: string): Promise<{ filled: number; conflicts: number }> {
  const table = tableFromIPC(readFileSync(arrowPath))
  const n = table.numRows
  if (n === 0) return { filled: 0, conflicts: 0 }

  const refC = table.getChild('ref')
  const clsC = table.getChild('road_class')
  const srcC = table.getChild('source_id')
  const sLatC = table.getChild('start_lat')
  const sLonC = table.getChild('start_lon')
  const eLatC = table.getChild('end_lat')
  const eLonC = table.getChild('end_lon')
  const lC = table.getChild('aadt_light')
  const mC = table.getChild('aadt_medium')
  const hC = table.getChild('aadt_heavy')
  const moC = table.getChild('aadt_moto')
  if (!sLatC || !sLonC || !clsC) return { filled: 0, conflicts: 0 } // malformed hex

  const segs: Seg[] = []
  const endpoint = new Map<string, number[]>() // endpoint node → seg indices touching it
  for (let i = 0; i < n; i++) {
    const sLat = sLatC.get(i) as number
    const sLon = sLonC.get(i) as number
    const eLat = (eLatC?.get(i) as number) ?? sLat
    const eLon = (eLonC?.get(i) as number) ?? sLon
    const light = (lC?.get(i) as number) ?? 0
    const medium = (mC?.get(i) as number) ?? 0
    const heavy = (hC?.get(i) as number) ?? 0
    const moto = (moC?.get(i) as number) ?? 0
    const a = nodeKey(sLat, sLon)
    const b = nodeKey(eLat, eLon)
    segs.push({
      i,
      cls: (clsC.get(i) as number) ?? 5,
      ref: canonRef((refC?.get(i) as string | null) ?? null),
      source: (srcC?.get(i) as number) ?? 0,
      light,
      medium,
      heavy,
      moto,
      total: light + medium + heavy + moto,
      a,
      b,
    })
    for (const k of [a, b]) {
      const arr = endpoint.get(k)
      if (arr) arr.push(i)
      else endpoint.set(k, [i])
    }
  }

  const fill = new Map<number, Flow>()
  const conflicted = new Set<number>()
  let conflicts = 0

  // Record a redistributed flow onto a target segment, arbitrating between
  // anchors. Returns false if the segment is DISPUTED (two anchors disagree >3x)
  // so the caller doesn't propagate flow through a contested junction (/gg Codex).
  const record = (t: number, v: Flow): boolean => {
    if (conflicted.has(t)) return false
    const prev = fill.get(t)
    if (prev && prev.anchor !== v.anchor) {
      const ratio = Math.max(prev.total, v.total) / Math.max(1, Math.min(prev.total, v.total))
      if (ratio > CONFLICT_RATIO) {
        fill.delete(t)
        conflicted.add(t)
        conflicts++
        return false
      }
      if (v.total < prev.total) fill.set(t, v) // conservative: keep the lower
      return true
    }
    fill.set(t, v)
    return true
  }

  // Multi-source flow redistribution from every measured anchor.
  for (const A of segs) {
    if (!isMeasured(A.source) || A.total <= 0) continue
    const seen = new Set<number>([A.i])
    const queue: Array<{ seg: number; v: Flow }> = [
      { seg: A.i, v: { light: A.light, medium: A.medium, heavy: A.heavy, moto: A.moto, total: A.total, anchor: A.i } },
    ]
    let head = 0
    while (head < queue.length) {
      const { seg, v } = queue[head++]
      for (const ep of [segs[seg].a, segs[seg].b]) {
        const outs = (endpoint.get(ep) ?? []).filter((j) => j !== seg)
        if (outs.length === 0) continue

        // Decide each output's share of the incoming flow `v`.
        const sameRef = segs[seg].ref ? outs.filter((o) => segs[o].ref === segs[seg].ref) : []
        const targets: Array<{ seg: number; total: number }> = []
        if (sameRef.length >= 1) {
          // Same number continues; other branches draw off their own flow.
          let drawn = 0
          for (const o of outs) if (!sameRef.includes(o)) drawn += drawEstimate(segs[o])
          const mainTotal = Math.max(0, v.total - drawn)
          for (const p of sameRef) targets.push({ seg: p, total: mainTotal / sameRef.length })
        } else {
          // Road split into different numbers → divide by class-default ratio.
          let totalDef = 0
          for (const o of outs) totalDef += classDefaultTotal(segs[o].cls)
          if (totalDef <= 0) continue
          for (const o of outs) targets.push({ seg: o, total: (v.total * classDefaultTotal(segs[o].cls)) / totalDef })
        }

        for (const { seg: t, total: tTotal } of targets) {
          if (seen.has(t)) continue
          const o = segs[t]
          // Only fillable, unmeasured (or our own) segments receive + carry flow.
          // Measured = boundary (its own truth); local = sink (drew its share above).
          if (!FILLABLE.has(o.cls) || (o.source !== 0 && o.source !== MY_SOURCE_ID)) continue
          // Stop once the flow has fallen to the segment's own class default — the
          // engine default already covers it, and propagating less is meaningless.
          if (tTotal <= classDefaultTotal(o.cls)) continue
          seen.add(t)
          const vt = scaleFlow(v, tTotal, v.anchor)
          // Only carry flow onward through segments we actually accepted — a
          // disputed (conflicted) segment is a dead-end, not a downstream seed.
          if (record(t, vt)) queue.push({ seg: t, v: vt })
        }
      }
    }
  }

  if (fill.size === 0) return { filled: 0, conflicts }

  // Write via the shared provenance-gated writer. It re-reads the file, but row
  // index `i` is stable (same file, same order), so the precomputed `fill` Map
  // keys line up. The gate guarantees we never overwrite a measured row.
  const res = await writeRoadAadt(
    arrowPath,
    (_row, i) => {
      const f = fill.get(i)
      return f
        ? {
            light: Math.round(f.light),
            medium: Math.round(f.medium),
            heavy: Math.round(f.heavy),
            moto: Math.round(f.moto),
            sourceId: MY_SOURCE_ID,
          }
        : null
    },
    undefined,
    FILLABLE,
  )
  return { filled: res.matched, conflicts }
}

async function main() {
  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }
  // --bbox → one region (road re-stamp); else the full tree (optionally a
  // --prefix slice). Sorted so SHARD slicing is reproducible (readdirSync order
  // isn't filesystem-guaranteed). Same enumeration shape as service-tree.
  let hexDirs = (
    BBOX
      ? iterateCountryHexes(H3R4_DIR, BBOX)
      : readdirSync(H3R4_DIR).filter((d) => !d.startsWith('.') && (!PREFIX || d.startsWith(PREFIX)))
  ).sort()

  if (process.env.SHARD) {
    const m = /^(\d+)\/(\d+)$/.exec(process.env.SHARD)
    const i = m ? Number(m[1]) : NaN
    const nShards = m ? Number(m[2]) : NaN
    if (!m || nShards <= 0 || i >= nShards) {
      console.error(`ERROR: invalid SHARD="${process.env.SHARD}" (expected i/n with 0 <= i < n)`)
      process.exit(1)
    }
    const start = Math.floor((i * hexDirs.length) / nShards)
    const end = Math.floor(((i + 1) * hexDirs.length) / nShards)
    hexDirs = hexDirs.slice(start, end)
  }

  let totalFilled = 0
  let totalConflicts = 0
  let hexesUpdated = 0
  for (const hexId of hexDirs) {
    const arrowPath = resolve(H3R4_DIR, hexId, 'roads.arrow')
    if (!existsSync(arrowPath)) continue
    const { filled, conflicts } = await processHex(arrowPath)
    totalFilled += filled
    totalConflicts += conflicts
    if (filled > 0) hexesUpdated++
  }

  console.log(`\n=== Results ===`)
  console.log(`  ${totalFilled} segments filled across ${hexesUpdated} hexes`)
  console.log(`  ${totalConflicts} segments skipped (conflicting anchors >${CONFLICT_RATIO}x)`)
}

main().catch((err) => {
  console.error('Error:', err)
  process.exit(1)
})
