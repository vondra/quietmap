/**
 * Rail graph-walk enrichment DRIVER — the I/O half of the rail-walk matcher
 * matcher. `rail-graph.ts` + `rail-graph-metrics.ts` are the pure
 * topology/routing SSOT (no file I/O); this module owns everything they
 * deliberately don't: enumerating hexes, reading `railways.arrow`, building
 * the ONE cross-hex graph for a scope, running `writeRailTrains` per hex
 * behind the shared walk result, and the rail-stops sidecar the R15/R16
 * stop exemption reads.
 *
 * Per-pair quarantine (2026-07-16 Step-B refinement + same-day redesign,
 * replaces the per-component gating this module used until then): the
 * silent residual and the destructive retract arm are gated on
 * `RailWalkResult.quarantinedSegmentKeys` — per failure kind, the union of
 * the pair's candidate paths (ambiguous) or a chord-vicinity band + capped
 * graph fingers (snapFailed/disconnected/detourRejected; unlocalized pairs
 * get the band alone) — never on the pair's WHOLE graph component.
 * Per-component withholding was verified on a live CZ Step-A run to
 * quarantine the entire national network behind 150 failed pairs out of
 * 2 461, because rail is one connected component nationwide (9 failed
 * components included the main CZ mainline, 31 245 km); the per-pair shapes
 * are strictly tighter evidence and no longer need a separate
 * global-suppression case for unlocalized pairs either — see
 * `rail-graph-metrics.ts`'s module doc and `RailWalkResult`'s doc
 * (rail-graph.ts) for the full rationale.
 *
 * `enableDestructive` gates `retract` (in full, including the `bleedGate`
 * sub-arm below) AND the silent residual — divisors ride with every accepted
 * `match` (walk-stamped or silent) in EVERY mode, since `RailTrains.divisor`
 * is part of the atomic counts+divisor write `writeRailTrains` itself now
 * owns. In stamp-only
 * (`enableDestructive=false`), NO `RailRetract` object is even constructed —
 * country-bleed healing there is deferred to `heal-rail-country-bleed.ts` or
 * the next destructive run of this same driver (2026-07-16 /gg review item
 * 3): the old design ran the bleed + the always-on service-arm auto-heal
 * regardless of `enableDestructive`, silently mutating data in a mode
 * documented as inspection-safe.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { iterateCountryHexes } from './roads-arrow.js'
import { coordKey4dp } from './spatial.js'
import {
  buildRailGraph, snapToNearestRailGraphNode,
  type RailGraph, type RailGraphSegmentInput, type RailStationPairCount, type RailWalkResult,
  type RailStopsSidecarV1,
  isWalkableRailType,
} from './rail-graph.js'
import { walkRailStationPairs } from './rail-graph-metrics.js'
import { writeRailTrains, type RailRow, type RailTrains } from './railways-arrow.js'
import { segmentWhollyOutside } from './country-polygon.js'
import { isNationallyOwnedSource } from './sources.js'

export interface RailWalkEnrichOptions {
  h3r4Dir: string
  /** [minLat, minLon, maxLat, maxLon] hex prefilter; hexes enumerated via
   *  `iterateCountryHexes(h3r4Dir, bbox, 'railways.arrow')`. */
  bbox: readonly [number, number, number, number]
  pairs: RailStationPairCount[]
  sourceId: number
  /** Passed straight to `writeRailTrains` (the #31.7 national-ownership gate).
   *  Also bounds this run's RETRACT testimony: a row wholly outside it is
   *  outside what this run's feeds testify about, so the ordinary
   *  quarantine-gated retract arm never disowns it — only `bleedGate` (when
   *  provided) can. */
  countryGate?: (lat: number, lon: number) => boolean
  /** Geometry-evidence gate for the BLEED retract arm (2026-07-16 /gg fix
   *  batch item 1). When provided, a row wholly outside THIS gate is
   *  disowned on geometry alone (bypassing `retractSafe`/quarantine — but
   *  never `enableDestructive`); when omitted, the bleed arm is OFF
   *  entirely. Contract: a nationally-owned id passes its own country gate
   *  here (CZ does); a SHARED multi-country id must pass the UNION of every
   *  territory the id may legitimately stamp — a single country's gate would
   *  disown rows a neighbouring country's run legitimately stamped under the
   *  SAME id (the CH-run-kills-southern-DE-rows bug) — or omit it and leave
   *  foreign healing to a dedicated sweep (heal-rail-country-bleed.ts). */
  bleedGate?: (lat: number, lon: number) => boolean
  /** Per-hex extra match arm (tram stop-join etc.) — consulted only for rows
   *  the walk did not stamp. Same shape as a `writeRailTrains` match closure. */
  extraMatch?: (row: RailRow, i: number, hexId: string) => RailTrains | null
  /** CZ option-b residual. Applied ONLY when destructive ops are enabled AND
   *  `retractSafe` holds AND the row sits outside EVERY failed pair's
   *  quarantine region (`RailWalkResult.quarantinedSegmentKeys` — the
   *  per-pair evidence shape: candidate-path union / chord band + fingers,
   *  see rail-graph.ts). The
   *  divisor actually stamped rides from `walk.divisorBySegmentKey`, not
   *  from this option — a quiet double-track must render at its true
   *  (2+1)/2, never the undivided (2+1)/1. */
  silentResidual?: { sourceId: number; pax: number; frt: number }
  /** Ids this driver may disown. Retract (incl. the `bleedGate` sub-arm)
   *  fires ONLY when `enableDestructive` is true — see module doc; in
   *  stamp-only NO retract object is even built. Within a destructive run:
   *  ordinary rows need `retractSafe` AND freedom from every failed pair's
   *  quarantine region; the bleed arm (row wholly outside `bleedGate`)
   *  bypasses BOTH of those (but still requires `enableDestructive`) —
   *  geometry is its own evidence the id never legitimately owned that row. */
  retract?: { sourceIds: readonly number[] }
  /** Caller's input-completeness verdict (feeds loaded non-empty etc.). */
  retractSafe: boolean
  /** Phase-3 Step A: stamp-only proof mode — when false, NO retract object is
   *  constructed at all (no bleed heal, no quarantine-gated reclaim
   *  check, no always-on service-arm auto-heal either) and NO silent residual
   *  fires. Walk-stamped rows still land with their FULL atomic write
   *  (pax/frt/sourceId AND divisor) in every mode — stamp-only only means
   *  "no retract, no silent", not "no divisor". */
  enableDestructive: boolean
  /** Sidecar provenance: scope slug (filename), extract fingerprint, feed list. */
  sidecar: { scope: string; extractFingerprint: string; feeds: string[] }
}

