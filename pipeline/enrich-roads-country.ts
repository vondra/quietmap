//! Per-segment country/city/continent bake (plan M3, 2026-07-28).
//!
//! Writes THREE columns, all-or-none, into every hex's roads.arrow AND
//! railways.arrow:
//!
//!   country_iso  UInt16 — ISO 3166-1 alpha-2 as two ASCII bytes packed
//!                `iso0 | iso1<<8` (the u16's little-endian byte pair IS
//!                [iso0, iso1], matching engine Admin.country_iso [u8;2]);
//!                0 = `\0\0` = Admin::UNKNOWN.
//!   city_id      UInt16 — metro id from scripts/h3-admin-metros.json
//!                (cityAt, gated by the RESOLVED country); 0 = no metro.
//!   continent    UInt8 — EXACT mirror of
//!                engine/noise-compute/src/admin.rs::Continent (repr(u8)):
//!                0 Unknown, 1 Europe, 2 NorthAmerica, 3 SouthAmerica,
//!                4 Asia, 5 Africa, 6 Oceania.
//!
//! Country, city, and continent are static geography, so resolve them once,
//! offline, per segment; the runtime reads a column and never guesses:
//! antimeridian-safe midpoint of (start,end) → `adminAt` (lib/admin-at.ts, M2 —
//! exact CGAZ PIP + uniquely-attributable 2 km coastal fallback) → `cityAt`
//! (metro PIP gated by the resolved country) → `continentForIso` (the ONE
//! ISO→continent table, scripts/iso-continent.mjs). UNRESOLVED (iso2 undefined)
//! bakes all three columns as 0 — present `00` reads as Admin::UNKNOWN with NO
//! engine fallback (fallback fires only when the columns are ABSENT).
//!
//! Bake-time invariant (FATAL, the G1 trap): a file whose on-land rows resolve
//! 100% UNKNOWN means the resolver silently died for that hex (CGAZ feature
//! gap/corruption) — stamping `\0\0` there would bake garbage that reads as
//! legitimately-unknown. Such a file is left UNWRITTEN, reported, and the pass
//! exits nonzero. "On-land" is tested independently of adminAt's verdict (see
//! landStatusAt): unresolved midpoints are split into claimed-land (should
//! have resolved — the trap signature), disputed (UNMAPPED CGAZ numeric
//! shapeGroups — land, but AdminAt policy resolves `\0\0` BY DESIGN; reported,
//! never fatal) and offshore (sea — bridges/piers/causeways legitimately bake
//! 0; reported, never fatal). Policy-MAPPED disputed areas (Abyei → SD, Aksai
//! Chin → CN, Falklands → FK — admin-at.ts::DISPUTED_SHAPEGROUP_ISO) resolve
//! to their administering country and never reach the unresolved split.
//!
//! Contract metadata (plan M3 — Convention-B per-file contract, the
//! finalize/mod.rs layout): the stamp rides IN THE SAME atomic write as the
//! columns (`roads_contract` / `railways_contract` = `country_baked_v1`) — an
//! extract-time stamp would pass on un-baked data. preserveArrowShape
//! (lib/provenance.ts) carries output-side metadata through, and every written
//! file is RE-READ and asserted (triplet present with the right types, stamp
//! intact, row count unchanged) before the hex is declared done.
//!
//! Idempotent + safe: unchanged hexes are left byte-identical; changed hexes go
//! through `withArrowWrite` (flock + tmp + rename, never truncate in place).
//! Per-hex, self-contained → SHARD=i/n parallelizes it like built-up /
//! service-tree (wired in scripts/osm-to-h3r4.sh, after the CGAZ cache warm).
//!
//! Usage:
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-country.ts
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-country.ts --prefix 841e309
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-country.ts --bbox 49.7,13.9,50.4,15.0
//!   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-country.ts --out-dir /tmp/m3-test/h3r4
//!   SHARD=0/96 DATA_YEAR=2026 node_modules/.bin/tsx pipeline/enrich-roads-country.ts

