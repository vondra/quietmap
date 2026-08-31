//! R8.1 enrichment-chain manifest — THE declarative order of every enrichment
//! step that must re-run after a fresh OSM planet extract, with per-step cache
//! semantics and the footguns encoded as data.
//!
//! Steps are ENUMERATED FROM THE FILESYSTEM (pipeline/enrich-*.ts) and joined
//! against the per-country spec tables below; a file this manifest cannot
//! classify THROWS at plan time — a new enricher cannot silently stay outside
//! the chain. Non-`enrich-*` diagnostics (enrichment-status, audit-map-
//! discontinuities, calibrate-lane-ratios, bench/*) are
//! deliberately not steps; `write_industrial.rs` emits site_subtype at extract
//! time, so no backfill step is needed.
//!
//! Phase order (extract → global prior → national census → city → heuristics →
//! taper → gate):
//!   column-parents  enrich-roads-built-up — built_up has NO extractor parent;
//!                   the engine reads it for untagged-road speeds, taper reads
//!                   it for ramp targets. Runs first so every later road pass
//!                   sees it. (osm-to-h3r4.sh tail also runs it when
//!                   RUN_SERVICE_TREE=1 — the re-run is a byte-identical no-op.)
//!                   enrich-roads-country — the M3 per-segment
//!                   country_iso/city_id/continent bake into roads+railways;
//!                   also chain-only, also run by the osm-to-h3r4.sh tail.
//!   global-priors   rail country-bleed PRE-heal (BEFORE any rail claimer — a
//!                   post-claimer heal can only retract, leaving freed rows
//!                   unclaimed until the NEXT run; /gg Codex CRITICAL), then
//!                   worldwide + continental datasets (GPPD/E-PRTR/GEM, USWTDB,
//!                   EU city traffic, continental GTFS). Rank gates make the
//!                   final state order-independent vs national; canonical order
//!                   global→national minimizes wasted writes (audit §8.1).
//!   national        every enrich-{roads,railway,buildings,industrial}-{cc}.ts.
//!   city            enrich-cities-roads.ts per enabled city — SEQUENTIAL and
//!                   after national on purpose (same-hex flock contention;
//!                   city-measured rank outranks national inside the polygon).
//!   heuristics      service-tree + continuity-fill (NEED the national/city
//!                   anchors stamped first), rail-heal-verify (read-only
//!                   tripwire AFTER all rail claimers: exit 1 = a claimer
//!                   re-bled across a border this run), parallel divisors
//!                   (AFTER all rail enrichers), NACE name mop-up.
//!   taper           enrich-roads-taper — ABSOLUTELY LAST road write: ramps are
//!                   derived annotations of the FINAL AADT/speed state; any
//!                   later write would void them (the shared writer clears
//!                   speed_taper on overwrite/retract by contract).
//!   gate            audit-enrichment-invariants.ts with machine outputs;
//!                   run.ts diffs the per-violation fingerprint MULTISET
//!                   against pipeline/chain/gate-baselines/{year}.{scope}.json
//!                   (NEW fingerprints fail; pre-existing pass with a warning;
//!                   crash/signal/exit-3 always fails).
//!
//! Chain-wide footguns (enforced by run.ts, documented here as the SSOT):
//!   • NEVER run `npm run gen:sources` mid-chain: it rewrites source ids in
//!     source-ids.generated.ts + engine sources.rs; already-spawned tsx/engine
//!     processes keep the old table → one dataset stamps under two ids and
//!     R0 unknown-source fires (memory: industrial-enrich-fix-recipe).
//!   • run.ts SPAWNS scripts, never imports them — several enrichers execute
//!     main() at module top level (e.g. enrich-industrial-name-heuristic.ts);
//!     an import would trigger a full write pass inside the runner process.
//!   • A fresh planet extract does NOT invalidate data/enrichment caches —
//!     feeds stay valid; the chain therefore always runs cache-only
//!     (--enrich-only where supported) and SKIPS with a reason when a cache is
//!     missing. Downloading mid-chain would silently change data vintage.