export interface RailWalkEnrichStats {
  hexes: number
  rows: number
  /** Rows the WALK itself stamped (a train count landed via `pairs` + the graph). */
  stamped: number
  /** Rows disowned this run — bleed-gate arm + quarantine-gated destructive retract, combined. */
  retracted: number
  /** Rows stamped by the CZ option-b silent residual (never the walk, never `extraMatch`). */
  silentStamped: number
  skippedService: number
  skippedForeign: number
  /** Rows a stamp candidate existed for but was withheld because the row's
   *  EXISTING source id is ANOTHER country's nationally-owned data (the
   *  foreign-national stamp guard — see the match closure below and
   *  `sources.ts::isNationallyOwnedSource`; DE Step A v2 Codex review item
   *  4, 2026-07-16). A persistently non-zero count on a national run means
   *  its graph scope reaches across a border into a neighbour's finished
   *  territory — expected for border-straddling rows, worth investigating
   *  when large. */
  skippedForeignNational: number
  walk: {
    failures: RailWalkResult['failures']
    /** Pairs where NEITHER end snapped onto the graph — stats-only telemetry
     *  (2026-07-16 Step-B refinement dropped the old global silent-residual
     *  suppression; see module doc). */
    unlocalizedPairs: number
    pairsWalked: number
    pairsTotal: number
  }
  /** Track-km of stampable rows inside `RailWalkResult.quarantinedSegmentKeys`
   *  — the "how much is still stuck" figure retract/silent actually withhold
   *  on now, computed from the same segments the graph was built from. */
  quarantinedKm: number
  /** Track-km of ALL stampable rows in the scope's graph — the denominator
   *  that makes `quarantinedKm` readable on its own. */
  stampableKm: number
  /** '' when nothing was written this run (never-cache-empty; see
   *  `writeStopsSidecarIfAny`) — a caller must not treat this as a path. */
  sidecarPath: string
}