import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import {
  makeVector,
  makeTable,
  tableFromIPC,
  RecordBatch,
  Schema,
  Table,
  Type,
} from 'apache-arrow'
import { withArrowWrite } from './lib/provenance.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import {
  adminAt,
  antimeridianMidpoint,
  cityAt,
  claimantShapeGroup,
  continentForIso,
} from './lib/admin-at.js'
import { DATA_YEAR as YEAR } from './lib/data-year.js'

const outDirIdx = process.argv.indexOf('--out-dir')
if (outDirIdx !== -1 && !process.argv[outDirIdx + 1]) {
  console.error('ERROR: --out-dir requires a directory argument')
  process.exit(1)
}
const H3R4_DIR =
  outDirIdx !== -1
    ? resolve(process.argv[outDirIdx + 1])
    : resolve(import.meta.dirname, `../data/prepared/${YEAR}/h3r4`)
const PREFIX = process.argv.includes('--prefix') ? process.argv[process.argv.indexOf('--prefix') + 1] : ''
const bboxArg = process.argv.includes('--bbox') ? process.argv[process.argv.indexOf('--bbox') + 1] : ''
const BBOX = bboxArg ? (bboxArg.split(',').map(Number) as [number, number, number, number]) : null
if (BBOX && (BBOX.length !== 4 || BBOX.some((x) => !Number.isFinite(x)))) {
  console.error(`ERROR: --bbox must be minLat,minLon,maxLat,maxLon (got "${bboxArg}")`)
  process.exit(1)
}

/** Convention-B contract value stamped into schema metadata at bake time. */
const CONTRACT_VALUE = 'country_baked_v1'

/** The all-or-none column triplet + the unsigned-int width each must carry
 *  (hard fail on partial presence or wrong type — plan M3 §1). apache-arrow
 *  JS gives EVERY int width the same typeId (Type.Int), so the check compares
 *  bitWidth + isSigned, not typeId. */
const COLUMNS = [
  { name: 'country_iso', bitWidth: 16 },
  { name: 'city_id', bitWidth: 16 },
  { name: 'continent', bitWidth: 8 },
] as const
const COLUMN_NAMES: ReadonlySet<string> = new Set(COLUMNS.map((c) => c.name))

/** True iff `type` is an UNSIGNED int of exactly `bitWidth` bits. */
function isUintOfWidth(type: { typeId: number; bitWidth?: number; isSigned?: boolean }, bitWidth: number): boolean {
  return type.typeId === Type.Int && type.bitWidth === bitWidth && type.isSigned === false
}

const LAYERS = ['roads', 'railways'] as const
type LayerName = (typeof LAYERS)[number]

// Continent ids — EXACT mirror of engine/noise-compute/src/admin.rs::Continent
// (repr(u8)) and scripts/build-h3-admin.ts::CONTINENT_ID. continentForIso is
// the ONE ISO→continent-name table (scripts/iso-continent.mjs, shared via
// admin-at.ts); an iso2 with no table entry (e.g. AQ) bakes 0 =
// Continent::Unknown, matching the admin.bin convention.
const CONTINENT_ID: Readonly<Record<string, number>> = {
  Europe: 1,
  NorthAmerica: 2,
  SouthAmerica: 3,
  Asia: 4,
  Africa: 5,
  Oceania: 6,
}

function continentIdForIso(iso2: string): number {
  return CONTINENT_ID[continentForIso(iso2) ?? ''] ?? 0
}

// ── On-land classification for UNRESOLVED midpoints ─────────────────────────
//
// adminAt's `undefined` conflates three cases the bake must count separately:
//   claimed   inside a CGAZ ring WITH ISO identity — adminAt should have
//             resolved it; a hex with such rows and ZERO resolved rows means
//             the resolver is silently dead there (G1 trap) → FATAL.
//   disputed  inside an UNMAPPED numeric-shapeGroup (US-DoS disputed) area —
//             land, but AdminAt policy resolves `\0\0` BY DESIGN (no coastal
//             fallback; mapped shapeGroups — Abyei/Aksai Chin/Falklands —
//             resolve to their administering country upstream and never reach
//             this classification). Reported, never fatal.
//   sea       open sea / strait-ambiguous water — bridges/piers/causeways
//             legitimately bake 0. Reported, never fatal.
// The test deliberately does NOT trust adminAt's verdict: it walks the shared
// cgazLandIndex rings directly (same PIP primitives via country-polygon.ts),
// so a broken resolver trips the gate instead of being stamped over. Called
// only for unresolved midpoints (rare), so a bbox-prechecked linear scan is
// fast enough.