import { readdirSync, existsSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { DATA_YEAR as YEAR } from '../lib/data-year.js'
import { CITY_DATASETS } from '../lib/city-datasets.js'
import { FEEDS as EUROPE_FEEDS } from '../enrich-railway-europe.js'
import { bboxIntersects, countryIntersectsBbox, serializeBbox, type Bbox, type ResolvedScope } from './scope.js'

export const PIPELINE_DIR = resolve(import.meta.dirname, '..')
export const REPO_ROOT = resolve(PIPELINE_DIR, '..')
const ENRICH_DIR = resolve(REPO_ROOT, 'data', 'enrichment')

export type Phase = 'column-parents' | 'global-priors' | 'national' | 'city' | 'heuristics' | 'taper' | 'gate'
export const PHASES: readonly Phase[] = ['column-parents', 'global-priors', 'national', 'city', 'heuristics', 'taper', 'gate']

export type Layer = 'roads' | 'railways' | 'buildings' | 'industrial' | 'rasters' | 'all'

/** One resolved plan entry: concrete args for the scope, or a skip reason. */
export interface PlanStep {
  id: string
  script: string // relative to pipeline/
  phase: Phase
  layer: Layer
  /** ISO2 (lower) from the filename for national steps, null otherwise. */
  country: string | null
  args: readonly string[]
  /** Per-step env on top of the runner defaults (DATA_YEAR pin + 8 GB heap).
   *  This is the ONLY way a step gets SHARD/START_INDEX/*_BBOX/DEBUG_OSM_ID —
   *  run.ts scrubs those from the ambient environment before every spawn. */
  env?: Readonly<Record<string, string>>
  notes: string
  /** Non-null = step is listed but not executed; the reason is always printed. */
  skipReason: string | null
  /** Conditional-skip marker honored by run.ts flags only (never automatic):
   *  'ran-by-extract-tail' + --assume-fresh-extract skips the step because the
   *  osm-to-h3r4.sh tail just produced identical output. */
  skipIf?: 'ran-by-extract-tail'
  /** #31.6 completeness FLOOR: the minimum number of input parts this step
   *  must load (36 EU cities; every feed in the europe GTFS registry — derived
   *  from FEEDS.length, never hand-written) for its stamp to be trusted. The
   *  step reports its actual via a QM_COMPLETENESS marker; run.ts records the
   *  {expected, actual, state} in status.json, clears safeToSync when a floored
   *  step loaded short, and FAILS a fresh full world run on it unless
   *  --allow-partial — the no-implicit-partial-stamp rule.
   *  Absent/0 = the step consumes no declared multi-part feed. */
  expectMinInputs?: number
}

// ── cache-behavior spec tables (from the 2026-07-11 full-file sweep) ─────────
// Values are paths relative to data/enrichment/. Semantics:
//   cachedDownload   script supports --enrich-only; chain passes it when the
//                    FIRST existing path is found, else SKIPs (never downloads).
//   downloadsAlways  script has no --enrich-only and would download when its
//                    cache is absent → SKIP unless every listed path exists.
//   requiredCache    script hard-exits when the cache is absent → pre-SKIP.
//   optionalCache    script degrades gracefully to computed defaults → always
//                    runs; listed paths are informational for the plan.
//   (absent from all tables) — no cache at all; always runs.

const ROADS_CACHED_DOWNLOAD: Record<string, string[]> = {
  ca: [`${YEAR}/ca/qc-djma.geojson`],
  cz: [`${YEAR}/cz/rsd-scitani.json`],
  de: [`${YEAR}/de/svz-census.json`, `${YEAR}/de/svz-autobahnen-2021.xlsx`],
  dk: [`${YEAR}/dk/mastra-page-0.json`],
  es: [`${YEAR}/es/mitma-tramos-parsed-v2.json`, `${YEAR}/es/mitma-tramos-2022.js`],
  fi: [`${YEAR}/fi/liikennemaarat-page-0.json`],
  fr: [`${YEAR}/fr/tmja-census-v2.json`, `${YEAR}/fr/tmja-2024.csv`],
  gb: [`${YEAR}/gb/dft-aadf.json`, `${YEAR}/gb/dft-aadf.zip`],
  ie: [`${YEAR}/ie/tii-parsed-aadt.json`, `${YEAR}/ie/tii-tmu-sites.json`],
  it: [`${YEAR}/it/tgm-roads.geojson`],
  no: [`${YEAR}/no/nvdb-trafikkmengde.json`],
  nz: [`${YEAR}/nz/nzta-page-0.json`],
  pl: [`${YEAR}/pl/gpr-2020-parsed-v3.json`, `${YEAR}/pl/gpr-2020-national.xls`],
  us: [`${YEAR}/us/hpms-page-0.json`],
}
const ROADS_DOWNLOADS_ALWAYS: Record<string, string[]> = {
  sa: [`${YEAR}/sa/mot-traffic-density.xlsx`],
  th: [`${YEAR}/th/drr-aadt-2024.csv`],
}
const ROADS_REQUIRED_CACHE: Record<string, string[]> = {
  br: [`${YEAR}/br/dnit-federal-highways.geojson`],
  cn: [`${YEAR}/cn/highway-network.geojson`],
  in: [`${YEAR}/in/bharatmala-roads.geojson`],
  jp: [`${YEAR}/jp`], // 47 manually-staged kasyo*.csv — dir presence pre-checks here; the enricher itself hard-fails unless all 47 parse non-empty (#31.2)
  mx: [`${YEAR}/mx/sict-datosviales/u1_segmentos.geojson`],
  ph: [`${YEAR}/ph/roads-classification.geojson`],
}
const ROADS_OPTIONAL_CACHE = new Set(['ar', 'bo', 'cl', 'co', 'ec', 'id', 'pe', 'py', 've'])

const RAIL_CACHED_DOWNLOAD: Record<string, string[]> = {
  ae: [`${YEAR}/ae/gtfs-stop-frequencies.json`],
  au: [`${YEAR}/au/gtfs-family-frequencies.json`],
  be: [`${YEAR}/be/gtfs-family-frequencies.json`],
  ca: [`${YEAR}/ca/gtfs-family-frequencies.json`],
  // czptt-train-sequences.json since the #26 graph-walk rewrite (2026-07-16);
  // the pre-#26 czptt-segment-counts.json is a fossil the enricher ignores.
  cz: [`${YEAR}/cz/czptt-train-sequences.json`],
  de: [`${YEAR}/de/gtfs-family-frequencies.json`],
  dk: [`${YEAR}/dk/gtfs-family-frequencies.json`],
  es: [`${YEAR}/es/renfe-stop-frequencies.json`],
  fi: [`${YEAR}/fi/gtfs-family-frequencies.json`],
  ie: [`${YEAR}/ie/gtfs-family-frequencies.json`],
  il: [`${YEAR}/il/gtfs-family-frequencies.json`],
  it: [`${YEAR}/it/gtfs-family-frequencies.json`],
  mx: [`${YEAR}/mx/gtfs-family-frequencies.json`],
  pl: [`${YEAR}/pl/gtfs-family-frequencies.json`],
  pt: [`${YEAR}/pt/gtfs-family-frequencies.json`],
  se: [`${YEAR}/se/gtfs-family-frequencies.json`],
  th: [`${YEAR}/th/gtfs-stop-frequencies.json`],
}
const RAIL_OPTIONAL_CACHE = new Set(['cn', 'in'])

const INDUSTRIAL_REQUIRED_CACHE: Record<string, string[]> = {
  // stampOneWinner FATAL guard: refuses to reset stamps it cannot replace.
  bo: [`${YEAR}/bo/power-plants-gem.geojson`],
  br: [`${YEAR}/br/wind-turbines.geojson`],
  cn: [`${YEAR}/cn/coal-plants.geojson`],
  co: [`${YEAR}/co/power-plants-gem.geojson`],
  ve: [`${YEAR}/ve/power-plants-ve360.geojson`],
  za: [`${YEAR}/za/power-plants-eskom.geojson`],
  // wind matchers with no download path at all (exit 1 when cache absent).
  de: [`${YEAR}/de/mastr-wind.csv`],
  us: [`global/uswtdb.csv`],
}
const INDUSTRIAL_CACHED_DOWNLOAD: Record<string, string[]> = {
  ca: [`${YEAR}/ca/wind-turbines-en.xlsx`],
  dk: [`${YEAR}/dk/ens-windturbines.xlsx`],
  es: [`${YEAR}/es/clm-aerogeneradores.csv`],
  no: [`${YEAR}/no/nve-vindturbiner.json`],
  se: [`${YEAR}/se/vindbrukskollen.zip`],
}

const BUILDINGS_CACHED_DOWNLOAD: Record<string, string[]> = {
  cz: [`${YEAR}/cz/ruian-buildings.json`],
  es: [`${YEAR}/es/catastro-buildings.json`],
}

/** Countries `enrich-railway-europe.ts` actually carries feeds for — DERIVED
 *  from its own `FEEDS` registry (2026-07-16 Phase 4), not a hand-maintained
 *  duplicate: the pre-Phase-4 version of this table was a literal copy-paste
 *  of the FEEDS country list with a comment warning it could drift, and the
 *  plan's own inventory undercounted europe's feed/country count once before
 *  (see the plan's Country inventory section) — importing the registry
 *  directly makes that drift structurally impossible. Despite the script's
 *  name this is NOT Europe-only (carries IN/US/CA/AU too). Used to (a) skip
 *  the step under a country/bbox scope it cannot touch and (b) floor each
 *  per-country invocation's `expectMinInputs` at that country's OWN feed
 *  count (`RAILWAY_EUROPE_FEED_COUNT_BY_COUNTRY`). */
const RAILWAY_EUROPE_FEED_COUNTRIES = new Set(EUROPE_FEEDS.map((f) => f.country.toLowerCase()))
const RAILWAY_EUROPE_FEED_COUNT_BY_COUNTRY: Record<string, number> = {}
for (const f of EUROPE_FEEDS) {
  const cc = f.country.toLowerCase()
  RAILWAY_EUROPE_FEED_COUNT_BY_COUNTRY[cc] = (RAILWAY_EUROPE_FEED_COUNT_BY_COUNTRY[cc] ?? 0) + 1
}
/** enrich-roads-europe.ts scan envelope (its EU_HEX_BBOX). */
const ROADS_EUROPE_BBOX: Bbox = [34, -32, 72, 45]
/** US CGAZ bbox is antimeridian-wide; USWTDB relevance test uses CONUS+AK+HI. */
const US_HEX_BBOX: Bbox = [17.5, -180.0, 71.5, -65.0]

// ── family notes (WHY + footguns; per-step overrides below) ─────────────────

const NOTE_ROADS_NATIONAL =
  'national road census → writeRoadAadt (coverage-gated, retract-capable). Runs before service-tree/continuity-fill: those heuristics need measured anchors.'
const NOTE_RAIL_NATIONAL =
  'national rail counts → writeRailTrains (service-skip + priority gate). retractSafe: the script retracts its own legacy stamps ONLY over a provably complete feed snapshot (v2 cache records feedsLoadedNonEmpty); an incomplete/missing feed withholds retract and logs one loud line — running cache-only is always safe.'
const NOTE_INDUSTRIAL_NATIONAL =
  'NACE stamping via lib/enrich-industrial-gem stampOneWinner (one facility, one polygon). FOOTGUN: never run `npm run gen:sources` while any chain step is in flight — source-id drift splits stamps across two ids (auditor R0). KNOWN GAP (#31.6, fix deferred into the #29 feed work): a GEM country whose power-plants-gem.geojson is missing runs as a silent no-op (stampOneWinner refuses to reset what it cannot re-stamp, so nothing is lost — but nothing is stamped either and the step still exits 0).'
const NOTE_BUILDINGS_NATIONAL =
  'national cadastre floors/type → writeBuildingEnrichment. A fresh extract resets buildings.arrow source_id to 0, so this must re-run even though rasters survive.'

// ── plan construction ────────────────────────────────────────────────────────

const isCc = (s: string) => /^[a-z]{2}$/.test(s)

function enumerateFamily(prefix: string): string[] {
  return readdirSync(PIPELINE_DIR)
    .filter((f) => f.startsWith(prefix) && f.endsWith('.ts') && !f.endsWith('.test.ts'))
    .map((f) => f.slice(prefix.length, -3))
    .sort()
}

function cacheState(paths: string[] | undefined): { present: boolean; missing: string } {
  if (!paths || paths.length === 0) return { present: true, missing: '' }
  for (const p of paths) {
    const full = resolve(ENRICH_DIR, p)
    if (existsSync(full) && (!statSync(full).isDirectory() || readdirSync(full).length > 0)) return { present: true, missing: '' }
  }
  return { present: false, missing: `data/enrichment/${paths[0]}` }
}

/** Applicability of a national step's country to the scope. Intersection runs
 *  per CGAZ POLYGON bbox (mainland/islands separately) — the union bbox of
 *  FR/GB/NO spans half the globe via overseas territories and would drag them
 *  into every bbox scope. 'unresolvable' = CGAZ has no feature for the cc
 *  (dependent territories like NC) → listed as a visible SKIP, never silent. */
function countryInScope(cc: string, scope: ResolvedScope): boolean | 'unresolvable' {
  if (scope.kind === 'world') return true
  if (scope.kind === 'country') return cc === scope.iso2!.toLowerCase()
  try {
    return scope.bboxes!.some((b) => countryIntersectsBbox(cc, b))
  } catch {
    return 'unresolvable'
  }
}

function nationalStep(
  family: 'roads' | 'railway' | 'buildings' | 'industrial',
  cc: string,
  scope: ResolvedScope,
  note: string,
): PlanStep {
  const layer: Layer = family === 'railway' ? 'railways' : family
  const spec =
    family === 'roads'
      ? { dl: ROADS_CACHED_DOWNLOAD[cc], always: ROADS_DOWNLOADS_ALWAYS[cc], req: ROADS_REQUIRED_CACHE[cc], opt: ROADS_OPTIONAL_CACHE.has(cc) }
      : family === 'railway'
        ? { dl: RAIL_CACHED_DOWNLOAD[cc], always: undefined, req: undefined, opt: RAIL_OPTIONAL_CACHE.has(cc) }
        : family === 'industrial'
          ? { dl: INDUSTRIAL_CACHED_DOWNLOAD[cc], always: undefined, req: INDUSTRIAL_REQUIRED_CACHE[cc], opt: false }
          : { dl: BUILDINGS_CACHED_DOWNLOAD[cc], always: undefined, req: undefined, opt: false }
  const args: string[] = []
  let skipReason: string | null = null
  if (spec.dl) {
    const c = cacheState(spec.dl)
    if (c.present) args.push('--enrich-only')
    else skipReason = `cache missing (${c.missing}) — chain never downloads mid-run; restore data/enrichment or run the script manually once`
  } else if (spec.always) {
    const c = cacheState(spec.always)
    if (!c.present) skipReason = `cache missing (${c.missing}) and script has no --enrich-only (would download mid-run)`
  } else if (spec.req) {
    const c = cacheState(spec.req)
    if (!c.present) skipReason = `required cache missing (${c.missing}) — script hard-exits without it`
  }
  const extra = spec.opt ? ' Cache-optional: missing GeoJSON degrades to the script\'s computed national defaults (logged by the script).' : ''
  return {
    id: `${family}-${cc}`,
    script: `enrich-${family}-${cc}.ts`,
    phase: 'national',
    layer,
    country: cc,
    args,
    notes: note + extra,
    skipReason,
  }
}

/** Resolve the full ordered plan for a scope. Throws when a pipeline file the
 *  manifest should own is not classified — completeness by construction. */
export function buildPlan(scope: ResolvedScope): { steps: PlanStep[]; excludedByScope: number } {
  const bboxes = scope.bboxes // null = world
  const steps: PlanStep[] = []
  let excludedByScope = 0

  // Completeness ledger: every enrich-*.ts must end up claimed exactly once.
  const claimed = new Set<string>()
  const claim = (script: string) => {
    if (!existsSync(resolve(PIPELINE_DIR, script))) throw new Error(`manifest references missing script ${script}`)
    claimed.add(script)
  }
  const push = (s: PlanStep) => {
    claim(s.script)
    steps.push(s)
  }
  const pushIf = (include: boolean, s: PlanStep) => {
    claim(s.script)
    if (include) steps.push(s)
    else excludedByScope++
  }
  /** Fan a bbox-parameterized step out over the scope's polygon bbox LIST —
   *  country scopes are per-polygon padded bboxes, NEVER the union envelope
   *  (FR's union covers 82% of the planet via overseas territories). Child
   *  scripts take one --bbox, so a multi-polygon scope becomes one step
   *  instance per bbox (`id@k`); the writers are idempotent, so a hex under
   *  two overlapping padded bboxes is at worst processed twice. The AUDITOR
   *  is the exception — it accepts repeated --bbox and dedups hexes itself,
   *  so the gate stays a single step with exact multiset semantics.
   *  `argsFor(null)` = the world-scope argument shape. */
  const pushPerBbox = (s: Omit<PlanStep, 'args'>, argsFor: (b: Bbox | null) => readonly string[]): void => {
    if (!bboxes || bboxes.length === 1) {
      push({ ...s, args: argsFor(bboxes ? bboxes[0] : null) })
      return
    }
    bboxes.forEach((b, k) => {
      push({ ...s, id: `${s.id}@${k + 1}`, args: argsFor(b), notes: `${s.notes} [scope polygon bbox ${k + 1}/${bboxes.length}]` })
    })
  }
  /** True when ANY scope bbox intersects `envelope` (world = always). */
  const scopeIntersects = (envelope: Bbox): boolean => !bboxes || bboxes.some((b) => bboxIntersects(envelope, b))

  // ── column-parents ─────────────────────────────────────────────────────────
  pushPerBbox(
    {
      id: 'roads-built-up',
      script: 'enrich-roads-built-up.ts',
      phase: 'column-parents',
      layer: 'roads',
      country: null,
      notes:
        'built_up (u8 rural/urban) has NO extractor parent — first step always, engine + taper consume it. Fails loud when the Overture obstacle store or its .ingested-tiles manifest is absent. Idempotent: re-run over an osm-to-h3r4.sh-tail run is byte-identical.',
      skipReason: null,
      // The osm-to-h3r4.sh tail (RUN_SERVICE_TREE=1) already ran it on a fresh
      // extract — --assume-fresh-extract skips the byte-identical re-run.
      skipIf: 'ran-by-extract-tail',
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : []),
  )
  pushPerBbox(
    {
      id: 'roads-country',
      script: 'enrich-roads-country.ts',
      phase: 'column-parents',
      layer: 'all',
      country: null,
      notes:
        'M3 bake: per-segment country_iso/city_id/continent into roads.arrow AND railways.arrow (AdminAt midpoint resolution; all-or-none triplet + country_baked_v1 contract stamp, column-presence read-back assertion). FATAL when a file\'s claimed-land rows resolve 100% UNKNOWN (the G1 trap — file left unwritten). Needs the CGAZ caches — osm-to-h3r4.sh warms them before its tail run; a cold manual run derives them on demand (curl + GDAL). Idempotent: byte-identical re-runs.',
      skipReason: null,
      // The osm-to-h3r4.sh tail (RUN_SERVICE_TREE=1) already ran it on a fresh
      // extract — --assume-fresh-extract skips the byte-identical re-run.
      skipIf: 'ran-by-extract-tail',
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : []),
  )

  // BEFORE every industrial claimer INCLUDING global-industrial (round-3
  // Codex: GPPD same-rank-lower-id cannot overwrite 330, so healing after it
  // strands the row empty until the NEXT chain run — 74 such rows found):
  // orphaned shared-id stamps (auditor R14) are unreachable by every national
  // pass since #31, so only this sweep can clear them, and GPPD must get its
  // chance at the freed rows in this same run.
  pushPerBbox(
    {
      id: 'industrial-heal-orphans',
      script: 'heal-industrial-orphans.ts',
      phase: 'global-priors',
      layer: 'industrial',
      country: null,
      notes:
        'clears shared-id (330) stamps whose centroid lies in no CGAZ country (auditor R14 write-side twin) — post-#31 passes cannot create new ones (countryGate scopes stamp+reset), so this drains legacy orphans once and then stays a no-op.',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : ['--world']),
  )
  // ── global-priors ──────────────────────────────────────────────────────────
  {
    const c = cacheState(['global/gppd.csv'])
    push({
      id: 'global-industrial',
      script: 'enrich-global-industrial.ts',
      phase: 'global-priors',
      layer: 'industrial',
      country: null,
      args: c.present ? ['--enrich-only'] : [],
      notes:
        'GPPD + E-PRTR + GEM trackers → nace_4digit worldwide. Per-source completeness floors: a source participates fully or not at all. Never pass --cells in a chain run (verification isolation only).',
      skipReason: c.present ? null : 'cache missing (data/enrichment/global/gppd.csv) — chain never downloads mid-run',
    })
  }
  {
    const c = cacheState(['global/uswtdb.csv'])
    const usRelevant = scope.kind === 'country' ? scope.iso2 === 'US' : scopeIntersects(US_HEX_BBOX)
    push({
      id: 'global-windturbines',
      script: 'enrich-global-windturbines.ts',
      phase: 'global-priors',
      layer: 'industrial',
      country: null,
      args: c.present ? ['--enrich-only'] : [],
      notes: 'USWTDB hub_height + rated_power_kw onto source_type=10 rows (US-only data despite the global name).',
      skipReason: !usRelevant
        ? 'USWTDB is US-only — a no-op under this scope'
        : c.present
          ? null
          : 'cache missing (data/enrichment/global/uswtdb.csv) — chain never downloads mid-run',
    })
  }
  push({
    id: 'global-dem',
    script: 'enrich-global-dem.ts',
    phase: 'global-priors',
    layer: 'rasters',
    country: null,
    args: [],
    notes: 'Copernicus GLO-30 DEM under prepared/dem/copernicus.',
    skipReason: 'raster-side one-time global enrichment — DEM is year-shared and survives an OSM re-extract; run manually via /enrich-global',
  })
  pushPerBbox(
    {
      id: 'obstacle-heights',
      script: 'enrich-obstacle-heights.ts',
      phase: 'global-priors',
      layer: 'buildings',
      country: null,
      notes:
        'height-tier ladder for prepared obstacles.arrow SCREENING heights (tier 3 city-measured zonal — IPR Praha; tier 4 GHS-BUILT-H ANBH prior replacing the flat 8 m default; ladder table in the enricher headers + noise_compute::low_profile). Every promoted cell carries an adjacent proof bound to its output inode, immutable Overture staging shards, height rasters, and worker source. Current cells are a cheap no-op; Overture re-promotion, raster replacement, staging change, or worker change makes the cell provably stale and regenerates it. Missing staging or a required height raster fails the chain rather than certifying a reverted cell.',
      skipReason: null,
    },
    (b) => (b ? ['--enrich-only', '--bbox', serializeBbox(b)] : ['--enrich-only']),
  )
  // PRE-heal, deliberately BEFORE every ROAD claimer incl. roads-europe (/gg
  // #33 Codex CRITICAL — same one-cycle hole flagged for rail): a legacy
  // foreign/zero stamp outranks the EU continental claimer via shouldOverwrite,
  // so healing AFTER roads-europe strands the row a whole run before the right
  // source can re-claim it. Heal first → the freed rows re-claim in this run.
  pushPerBbox(
    {
      id: 'road-heal-country-bleed',
      script: 'heal-road-country-bleed.ts',
      phase: 'global-priors',
      layer: 'roads',
      country: null,
      notes:
        'registry-driven disown of national ROAD stamps outside their own country (auditor R9 write-side twin, #33) — runs BEFORE every road claimer so the freed rows are re-claimed (correct country / heuristics) in this same run. writeRoadAadt auto-gates the STAMP side from each national id, so this drains legacy bbox-bleed once and then stays a no-op.',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : ['--world']),
  )
  pushPerBbox(
    {
      id: 'road-heal-zero-write',
      script: 'heal-road-zero-write.ts',
      phase: 'global-priors',
      layer: 'roads',
      country: null,
      notes:
        'clears measured-source rows with all-zero AADT (auditor R7 write-side twin, #33) — legacy pre-#31.4-guard rows (de-bast/es/pl). Post-guard writers reject the shape, so this too drains once and stays a no-op; runs BEFORE every road claimer so the freed rows re-claim.',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : ['--world']),
  )
  {
    const inEnvelope = scopeIntersects(ROADS_EUROPE_BBOX)
    const c = cacheState(['global/eu-city-traffic'])
    push({
      id: 'roads-europe',
      script: 'enrich-roads-europe.ts',
      phase: 'global-priors',
      layer: 'roads',
      country: null,
      args: c.present ? ['--enrich-only'] : [],
      expectMinInputs: 36, // #31.6: all 36 treated cities must load (enricher now normalizes the staged raws under --enrich-only)
      notes:
        'EU 36-city segment traffic (continental-measured) — the prior national censuses overwrite. Cache is NOT year-stamped (data/enrichment/global/eu-city-traffic). #31.6: --enrich-only now normalizes the staged `<City>_<METRIC>_<YEAR>.geojson` raws into the consumed `<city>.geojson` so all 36 load offline (was 2/36); the enricher emits a QM_COMPLETENESS marker and expectMinInputs floors it so a partial fragment can no longer stamp as done.',
      skipReason: !inEnvelope
        ? 'scope outside the EU city-traffic envelope [34,-32,72,45]'
        : c.present
          ? null
          : 'cache missing (data/enrichment/global/eu-city-traffic) — chain never downloads mid-run',
    })
  }
  // PRE-heal, deliberately BEFORE every rail claimer (/gg Codex CRITICAL): the
  // old post-claimer position could only retract — a stale foreign stamp (PL
  // rows in SK) was zeroed AFTER railway-europe had already run past it, so
  // nothing could reclaim the row until the NEXT chain run. Healing first
  // frees foreign rows while the claimers are still ahead in this very run.
  // rail-heal-verify (heuristics) is the post-claimer tripwire twin.
  pushPerBbox(
    {
      id: 'rail-heal-country-bleed',
      script: 'heal-rail-country-bleed.ts',
      phase: 'global-priors',
      layer: 'railways',
      country: null,
      notes:
        'registry-driven disown of national rail stamps outside their own country (auditor R9 write-side twin) — runs BEFORE railway-europe + national rail so the claimers can immediately re-claim the freed rows in this same run.',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : ['--world']),
  )
  {
    const c = cacheState(['global/gtfs'])
    const cacheSkipReason = c.present ? null : 'cache missing (data/enrichment/global/gtfs) — chain never downloads mid-run'
    if (scope.kind === 'world') {
      // World mode: the enricher's OWN all-countries loop (processCountry
      // per country, sequentially) plus the CRITICAL-2 legacy retract sweep
      // — unchanged since before Phase 4.
      push({
        id: 'railway-europe',
        script: 'enrich-railway-europe.ts',
        phase: 'global-priors',
        layer: 'railways',
        country: null,
        args: c.present ? ['--enrich-only'] : [],
        // #31.6 floor DERIVED from the registry (2026-07-16 /gg fix batch
        // item 7 — the hand-written 23 here drifted the moment FEEDS changed):
        // every registered GTFS feed must load non-empty; the enricher emits
        // QM_COMPLETENESS so a partial (au-vic yields 0) is recorded, not
        // silently stamped.
        expectMinInputs: EUROPE_FEEDS.length,
        notes:
          `Continental GTFS aggregate (${EUROPE_FEEDS.length} feeds incl. IN/US/CA/AU — one shared source id), run as the enricher's own all-countries SEQUENTIAL loop (2026-07-16 Phase 4). NEVER pass --feed in a chain run: any subset forces retract-unsafe for the whole pass. Cache is NOT year-stamped (data/enrichment/global/gtfs). #31.6: the enricher emits a QM_COMPLETENESS marker (feeds-loaded/${EUROPE_FEEDS.length}, aggregated across every country) and this floor records the gap — au-vic (nested PTV zip-of-zips) currently loads 0, so a full run reports one short of the floor. FLAT per-feed cache layout accepted since #31.`,
        skipReason: cacheSkipReason,
      })
    } else {
      // Per-country fan-out (2026-07-16 Phase 4 / /gg review item 7): a
      // country:XX (or bbox) scope must scope the europe aggregate to XX's
      // own feed(s) + a border-buffered rail graph via `--country CC`
      // (`processCountry` — the SAME path the all-countries loop uses), not
      // mutate the whole continent + run the world-wide CRITICAL-2 sweep.
      // Mirrors nationalStep's own scope-resolution precedence: a country
      // with no CGAZ feature is listed with an explicit skip (never silent);
      // a country simply outside this scope is excluded entirely, like any
      // national step for a country outside the scope.
      for (const cc of [...RAILWAY_EUROPE_FEED_COUNTRIES].sort()) {
        const feedCount = RAILWAY_EUROPE_FEED_COUNT_BY_COUNTRY[cc] ?? 1
        const step: PlanStep = {
          id: `railway-europe-${cc}`,
          script: 'enrich-railway-europe.ts',
          phase: 'global-priors',
          layer: 'railways',
          country: cc,
          args: [...(c.present ? ['--enrich-only'] : []), '--country', cc.toUpperCase()],
          expectMinInputs: feedCount,
          notes:
            `per-country invocation (2026-07-16 Phase 4): scopes to ${cc.toUpperCase()}'s own ${feedCount} feed(s) + a 0.5°-buffered rail graph via the enricher's --country flag — fixes /gg item 7 (a country:XX chain scope used to run the WHOLE 23-feed aggregate + the world-wide retract sweep). The CRITICAL-2 legacy OLD_FALLBACK sweep runs ONLY under world scope, never here.`,
          skipReason: cacheSkipReason,
        }
        const inScope = countryInScope(cc, scope)
        if (inScope === 'unresolvable') {
          step.skipReason = `country '${cc.toUpperCase()}' has no CGAZ ADM0 feature (dependent territory) — bbox scoping cannot place it; runs under --scope world or country:${cc.toUpperCase()}`
          push(step)
        } else {
          pushIf(inScope, step)
        }
      }
    }
  }

  // ── national (roads → railways → buildings → industrial, alpha per family) ─
  const perStepNote: Record<string, string> = {
    'railway-cz':
      'graph-walk path onto the shared enrichRailwaysByGraphWalk driver (migration #26 complete, 2026-07-16): CZPTT sequences are paired AFTER GPS resolution and walked across the CZ rail graph. Still stamps SOURCE_ID_CZ_TIMETABLE_SILENT (2 pax/1 frt) on failure-free components the walk never reaches, and parallel_divisor via the walk\'s own lateral spread (no more czpttKey) — retract/silent/divisor all gated on retractSafe AND enableDestructive (--stamp-only forces the latter false for Phase-3 Step A).',
    'railway-kr': 'pure retract heal (KR publishes no rail data; match() never stamps). Deliberately no retractSafe gate — no inputs exist to be incomplete.',
    'roads-ru': 'env RU_BBOX="S,W,N,E" can narrow a resume — chain runs it unset (full RU).',
    'roads-mx': 'runs its own clearStaleStamps() pre-pass (manual withArrowWrite) before matching.',
    'industrial-de': 'docstring advertises --enrich-only but the flag is dead code — script always reads the pre-staged cache, exits 1 without it.',
    'industrial-us': 'docstring advertises --enrich-only but the flag is dead code — reads data/enrichment/global/uswtdb.csv (same file as global-windturbines), exits 1 without it.',
  }
  const families: Array<['roads' | 'railway' | 'buildings' | 'industrial', string, string]> = [
    ['roads', 'enrich-roads-', NOTE_ROADS_NATIONAL],
    ['railway', 'enrich-railway-', NOTE_RAIL_NATIONAL],
    ['buildings', 'enrich-buildings-', NOTE_BUILDINGS_NATIONAL],
    ['industrial', 'enrich-industrial-', NOTE_INDUSTRIAL_NATIONAL],
  ]
  for (const [family, prefix, note] of families) {
    for (const suffix of enumerateFamily(prefix)) {
      if (!isCc(suffix)) continue // specials (built-up, europe, name-heuristic, …) are explicit steps
      const s = nationalStep(family, suffix, scope, note)
      const override = perStepNote[s.id]
      if (override) s.notes = `${s.notes} ${override}`
      const inScope = countryInScope(suffix, scope)
      if (inScope === 'unresolvable') {
        s.skipReason = `country '${suffix}' has no CGAZ ADM0 feature (dependent territory) — bbox scoping cannot place it; runs under --scope world or country:${suffix.toUpperCase()}`
        push(s)
      } else {
        pushIf(inScope, s)
      }
    }
  }

  // ── national specials (explicit steps — their suffix is not a cc) ─────────
  // TH from-to name matcher: claims ref-less segments the exact-ref census
  // matcher (roads-th) cannot claim — same DRR census, same measured source
  // id, so it must run AFTER the national family. Cache rule mirrors the
  // family's `spec.always` branch: no cache → visible skip, NEVER download
  // mid-run (`ROADS_DOWNLOADS_ALWAYS`, not the --enrich-only map).
  {
    const thNamesCache = cacheState(ROADS_DOWNLOADS_ALWAYS['th'])
    pushIf(countryInScope('th', scope) === true, {
      id: 'roads-th-names',
      script: 'enrich-roads-th-names.ts',
      phase: 'national',
      layer: 'roads',
      country: 'th',
      args: ['--write'],
      notes:
        'DRR from-to name matching for ref-less TH roads (the Koh Phangan class) — held-out precision gate (0 FPs and ≥150 claims) enforced by the script itself; ambiguity → no match.',
      skipReason: thNamesCache.present
        ? null
        : `cache missing (${thNamesCache.missing}) — chain never downloads mid-run; restore data/enrichment or run the script manually once`,
    })
  }

  // ── city ───────────────────────────────────────────────────────────────────
  for (const city of CITY_DATASETS) {
    const include =
      scope.kind === 'world' ||
      (scope.kind === 'country' ? city.iso2 === scope.iso2!.toLowerCase() : scopeIntersects(city.hexBbox))
    pushIf(include, {
      id: `city-${city.slug}`,
      script: 'enrich-cities-roads.ts',
      phase: 'city',
      layer: 'roads',
      country: city.iso2,
      args: ['--city', city.slug, '--enrich-only'],
      notes:
        'municipal counters (city-measured rank 90) — polygon-gated, outranks national INSIDE the ADM2 boundary only. Sequential after national by design: the driver is named outside the enrich-roads-* glob so bench parallel runs cannot race it over the same hex locks.',
      skipReason: city.enabled ? null : 'disabled in lib/city-datasets.ts',
    })
  }

  // ── heuristics ─────────────────────────────────────────────────────────────
  pushPerBbox(
    {
      id: 'roads-service-tree',
      script: 'enrich-roads-service-tree.ts',
      phase: 'heuristics',
      layer: 'roads',
      country: null,
      notes:
        'building-driven AADT for local classes 5-9, gated by shouldOverwrite precedence (claims empty/self/lower-rank rows; census-claimed locals are never touched) — AFTER national/city. REQUIRES data/prepared/h3r4-admin.bin (fail-closed: national vehicle-mix + trips/dwelling resolve per hex country; osm-to-h3r4.sh builds the table BEFORE this pass). Rates/mix come from committed generated tables (lib/trip-rates.ts, lib/country-fleet.generated.ts) — NEVER regenerate country-fleet mid-chain, half the hexes would carry the old mix. osm-to-h3r4.sh tail already runs it sharded on a fresh extract; a re-run is idempotent (use --from to skip when the extract is hours old). Never run two overlapping-scope chains concurrently: processHex holds the per-hex arrow lock across its whole compute (minutes on dense hexes) and the second process would hit the 5-minute lock timeout.',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : []),
  )
  pushPerBbox(
    {
      id: 'roads-continuity-fill',
      script: 'enrich-roads-continuity-fill.ts',
      phase: 'heuristics',
      layer: 'roads',
      country: null,
      notes:
        'flow-redistribution from MEASURED anchors across junction gaps (majors 0-4 + links) — REQUIRES every national + city anchor stamped first (rerun-measured Phase 4 contract). Anchors are isMeasured() only, so it can never chain off its own output.',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : []),
  )
  // Post-claimer tripwire twin of the global-priors PRE-heal: report-only,
  // exits 1 when the heal WOULD retract anything — i.e. some rail claimer
  // re-stamped foreign rows after the pre-heal freed them. The fix is always
  // the offending enricher, never a second silent heal (which would just
  // re-zero rows every run and hide the bug).
  pushPerBbox(
    {
      id: 'rail-heal-verify',
      script: 'heal-rail-country-bleed.ts',
      phase: 'heuristics',
      layer: 'railways',
      country: null,
      notes:
        'read-only --verify pass of heal-rail-country-bleed AFTER all rail claimers: exit 1 = a claimer wrote country-bleed rows in this run (auditor R9 would fire) — fix that enricher and re-run.',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b), '--verify'] : ['--world', '--verify']),
  )
  pushPerBbox(
    {
      id: 'road-heal-verify',
      script: 'heal-road-country-bleed.ts',
      phase: 'heuristics',
      layer: 'roads',
      country: null,
      notes:
        'read-only --verify pass of heal-road-country-bleed AFTER all road claimers: exit 1 = a claimer wrote country-bleed rows this run (writeRoadAadt auto-gate should make this impossible — a nonzero exit means a bespoke road writer bypassed the shared writer).',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b), '--verify'] : ['--world', '--verify']),
  )
  pushPerBbox(
    {
      id: 'railways-parallel',
      script: 'enrich-railways-parallel.ts',
      phase: 'heuristics',
      layer: 'railways',
      country: null,
      notes:
        'shared OSM-corridor parallel-track divisor, only-raise-from-1 rule, for sources NOT flagged railDivisorFromWalk in enrichment-datasets.ts — walk-managed sources (cz-szcd-gtfs, cz-timetable-silent) get their divisor from the graph-walk driver\'s own lateral spread and this pass skips their rows entirely (czpttKey identity is gone; migration #26).',
      skipReason: null,
    },
    (b) => (b ? ['--bbox', serializeBbox(b)] : ['--world']),
  )
  push({
    id: 'industrial-name-heuristic',
    script: 'enrich-industrial-name-heuristic.ts',
    phase: 'heuristics',
    layer: 'industrial',
    country: null,
    args: [],
    notes:
      'name-keyword → NACE mop-up (heuristic rank — can never beat national/GEM stamps, order-independent by rank design). No bbox support: ALWAYS a global scan, even under a country scope. Executes main() on import — spawn only.',
    skipReason: null,
  })

  // ── taper ──────────────────────────────────────────────────────────────────
  {
    const czRelevant =
      scope.kind === 'world' ||
      (scope.kind === 'country' ? scope.iso2 === 'CZ' : scope.bboxes!.some((b) => countryIntersectsBbox('CZ', b)))
    pushPerBbox(
      {
        id: 'roads-taper',
        script: 'enrich-roads-taper.ts',
        phase: 'taper',
        layer: 'roads',
        country: null,
        notes:
          'R7 transition taper — ABSOLUTELY LAST road write (ramps derive from the final AADT/speed state; the shared writer voids speed_taper on any later overwrite). Writes only src=0 untagged rows; v1 is CZ-polygon-gated internally.',
        skipReason: czRelevant ? null : 'taper v1 mirrors the engine CZ resolution only (module doc) — no CZ hexes in this scope',
      },
      (b) => [...(b ? ['--bbox', serializeBbox(b)] : ['--bbox', '-90,-180,90,180']), '--write'],
    )
  }

  // ── gate ───────────────────────────────────────────────────────────────────
  // SINGLE step even for multi-polygon scopes: the auditor accepts repeated
  // --bbox and dedups hexes internally, so the fingerprint multiset counts
  // every violation exactly once. run.ts appends the machine-output plumbing
  // (--ndjson/--summary-json) at spawn time — paths live in the per-run log
  // dir the manifest cannot know. I/O damage is exit 3 by the auditor's own
  // DEFAULT (--lenient-io exists for exploratory scans, never passed here).
  push({
    id: 'gate-invariants',
    script: 'audit-enrichment-invariants.ts',
    phase: 'gate',
    layer: 'all',
    country: null,
    args: bboxes ? bboxes.flatMap((b) => ['--bbox', serializeBbox(b)]) : ['--bbox', '-90,-180,90,180'],
    notes:
      'READ-ONLY acceptance gate (rules R0-R16, incl. the rail continuity R15 flow-jump / R16 continuity-gap detectors). run.ts diffs the per-violation fingerprint MULTISET against pipeline/chain/gate-baselines/{year}.{scope}.json (one file per DATA_YEAR + exact scope): any fingerprint above its baseline count FAILS the chain; pre-existing ones pass with a warning; exit 3 (I/O damage) always fails.',
    skipReason: null,
  })

  // ── completeness: no enrich-*.ts may stay unclassified ─────────────────────
  const all = readdirSync(PIPELINE_DIR).filter((f) => /^enrich-.+\.ts$/.test(f) && !f.endsWith('.test.ts'))
  const unclaimed = all.filter((f) => !claimed.has(f))
  if (unclaimed.length > 0) {
    throw new Error(
      `manifest completeness violated — unclassified enrichment script(s): ${unclaimed.join(', ')}. ` +
        `Add them to a phase (or an explicit skip step) in pipeline/chain/manifest.ts.`,
    )
  }

  return { steps, excludedByScope }
}