/** `${hexId}:${rowIndex}` — the SAME key format `RailGraphSegmentInput.key`
 *  uses, so a stamp/component lookup from the graph or the walk result maps
 *  back to exactly one row across the whole scope with no separate index. */
function segmentKey(hexId: string, rowIndex: number): string {
  return `${hexId}:${rowIndex}`
}

/** Phase 1 (graph inputs): reads every hex's `railways.arrow` ONCE and keeps
 *  only what `buildRailGraph` needs. Per the plan (+ the 2026-07-16
 *  narrow-gauge extension, see `isWalkableRailType`): rows with a walkable
 *  family (heavy rail 0, narrow gauge 3) and `service===0` are stampable
 *  graph edges; `service===4` crossovers enter traversal-only (connect
 *  topology, never stamped); every other family/service combination never
 *  enters the graph at all — a tram can neither carry nor bridge a train
 *  count. */
function collectGraphSegments(h3r4Dir: string, hexIds: readonly string[]): RailGraphSegmentInput[] {
  const segments: RailGraphSegmentInput[] = []
  for (const hexId of hexIds) {
    const path = resolve(h3r4Dir, hexId, 'railways.arrow')
    const table = tableFromIPC(readFileSync(path))
    const n = table.numRows
    const sLat = table.getChild('start_lat')
    const sLon = table.getChild('start_lon')
    if (!sLat || !sLon) continue // malformed hex — mirrors writeRailTrains's own guard
    const eLat = table.getChild('end_lat')
    const eLon = table.getChild('end_lon')
    const lenCol = table.getChild('length_m')
    const rtCol = table.getChild('rail_type')
    const usCol = table.getChild('usage')
    const svcCol = table.getChild('service')
    const nmCol = table.getChild('name')
    const refCol = table.getChild('ref')
    const osmIdCol = table.getChild('osm_id')

    for (let i = 0; i < n; i++) {
      const railType = (rtCol?.get(i) as number) ?? 0
      const service = (svcCol?.get(i) as number) ?? 0
      const isTraversalOnly = service === 4
      if (!isTraversalOnly && !(isWalkableRailType(railType) && service === 0)) continue // never in the graph

      const startLat = sLat.get(i) as number
      const startLon = sLon.get(i) as number
      const endLat = (eLat?.get(i) as number) ?? startLat
      const endLon = (eLon?.get(i) as number) ?? startLon
      const ref = (refCol?.get(i) as string | null) ?? ''
      const name = (nmCol?.get(i) as string | null) ?? ''
      segments.push({
        key: segmentKey(hexId, i),
        osmId: String(osmIdCol?.get(i) ?? ''),
        railType,
        usage: (usCol?.get(i) as number) ?? 0,
        isTraversalOnly,
        corridorToken: ref || name || '',
        startLat, startLon, endLat, endLon,
        lengthM: (lenCol?.get(i) as number) ?? 0,
      })
    }
  }
  return segments
}

/** Track-km of ALL stampable (non-traversal) rows in the scope's graph —
 *  the denominator that makes `computeQuarantinedKm`'s output readable. */
function computeStampableKm(segments: readonly RailGraphSegmentInput[]): number {
  let km = 0
  for (const s of segments) if (!s.isTraversalOnly) km += s.lengthM / 1000
  return km
}