type LandStatus = 'claimed' | 'disputed' | 'sea'

/** 'claimed' | 'disputed' | 'sea' at the point — ONE call into AdminAt's
 *  shared part index (no duplicated geometry here; /gg M3). A numeric
 *  shapeGroup is a CGAZ disputed-area code (no ISO identity) per the
 *  country-polygon.ts convention — NOT the ISO table, so a future CGAZ
 *  alpha code can't silently fall into the non-fatal bucket. */
function landStatusAt(lat: number, lon: number): LandStatus {
  const g = claimantShapeGroup(lat, lon)
  return g === null ? 'sea' : /^\d+$/.test(g) ? 'disputed' : 'claimed'
}

// ── Resolver liveness probes (/gg M3 CRITICAL) ───────────────────────────────
// The bake invariant shares CGAZ with adminAt, so a corrupt/empty polygon
// file would zero BOTH verdicts and ship a whole-world \0\0 bake silently.
// These hard-coded on-land probes must resolve before anything writes.
const REFERENCE_PROBES: Array<[number, number, string]> = [
  [50.087, 14.421, 'CZ'], // Prague
  [48.857, 2.352, 'FR'], // Paris
  [35.681, 139.767, 'JP'], // Tokyo
  [6.524, 3.379, 'NG'], // Lagos
  [-23.55, -46.633, 'BR'], // São Paulo
  [-33.868, 151.209, 'AU'], // Sydney
]
function assertResolverAlive(): void {
  for (const [lat, lon, want] of REFERENCE_PROBES) {
    const got = adminAt(lat, lon).iso2
    if (got !== want) {
      console.error(`FATAL: resolver liveness probe failed at ${lat},${lon}: got ${got}, want ${want} — refusing to bake (polygon data broken?)`)
      process.exit(1)
    }
  }
}

// ── Per-file bake ────────────────────────────────────────────────────────────

interface FileResult {
  layer: LayerName
  rows: number
  /** adminAt resolved a country (PIP or coastal fallback). */
  resolved: number
  /** Rows tagged with a metro id (>0). */
  metro: number
  /** UNRESOLVED on claimed land — the G1-trap signature. */
  unknownOnLand: number
  /** UNRESOLVED inside a disputed area — legitimate `\0\0` by M2 policy. */
  disputed: number
  /** UNRESOLVED over sea — legitimate `\0\0` (bridges/piers/causeways). */
  offshore: number
  changed: boolean
  /** Invariant tripped: on-land rows present and 100 % UNKNOWN — file NOT written. */
  invariantFailed: boolean
  /** Malformed/empty file skipped (no triplet, no stamp) — counted, never silent. */
  skipped: boolean
}

/** Column-presence + contract assertion on read-back: a file is declared done
 *  ONLY after the on-disk bytes prove the triplet and the stamp survived the
 *  write (preserveArrowShape's metadata merge / batch re-slicing). Any miss
 *  here is a writer bug — throw and fail the shard. */
function assertBaked(arrowPath: string, contractKey: string, rows: number): void {
  const t = tableFromIPC(readFileSync(arrowPath))
  if (t.numRows !== rows) {
    throw new Error(`${arrowPath}: read-back row count ${t.numRows} ≠ ${rows} right after a bake write`)
  }
  for (const c of COLUMNS) {
    const v = t.getChild(c.name)
    if (!v) throw new Error(`${arrowPath}: read-back missing column ${c.name} right after a bake write`)
    if (!isUintOfWidth(v.type, c.bitWidth)) {
      throw new Error(`${arrowPath}: read-back column ${c.name} has type ${v.type} (expected Uint${c.bitWidth})`)
    }
  }
  const stamp = t.schema.metadata.get(contractKey)
  if (stamp !== CONTRACT_VALUE) {
    throw new Error(`${arrowPath}: read-back ${contractKey}='${stamp}' (expected '${CONTRACT_VALUE}') — metadata did not survive the write`)
  }
}