/** Track-km of stampable rows sitting inside
 *  `RailWalkResult.quarantinedSegmentKeys` — the figure retract/silent
 *  actually withhold on now (2026-07-16 Step-B refinement). */
function computeQuarantinedKm(
  segments: readonly RailGraphSegmentInput[],
  quarantinedSegmentKeys: ReadonlySet<string>,
): number {
  let m = 0
  for (const seg of segments) {
    if (seg.isTraversalOnly) continue
    if (quarantinedSegmentKeys.has(seg.key)) m += seg.lengthM
  }
  return m / 1000
}

/** Deduped (4dp) GPS of every `pairs` endpoint that resolves onto the graph —
 *  the rail-stops sidecar (R15/R16 stop exemption). Snap success is checked
 *  per ENDPOINT, independent of whether that endpoint's own OD pair's walk
 *  succeeded: a real platform a rider boards at is evidence for the
 *  auditor's stop exemption whether or not the far end of one particular
 *  trip snapped too — this is exactly why `snapToNearestRailGraphNode` is
 *  public API here, not only something `walkRailStationPairs` calls
 *  internally. */
function collectSnappedStops(graph: RailGraph, pairs: readonly RailStationPairCount[]): Array<{ lat: number; lon: number }> {
  const seen = new Map<string, { lat: number; lon: number }>()
  const consider = (lat: number, lon: number): void => {
    if (snapToNearestRailGraphNode(graph, lat, lon) === -1) return
    // Dedup key via the shared 4dp station-identity helper (spatial.ts); the
    // emitted {lat, lon} still derives from a toFixed(4) parse, same rounding
    // the key itself uses, so the two never disagree.
    const lat4 = Number(lat.toFixed(4))
    const lon4 = Number(lon.toFixed(4))
    seen.set(coordKey4dp(lat4, lon4), { lat: lat4, lon: lon4 })
  }
  for (const p of pairs) {
    consider(p.fromLat, p.fromLon)
    consider(p.toLat, p.toLon)
  }
  return [...seen.values()]
}

/** `data/prepared/{year}/rail-stops/{scope}.json` sidecar path — shared by
 *  `writeStopsSidecarIfAny` and `warnIfStaleSidecarRemains` so the two can
 *  never disagree on where a scope's sidecar lives. */
function sidecarPathFor(h3r4Dir: string, scope: string): string {
  return resolve(h3r4Dir, '..', 'rail-stops', `${scope}.json`)
}

/** Zero-snapped-stops visibility (2026-07-16 /gg review item 12, reshaped by
 *  the DE Step A v2 Codex review item 5 — full extract-identity invalidation
 *  is still deferred, see TODO below). Two zero-stop cases:
 *
 *  - `diagnosticsWillBeWritten` (this walk produced failure records):
 *    `writeStopsSidecarIfAny` WILL write the sidecar — empty `stops`, full
 *    `failedPairChords` — because a run whose every endpoint missed the
 *    graph is exactly the run whose diagnostics matter most (the DE Step A
 *    failure mode), and losing them would force a full re-run to see why.
 *    Cost accepted with open eyes: an earlier sidecar's stop evidence for
 *    this scope is REPLACED by an empty list, so R15/R16's stop exemption
 *    goes dark here until a healthy re-run — hence the loud warning.
 *  - otherwise (no pairs at all → nothing to write): an earlier sidecar, if
 *    any, silently remains in force for R15/R16, potentially describing a
 *    graph/extract that no longer matches — never deleted here (additive
 *    evidence, not a claim of "no stations exist"), but said loudly.
 *
 *  TODO(2026-07-16): hard extract-identity invalidation (compare the
 *  sidecar's `extractFingerprint` against the CURRENT run's and expire a
 *  mismatch) belongs with the osm-extract stations layer. Until then this is
 *  a loud warning, not an automatic fix. */