/** One layer file of one hex: resolve every segment midpoint, rewrite with the
 *  country_iso/city_id/continent triplet + contract stamp added (or replaced —
 *  re-runs are byte-identical no-ops). */
async function processFile(
  arrowPath: string,
  layer: LayerName,
  countries: Map<string, number>,
): Promise<FileResult> {
  const contractKey = `${layer}_contract`
  const res: FileResult = {
    layer, rows: 0, resolved: 0, metro: 0, unknownOnLand: 0, disputed: 0, offshore: 0,
    changed: false, invariantFailed: false, skipped: false,
  }
  await withArrowWrite(arrowPath, (table: Table): Table => {
    const n = table.numRows
    res.rows = n
    const sLat = table.getChild('start_lat')
    const sLon = table.getChild('start_lon')
    const eLat = table.getChild('end_lat')
    const eLon = table.getChild('end_lon')
    if (n === 0 || !sLat || !sLon) {
      res.skipped = true // empty/malformed hex — never touch, but count it
      return table
    }

    // All-or-none contract (plan M3 §1): partial presence or a wrong type is a
    // hard fail — the file stays untouched.
    const present = COLUMNS.filter((c) => table.getChild(c.name) !== null)
    if (present.length > 0 && present.length < COLUMNS.length) {
      throw new Error(
        `${arrowPath}: partial country bake — only [${present.map((c) => c.name).join(', ')}] of ` +
          `${COLUMNS.map((c) => c.name).join('/')} present (all-or-none contract)`,
      )
    }
    const baked = present.length === COLUMNS.length
    if (baked) {
      for (const c of COLUMNS) {
        const v = table.getChild(c.name)!
        if (!isUintOfWidth(v.type, c.bitWidth)) {
          throw new Error(`${arrowPath}: existing column ${c.name} has wrong type ${v.type} (expected Uint${c.bitWidth}) — refusing to mix bake versions`)
        }
      }
    }

    const countryIso = new Uint16Array(n)
    const city = new Uint16Array(n)
    const cont = new Uint8Array(n)
    const exIso = baked ? table.getChild('country_iso')! : null
    const exCity = baked ? table.getChild('city_id')! : null
    const exCont = baked ? table.getChild('continent')! : null
    let sameAsExisting = baked
    for (let i = 0; i < n; i++) {
      const startLat = sLat.get(i) as number
      const startLon = sLon.get(i) as number
      const endLat = (eLat?.get(i) as number) ?? startLat
      const endLon = (eLon?.get(i) as number) ?? startLon
      const [midLat, midLon] = antimeridianMidpoint(startLat, startLon, endLat, endLon)
      const { iso2 } = adminAt(midLat, midLon)
      if (iso2 !== undefined) {
        res.resolved++
        countryIso[i] = iso2.charCodeAt(0) | (iso2.charCodeAt(1) << 8)
        city[i] = cityAt(midLat, midLon, iso2)
        cont[i] = continentIdForIso(iso2)
        if (city[i] > 0) res.metro++
        countries.set(iso2, (countries.get(iso2) ?? 0) + 1)
      } else {
        // All three columns stay 0 (the Admin::UNKNOWN sentinel). Classify the
        // miss for the invariant/report — the columns are identical either way.
        const land = landStatusAt(midLat, midLon)
        if (land === 'claimed') res.unknownOnLand++
        else if (land === 'disputed') res.disputed++
        else res.offshore++
      }
      if (
        sameAsExisting &&
        ((exIso!.get(i) as number) !== countryIso[i] ||
          (exCity!.get(i) as number) !== city[i] ||
          (exCont!.get(i) as number) !== cont[i])
      ) {
        sameAsExisting = false
      }
    }

    // Bake-time invariant (FATAL — the G1 trap): claimed-land rows present and
    // NOT A SINGLE row resolved = the resolver silently died for this hex.
    // Stamping `\0\0` here would bake garbage indistinguishable from
    // legitimately-unknown; leave the file UNWRITTEN and let the pass exit
    // nonzero at the end. Offshore-only and disputed-only files do NOT trip —
    // 100 % UNKNOWN is their legitimate outcome.
    if (res.unknownOnLand > 0 && res.resolved === 0) {
      res.invariantFailed = true
      return table
    }

    const stamped = table.schema.metadata.get(contractKey) === CONTRACT_VALUE
    if (sameAsExisting && stamped) return table // idempotent re-run → leave bytes untouched

    // Rebuild preserving every other column verbatim (same schema-copy/append
    // idiom as writeRoadAadt in lib/roads-arrow.ts): the triplet is appended
    // when absent, replaced in place when not — ALL THREE together or not at
    // all, and the contract stamp rides in the SAME atomic write.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- makeTable's
    // typing wants TypedArrays; mixing existing Vectors with makeVector is fine
    // at runtime (same bridge as writeRoadAadt).
    const cols: Record<string, any> = {}
    for (const f of table.schema.fields) {
      if (COLUMN_NAMES.has(f.name)) continue
      cols[f.name] = table.getChild(f.name)!
    }
    cols['country_iso'] = makeVector(countryIso)
    cols['city_id'] = makeVector(city)
    cols['continent'] = makeVector(cont)
    const built = makeTable(cols)
    // Output-side metadata overrides input metadata in preserveArrowShape
    // (lib/provenance.ts), so the stamp survives the write exactly once.
    const md = new Map(built.schema.metadata)
    md.set(contractKey, CONTRACT_VALUE)
    const schema = new Schema(built.schema.fields, md)
    res.changed = true
    return new Table(schema, built.batches.map((b) => new RecordBatch(schema, b.data)))
  })
  if (res.changed) assertBaked(arrowPath, contractKey, res.rows)
  return res
}