function warnZeroSnappedStops(h3r4Dir: string, scope: string, diagnosticsWillBeWritten: boolean): void {
  const path = sidecarPathFor(h3r4Dir, scope)
  if (diagnosticsWillBeWritten) {
    console.error(
      `\n!!! rail-stops sidecar EMPTIED: zero stop endpoints snapped onto the graph this run — ` +
      `${path} is being written with failure diagnostics only (empty stops), so R15/R16's stop ` +
      `exemption for this scope is now empty${existsSync(path) ? ' (stop evidence from an earlier run was replaced)' : ''}. ` +
      `Inspect its failedPairChords for why nothing snapped, then re-run after fixing the extract/pairs.`,
    )
    return
  }
  if (!existsSync(path)) return
  console.error(
    `\n!!! rail-stops sidecar STALE: zero stop endpoints snapped onto the graph this run, but ` +
    `${path} still exists from an earlier run — R15/R16's stop exemption keeps using it as-is ` +
    `(not deleted; see warnZeroSnappedStops's doc for why). Verify the extract/pairs this run ` +
    `actually used before trusting that exemption.`,
  )
}

/** Returns '' when nothing was written — only a walk with NEITHER snapped
 *  stops NOR failure records (i.e. no pairs at all) writes nothing; see
 *  `warnZeroSnappedStops`'s doc for the zero-stops-but-failures case.
 *
 *  `failedPairChords` (DE Step A v2, 2026-07-16 failure analysis, fix 3 +
 *  Codex review item 5): persisted alongside the stops THIS SAME walk
 *  snapped — the sidecar is the ONE existing per-run JSON artifact a walk
 *  produces, so a v3 twin-gate tuning pass reads failure diagnostics from
 *  here instead of re-running the whole enrichment (no new file family).
 *  Written even when ZERO stops snapped, as long as there are failure
 *  records — the all-endpoints-missed run is precisely the one whose
 *  diagnostics must survive (Codex item 5: the old stops-only early return
 *  silently dropped them). */
function writeStopsSidecarIfAny(
  h3r4Dir: string,
  sidecar: RailWalkEnrichOptions['sidecar'],
  stops: ReadonlyArray<{ lat: number; lon: number }>,
  failedPairChords: RailWalkResult['failedPairChords'],
): string {
  if (stops.length === 0 && failedPairChords.length === 0) return ''
  const preparedRoot = resolve(h3r4Dir, '..')
  const year = basename(preparedRoot)
  const dir = resolve(preparedRoot, 'rail-stops')
  mkdirSync(dir, { recursive: true })
  const path = sidecarPathFor(h3r4Dir, sidecar.scope)
  const payload: RailStopsSidecarV1 = {
    version: 1,
    year,
    scope: sidecar.scope,
    extractFingerprint: sidecar.extractFingerprint,
    feeds: sidecar.feeds,
    generatedAt: new Date().toISOString(),
    stops: [...stops],
    failedPairChords,
  }
  writeFileSync(path, JSON.stringify(payload))
  return path
}