async function main() {
  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }
  assertResolverAlive()

  // Same enumeration shape as built-up/service-tree: --bbox → region (union of
  // the two layer files so rail-only hexes are not dropped), else the full
  // tree (optionally --prefix), sorted so SHARD slices reproduce.
  let hexDirs = (
    BBOX
      ? [
          ...new Set([
            ...iterateCountryHexes(H3R4_DIR, BBOX, 'roads.arrow'),
            ...iterateCountryHexes(H3R4_DIR, BBOX, 'railways.arrow'),
          ]),
        ]
      : readdirSync(H3R4_DIR).filter(
          (d) => d.length === 15 && d.endsWith('ffffffff') && (!PREFIX || d.startsWith(PREFIX)),
        )
  ).sort()

  if (process.env.SHARD) {
    const m = /^(\d+)\/(\d+)$/.exec(process.env.SHARD)
    const i = m ? Number(m[1]) : NaN
    const nShards = m ? Number(m[2]) : NaN
    if (!m || nShards <= 0 || i >= nShards) {
      console.error(`ERROR: invalid SHARD="${process.env.SHARD}" (expected i/n with 0 <= i < n)`)
      process.exit(1)
    }
    hexDirs = hexDirs.slice(Math.floor((i * hexDirs.length) / nShards), Math.floor(((i + 1) * hexDirs.length) / nShards))
  }

  const t0 = Date.now()
  let hexes = 0
  let files = 0
  let changedFiles = 0
  const zeroTotals = () => ({ rows: 0, resolved: 0, metro: 0, unknownOnLand: 0, disputed: 0, offshore: 0 })
  const totals: Record<LayerName, ReturnType<typeof zeroTotals>> = { roads: zeroTotals(), railways: zeroTotals() }
  const countries = new Map<string, number>()
  const failed: string[] = [] // invariant trips — FATAL at the end
  const gapWarn: string[] = [] // partial claimed-land UNKNOWN alongside resolved rows
  let skipped = 0 // malformed/empty files (no triplet, no stamp)
  for (const hexId of hexDirs) {
    const hexDir = resolve(H3R4_DIR, hexId)
    let touched = false
    for (const layer of LAYERS) {
      const arrowPath = resolve(hexDir, `${layer}.arrow`)
      if (!existsSync(arrowPath)) continue
      touched = true
      const r = await processFile(arrowPath, layer, countries)
      if (r.skipped) skipped++
      files++
      if (r.changed) changedFiles++
      const t = totals[layer]
      t.rows += r.rows
      t.resolved += r.resolved
      t.metro += r.metro
      t.unknownOnLand += r.unknownOnLand
      t.disputed += r.disputed
      t.offshore += r.offshore
      if (r.invariantFailed) {
        failed.push(
          `${hexId}/${layer}.arrow (rows=${r.rows}, unknown-on-land=${r.unknownOnLand}, disputed=${r.disputed}, offshore=${r.offshore})`,
        )
      } else if (r.unknownOnLand > 0) {
        gapWarn.push(`${hexId}/${layer}.arrow (${r.unknownOnLand} claimed-land row(s) UNKNOWN but ${r.resolved} row(s) resolved)`)
      }
    }
    if (touched) hexes++
    if (touched && hexes % 1000 === 0) {
      const dt = ((Date.now() - t0) / 1000).toFixed(0)
      const t = totals.roads
      console.log(
        `  progress: ${hexes}/${hexDirs.length} hexes in ${dt}s — ${t.rows} road rows (${t.resolved} resolved / ${t.unknownOnLand} unknown-on-land / ${t.offshore} offshore)`,
      )
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  ${hexes} hexes scanned, ${files} files processed, ${changedFiles} rewritten${skipped > 0 ? `, ${skipped} skipped (malformed/empty)` : ''}`)
  for (const layer of LAYERS) {
    const t = totals[layer]
    console.log(
      `  ${layer}: rows=${t.rows} resolved=${t.resolved} unknown_on_land=${t.unknownOnLand} disputed=${t.disputed} offshore=${t.offshore} metro=${t.metro}`,
    )
  }
  const topCountries = [...countries.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 20)
    .map(([k, v]) => `${k}=${v}`)
    .join(' ')
  console.log(`  countries (top 20): ${topCountries || '(none)'}`)
  if (gapWarn.length > 0) {
    console.log(`  WARNING: ${gapWarn.length} file(s) had claimed-land rows resolving UNKNOWN alongside resolved rows (resolver gap — investigate):`)
    for (const w of gapWarn.slice(0, 20)) console.log(`    ${w}`)
    if (gapWarn.length > 20) console.log(`    … and ${gapWarn.length - 20} more`)
  }
  const totalRows = totals.roads.rows + totals.railways.rows
  const totalResolved = totals.roads.resolved + totals.railways.resolved
  // Global guard (/gg M3): only the liveness probes are independent of the
  // resolver's verdict — a row-level "0 resolved" can be LEGITIMATE (an
  // offshore-only test hex). At world scale it cannot: a bake scanning a
  // million rows and resolving none means the resolver died silently.
  if (totalRows > 1_000_000 && totalResolved === 0) {
    console.error(`  FATAL: ${totalRows} rows scanned, 0 resolved — resolver globally dead, refusing to publish`)
    process.exit(1)
  }
  if (failed.length > 0) {
    console.log(
      `  FATAL: ${failed.length} file(s) had on-land rows resolving 100% UNKNOWN (the G1 trap — resolver silently dead for the hex; file left UNWRITTEN):`,
    )
    for (const f of failed.slice(0, 20)) console.log(`    ${f}`)
    if (failed.length > 20) console.log(`    … and ${failed.length - 20} more`)
    process.exit(1)
  }
}

main().catch((err) => {
  console.error('Error:', err)
  process.exit(1)
})