export async function enrichRailwaysByGraphWalk(opts: RailWalkEnrichOptions): Promise<RailWalkEnrichStats> {
  const hexIds = iterateCountryHexes(opts.h3r4Dir, opts.bbox, 'railways.arrow')
  const segments = collectGraphSegments(opts.h3r4Dir, hexIds)
  const graph = buildRailGraph(segments)
  const walk = walkRailStationPairs(graph, opts.pairs)

  const rowWhollyOutside = (gate: (lat: number, lon: number) => boolean, row: RailRow): boolean =>
    segmentWhollyOutside(gate, row.midLat, row.midLon, row.startLat, row.startLon, row.endLat, row.endLon)

  let rows = 0
  let stamped = 0
  let retracted = 0
  let silentStamped = 0
  let skippedService = 0
  let skippedForeign = 0
  let skippedForeignNational = 0

  // Every id THIS run legitimately stamps or absorbs: its own stamp id, its
  // silent-residual id, and any (predecessor) ids its retract may disown.
  // The foreign-national guard below must never block a run from updating or
  // reclaiming its OWN rows — only ANOTHER country's national data is
  // untouchable (DE Step A v2 Codex review item 4, 2026-07-16).
  const runOwnSourceIds = new Set<number>([
    opts.sourceId,
    ...(opts.silentResidual ? [opts.silentResidual.sourceId] : []),
    ...(opts.retract?.sourceIds ?? []),
  ])

  for (const hexId of hexIds) {
    const path = resolve(opts.h3r4Dir, hexId, 'railways.arrow')
    // i -> which arm produced the ACCEPTED match, so onApplied (fires only after the
    // priority gate accepts) can tally `stamped` vs `silentStamped` precisely — `match`'s
    // own return value can't distinguish a rejected candidate from an applied one.
    const acceptedBranch = new Map<number, 'walk' | 'silent'>()

    // The three candidate arms (walk stamp / silent residual / extraMatch),
    // unchanged in substance — hoisted out of the match closure so the
    // foreign-national guard below can veto ONE computed candidate instead of
    // being copy-pasted into every arm.
    const candidateFor = (row: RailRow, i: number, key: string): { trains: RailTrains; branch: 'walk' | 'silent' | 'extra' } | null => {
      const stamp = walk.stampsBySegmentKey.get(key)
      if (stamp) {
        // Divisor rides with the stamp in the same atomic write: there is no
        // separate writeRailParallelDivisor pass, and
        // this fires regardless of enableDestructive (stamp-only still gets the
        // walk's own divisor, never an undivided count next to a divided sibling).
        return { trains: { pax: Math.round(stamp.pax), frt: Math.round(stamp.frt), sourceId: opts.sourceId, divisor: stamp.divisor }, branch: 'walk' }
      }
      if (
        opts.silentResidual && opts.enableDestructive && opts.retractSafe && isWalkableRailType(row.railType) &&
        !walk.quarantinedSegmentKeys.has(key)
      ) {
        // A quiet double-track must render at its true divided count, not the
        // engine's undivided default — divisorBySegmentKey covers every
        // sibling-bearing segment independent of whether the WALK stamped it
        // (rail-graph-metrics.ts's applyParallelSpread, /gg review item 2).
        return { trains: { ...opts.silentResidual, divisor: walk.divisorBySegmentKey.get(key) ?? 1 }, branch: 'silent' }
      }
      const extra = opts.extraMatch?.(row, i, hexId)
      return extra ? { trains: extra, branch: 'extra' } : null
    }

    const result = await writeRailTrains(
      path,
      (row, i) => {
        const candidate = candidateFor(row, i, segmentKey(hexId, i))
        if (!candidate) return null
        // FOREIGN-NATIONAL STAMP GUARD (DE Step A v2 Codex review item 4,
        // 2026-07-16): a row whose EXISTING id is another country's
        // nationally-owned data (isNationallyOwnedSource — provenance-tier +
        // registry-flag driven, see sources.ts) is never overwritten by this
        // run, REGARDLESS of `shouldOverwrite`'s rank/year ladder. Verified
        // live risk: DE 9864 vs CZ 110 is a same-rank newer-year win and CZ
        // 9863 is baseline-tier — both would fall to the priority gate alone,
        // yet CZ prepared data is FINAL; territory, not rank, separates two
        // national feeds. The run's OWN ids stay updatable/reclaimable.
        if (!runOwnSourceIds.has(row.existingSourceId) && isNationallyOwnedSource(row.existingSourceId)) {
          skippedForeignNational++
          return null
        }
        if (candidate.branch !== 'extra') acceptedBranch.set(i, candidate.branch)
        return candidate.trains
      },
      (_row, i) => {
        const branch = acceptedBranch.get(i)
        if (branch === 'walk') stamped++
        else if (branch === 'silent') silentStamped++
      },
      // enableDestructive gates the ENTIRE retract object, not just individual
      // arms within it — in stamp-only, no RailRetract is built at all, so
      // neither the country-bleed heal nor the always-on service-arm
      // auto-heal (both live inside writeRailTrains itself) can mutate
      // anything: stamp-only is inspection-safe in full, not just "mostly"
      // (2026-07-16 /gg review item 3).
      opts.retract && opts.enableDestructive
        ? {
            sourceId: opts.retract.sourceIds,
            when: (row, i) => {
              // BLEED arm (2026-07-16 /gg fix batch item 1): fires ONLY when the
              // caller provided a `bleedGate`, and uses IT — never `countryGate`.
              // Bypasses retractSafe/quarantine health (geometry is its own
              // evidence the id never legitimately owned this row) but NOT
              // enableDestructive, since the object above is only ever
              // constructed when that's already true. Sibling implementation of
              // the SAME policy over `segmentWhollyOutside`:
              // heal-rail-country-bleed.ts's `wouldRetract` — a future policy
              // change here must update both.
              if (opts.bleedGate && rowWhollyOutside(opts.bleedGate, row)) return true
              // Testimony guard (item 1's other half): a row wholly outside
              // `countryGate` is outside what THIS run's feeds testify about —
              // the ordinary arm below must never disown it (a shared
              // multi-country id's row here belongs to ANOTHER country's run;
              // only the bleed arm above, whose gate spans every legitimate
              // territory, may condemn it on geometry).
              if (opts.countryGate && rowWhollyOutside(opts.countryGate, row)) return false
              if (!opts.retractSafe) return false
              const key = segmentKey(hexId, i)
              if (walk.stampsBySegmentKey.has(key)) return false // the walk still claims this row this pass
              // quarantineFree (2026-07-16 Step-B refinement): a row NEVER inside
              // any failed pair's quarantine region — including a row with NO
              // graph component at all (a tram, or any family the graph never
              // admitted), which by construction can never appear in
              // `quarantinedSegmentKeys` either — is disownable on ordinary
              // priority-gate terms, same as a road/rail predecessor-id migration
              // would expect. Strictly tighter than the old whole-component check.
              return !walk.quarantinedSegmentKeys.has(key)
            },
          }
        : undefined,
      opts.countryGate,
    )

    rows += result.rows
    retracted += result.retracted
    skippedService += result.skippedService
    skippedForeign += result.skippedForeign
  }

  const quarantinedKm = computeQuarantinedKm(segments, walk.quarantinedSegmentKeys)
  const stampableKm = computeStampableKm(segments)
  const stops = collectSnappedStops(graph, opts.pairs)
  if (stops.length === 0) warnZeroSnappedStops(opts.h3r4Dir, opts.sidecar.scope, walk.failedPairChords.length > 0)
  const sidecarPath = writeStopsSidecarIfAny(opts.h3r4Dir, opts.sidecar, stops, walk.failedPairChords)

  const stats: RailWalkEnrichStats = {
    hexes: hexIds.length,
    rows,
    stamped,
    retracted,
    silentStamped,
    skippedService,
    skippedForeign,
    skippedForeignNational,
    walk: {
      failures: walk.failures,
      unlocalizedPairs: walk.unlocalizedPairs,
      pairsWalked: walk.pairsWalked,
      pairsTotal: walk.pairsTotal,
    },
    quarantinedKm,
    stampableKm,
    sidecarPath,
  }

  console.log(
    `rail-walk-enrich: ${stats.hexes} hexes, ${stats.rows} rows, ${stats.stamped} stamped ` +
    `(${stats.silentStamped} silent, ${stats.retracted} retracted, ${stats.skippedForeignNational} foreign-national skipped) — ` +
    `pairs ${stats.walk.pairsWalked}/${stats.walk.pairsTotal} walked; ` +
    `failures snap=${stats.walk.failures.snapFailed} disconnected=${stats.walk.failures.disconnected} ` +
    `detour=${stats.walk.failures.detourRejected} ambiguous=${stats.walk.failures.ambiguous}; ` +
    `${stats.quarantinedKm.toFixed(1)} of ${stats.stampableKm.toFixed(1)} stampable km quarantined, ${stats.walk.unlocalizedPairs} unlocalized`,
  )

  return stats
}
