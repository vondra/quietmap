/**
 * The ONE place railway train counts are written — the rail twin of
 * `writeRoadAadt` in `roads-arrow.ts`. Centralizes the read→seed→gate→write
 * plumbing every `enrich-railway-{cc}.ts` hand-rolled, and the three correctness
 * invariants that the class-blind enrichers kept getting wrong:
 *
 *   1. SERVICE-SKIP — sidings/yards/spurs (`service > 0`) carry ~no through
 *      traffic and must never inherit a mainline count. (CZ stamped 94,928 of
 *      them before the fix; this skip is now unforgettable.)
 *   2. PRIORITY GATE — `shouldOverwrite` decides per row, so a national feed
 *      can't clobber a higher-rank neighbour's cross-border rows.
 *   3. ATOMIC COUNTS+DIVISOR — `parallel_divisor` rides in the SAME write as
 *      pax/frt/source_id, never a separate pass (2026-07-16 /gg review):
 *        - a `retract` always resets the row's divisor to 1 in the same write
 *          — divisor is void without its stamp (a disowned row must not stay
 *          wrongly quiet under a stale ÷N, verified /gg finding, 2026-07-15);
 *        - an accepted `match` that carries an optional `divisor` sets it in
 *          that SAME write — a caller that computes counts and a divisor
 *          together (the graph-walk driver's per-segment lateral spread) can
 *          no longer lose the divisor half to a separate opt-in pass that a
 *          stamp-only/inspection mode skips (the bug this fixes: a silent
 *          residual landing on a parallel-track row rendered undivided, and a
 *          re-run over already-correct data could wipe a divisor nobody
 *          re-asserted that pass);
 *        - order within one row is retract-then-match: a same-pass reclaim
 *          (retract zeroes, match immediately re-stamps) ends at the NEW
 *          divisor, never stuck at the retract's interim 1.
 *      Value-diff idempotence (below) compares divisor too: identical
 *      pax/frt/sourceId/divisor never forces a rewrite.
 *
 * The FAMILY routing (rail↔rail_type 0, tram↔1/2) lives in the per-feed `match`
 * closure — each national GTFS classifies its own route_types, so that stays
 * per-country (see `enrich-railway-th.ts` for the reference shape).
 *
 * PROVENANCE HONESTY: `match` returns null when the feed has no real answer —
 * the row stays source_id=0 and the ENGINE default table
 * (engine/noise-compute/src/emission/railway.rs::default_traffic) owns the
 * "we don't know" case. Enrichers must NEVER stamp a class-default guess under
 * their measured-tier id (the pre-2026-07-10 fallback design did; each enricher
 * now carries an OLD_FALLBACK retract signature to disown those stamps).
 *
 * Also hosts `writeRailParallelDivisor` — the dedicated `parallel_divisor`
 * column writer used by the shared parallel-track pass
 * (enrich-railways-parallel.ts, task #30) for sources that do NOT ride their
 * divisor through `RailTrains.divisor` above (the walk-managed sources do and
 * skip that pass entirely — see enrich-railways-parallel.ts's manifest note).
 */

import { makeVector, makeTable, type Table } from 'apache-arrow'
import { shouldOverwrite, withArrowWrite } from './provenance.js'
import { SOURCES_BY_ID } from './sources.js'
import { segmentWhollyOutside } from './country-polygon.js'

/** Train counts per day + the provenance id to stamp on one matched railway row. */
export interface RailTrains {
  pax: number
  frt: number
  sourceId: number
  /** Optional parallel-track divisor to SET on this row in the SAME write as
   *  pax/frt/sourceId (the atomic counts+divisor invariant).
   *  Must be a positive integer (1-255) when given; validated with the same
   *  fail-loud contract as pax/frt/sourceId (a bad value throws, file
   *  untouched). Omit to leave the row's existing divisor byte-for-byte
   *  alone — most `match` closures don't know a divisor and should never
   *  reset one they didn't compute. */
  divisor?: number
}

/** Read-only view of one railway row, handed to the match callback. */
export interface RailRow {
  /** 0=rail 1=tram 2=light_rail 3=narrow_gauge 4=funicular (engine inputs.rs). */
  railType: number
  /** 0=main 1=branch 2=industrial. */
  usage: number
  /** The source_id already on the row. Fast-exit BEFORE expensive matching when a
   *  higher-priority dataset owns it: `if (!shouldOverwrite(row.existingSourceId, MY_ID)) return null`.
   *  The writer re-applies the same gate, so this is a perf shortcut, never the safety net. */
  existingSourceId: number
  /** Current trains_passenger / trains_freight on the row. Exists so a `retract`
   *  `when` can fingerprint legacy stamps by their exact value tuple (the
   *  pre-2026-07-10 class-default fallbacks each enricher now disowns via its
   *  OLD_FALLBACK signature) — match closures should not read these. */
  existingPax: number
  existingFrt: number
  startLat: number
  startLon: number
  endLat: number
  endLon: number
  midLat: number
  midLon: number
  /** OSM `name` ('' when absent) — lets an enricher disambiguate named corridors
   *  (e.g. ET routes EDR vs AKR on the shared Addis–Awash trunk by line name). */
  name: string
}

export interface WriteRailResult {
  rows: number
  /** Every row an accepted `match` landed on, INCLUDING a value-identical
   *  re-stamp (same pax/frt/sourceId/divisor already on the row) — callers log
   *  this as "GTFS-stamped" and expect it to count every accepted match, not
   *  just the ones that changed bytes. Use `updated` to learn whether the file
   *  itself was rewritten. */
  matched: number
  /** True only when some row's on-disk bytes actually changed — a re-run whose
   *  matches/retracts reproduce identical values (incl. divisor) returns
   *  `false` and leaves the file byte-identical (world-scale walk re-runs are
   *  byte-no-ops). */
  updated: boolean
  /** Rows skipped because `service > 0` (sidings/yards) — never offered to `match`. */
  skippedService: number
  /** Rows skipped by `countryGate` (segment wholly outside the source's country)
   *  — never offered to `match`; the retract arms still reached them. */
  skippedForeign: number
  /** Rows this dataset previously owned and has now disowned via `retract`
   *  (stamp + train counts reset to zero, open for lower-priority enrichers).
   *  Every retracted row also has its `parallel_divisor` reset to 1 in the
   *  same write — a disowned row must not stay
   *  wrongly quiet under a stale ÷N, unless a same-pass `match` reclaim sets
   *  a new divisor of its own (retract-then-match order). */
  retracted: number
}

/** Self-healing: disown rows a dataset previously stamped but would no longer
 *  claim under its CURRENT matching rules (e.g. an OSM edit that reclassifies
 *  an already-GTFS-stamped mainline segment as a yard track, or a re-import
 *  that no longer routes through a corridor it used to claim) — the rail
 *  twin of `RoadRetract` in `roads-arrow.ts`.
 *  `when` is evaluated for every row whose `source_id` equals `sourceId` (or,
 *  when `sourceId` is a list, equals ANY id in it — a dataset absorbing rows
 *  previously stamped under one or more retired predecessor ids, e.g. a
 *  bespoke enricher folded into a shared driver, disowns them all in the same
 *  pass), BEFORE the service-skip gate — retraction is about disowning, not
 *  matching, so it must reach rows the dataset would no longer even reach
 *  today (e.g. a row now `service > 0`, which `match` never sees). `when` may
 *  fingerprint by the row's current values (`existingPax`/`existingFrt`) — how
 *  the legacy class-default stamps are recognized. A retract
 *  zeroes trains_passenger/trains_freight and the stamp AND resets the row's
 *  parallel_divisor to 1 in the same write (divisor is void without its stamp
 *  under the atomic counts+divisor invariant; other rows' divisors are left untouched, and
 *  a hex whose `parallel_divisor` column is absent stays absent since an
 *  implicit-1 row has nothing to reset); `match` STILL runs on
 *  the row in the same pass and may immediately re-claim it (a real count that
 *  coincidentally equals a legacy tuple is re-stamped, never lost) — such a row
 *  counts in BOTH `retracted` and `matched`. A dataset can only ever retract rows it
 *  owns — `sourceId` is compared against the row, not trusted from the
 *  caller's match logic. Rails have no taper-style derived annotation, so a
 *  retract here is just the three payload columns plus the divisor reset. */
export interface RailRetract {
  /** The row's current `source_id` must equal this id — or, when a list is
   *  given, ANY id in it — to be a retract candidate. A single id is the
   *  common case; a list lets one dataset disown rows left by itself under
   *  several historical ids in one pass. */
  sourceId: number | readonly number[]
  when: (row: RailRow, i: number) => boolean
}

/**
 * Seed-from-existing → skip service tracks → run `match` per non-service row
 * behind the `shouldOverwrite` priority gate → rebuild trains_passenger/
 * trains_freight (Int32) + source_id (Uint16) + (conditionally)
 * parallel_divisor (Uint8) — ATOMICALLY, all in the same write (module doc
 * invariant 3): a retract resets a disowned row's divisor to 1; an accepted
 * match's optional `divisor` sets it to a new value; every other row's
 * divisor is copied byte-for-byte verbatim, and the column itself is only
 * touched (or newly materialized) when SOME row's divisor actually changes —
 * every other column copied verbatim → atomic 'file'-format write via
 * `withArrowWrite`. Returns the original table unchanged when no row's
 * on-disk values actually change — whether because nothing matched or
 * retracted, or every accepted match reproduced pax/frt/sourceId/divisor
 * already on the row — so the file stays byte-identical (a world-scale walk
 * re-run over already-correct data is a byte-no-op).
 *
 * `match(row, i)` is invoked for every NON-SERVICE row (return `null` = leave the
 * row as-is; source_id stays 0 and the ENGINE default table owns the unknown —
 * see the module header's provenance-honesty rule; the old advice to fallback
 * inside `match` was the design the 2026-07-10 purge deleted). A match whose
 * pax/frt/sourceId/divisor all equal what the row already carries still counts in
 * `matched` (callers log it as "GTFS-stamped") and still reaches `onApplied`,
 * but does NOT itself force a rewrite — `updated` reflects only rows that
 * actually changed value. `onApplied` fires for every accepted match,
 * value-changing or not — count matched there.
 *
 * `countryGate` (#31.7) is THE national-ownership gate, centralized so no
 * per-country enricher can forget it: when set, `match` is never offered a
 * segment wholly outside the country (start+mid+end all foreign — the R9
 * auditor rule via `segmentWhollyOutside`, so genuine border-straddlers stay
 * claimable). Without it, a border stop's 500 m grid join stamps rows across
 * the border and heal-rail-country-bleed retracts them every run (ping-pong;
 * DE↔CZ held 151 such rows). Every NATIONAL enricher must pass its
 * `makeCountryGate(iso)`; only the continental aggregate (europe) and
 * multi-country passes legitimately omit it. Retract `when` country arms use
 * the same predicate directly — the two sides must agree or they oscillate.
 */
export async function writeRailTrains(
  arrowPath: string,
  match: (row: RailRow, i: number) => RailTrains | null,
  onApplied?: (row: RailRow, i: number, applied: RailTrains) => void,
  retract?: RailRetract,
  countryGate?: (lat: number, lon: number) => boolean,
): Promise<WriteRailResult> {
  let rows = 0
  let matched = 0
  let skippedService = 0
  let skippedForeign = 0
  let retracted = 0
  let updated = false

  await withArrowWrite(arrowPath, (table: Table): Table => {
    const n = table.numRows
    rows = n
    if (n === 0) return table

    const sLat = table.getChild('start_lat')
    const sLon = table.getChild('start_lon')
    const eLat = table.getChild('end_lat')
    const eLon = table.getChild('end_lon')
    const rtCol = table.getChild('rail_type')
    const usCol = table.getChild('usage')
    const nmCol = table.getChild('name')
    const svcCol = table.getChild('service')
    if (!sLat || !sLon) return table // malformed hex — never touch

    const exPax = table.getChild('trains_passenger')
    const exFrt = table.getChild('trains_freight')
    const exSrc = table.getChild('source_id')
    const exDiv = table.getChild('parallel_divisor')

    const pax = new Int32Array(n)
    const frt = new Int32Array(n)
    const src = new Uint16Array(n)
    // Divisor rides atomically with pax/frt/src —
    // seeded verbatim (default 1 when the column is absent) exactly like the
    // other three, so an untouched row's value survives unchanged whether or
    // not the column pre-existed.
    const div = new Uint8Array(n)
    for (let i = 0; i < n; i++) {
      pax[i] = (exPax?.get(i) as number) ?? 0
      frt[i] = (exFrt?.get(i) as number) ?? 0
      src[i] = (exSrc?.get(i) as number) ?? 0
      div[i] = (exDiv?.get(i) as number) ?? 1
    }

    // Multi-id retract: `sourceId` is a single id in the common case, or a
    // short list when one dataset is absorbing rows left by retired
    // predecessor ids — a Set makes both arms below a single O(1) membership
    // check regardless of shape.
    const retractIds = retract
      ? new Set(typeof retract.sourceId === 'number' ? [retract.sourceId] : retract.sourceId)
      : null

    let any = false
    // True once SOME row's divisor value actually changed this pass — decides
    // whether the parallel_divisor column is rebuilt/materialized at all
    // (never touch it when nothing needed a different value, matching
    // writeRailParallelDivisor's own "never materialize an all-1 column"
    // policy below).
    let divisorTouched = false
    for (let i = 0; i < n; i++) {
      const startLat = sLat.get(i) as number
      const startLon = sLon.get(i) as number
      const endLat = (eLat?.get(i) as number) ?? startLat
      const endLon = (eLon?.get(i) as number) ?? startLon
      const row: RailRow = {
        railType: (rtCol?.get(i) as number) ?? 0,
        usage: (usCol?.get(i) as number) ?? 0,
        existingSourceId: src[i],
        // Reading the seeded arrays is safe: index i is only mutated at iteration i,
        // after this row object is built — so these are the pre-pass values.
        existingPax: pax[i],
        existingFrt: frt[i],
        startLat,
        startLon,
        endLat,
        endLon,
        midLat: (startLat + endLat) / 2,
        midLon: (startLon + endLon) / 2,
        name: (nmCol?.get(i) as string) ?? '',
      }
      // Self-heal BEFORE the service-skip gate: a row this dataset owns but
      // would no longer claim is disowned even when it's now a siding (that
      // is precisely the mis-stamp being healed). NO `continue` — the pass
      // falls through so `match` may re-claim the row in the SAME pass: a
      // real measurement that coincidentally equals a legacy tuple (BE cache
      // holds 5 genuine tram rows at exactly 200/0 — /gg Codex+Gemini
      // consensus CRITICAL) is re-stamped immediately, never lost.
      if (retract && retractIds!.has(src[i]) && retract.when(row, i)) {
        pax[i] = 0
        frt[i] = 0
        src[i] = 0
        // Divisor void without its stamp — reset to
        // 1 IN THE SAME retract arm, not a separate pass. A same-pass `match`
        // reclaim below (if any) may immediately overwrite this with its own
        // divisor — retract-then-match order.
        if (div[i] !== 1) { div[i] = 1; divisorTouched = true }
        retracted++
        any = true
      }
      // Service tracks (yards/sidings/spurs) never inherit a count — centralized
      // here so no enricher can forget it (the CZ 94,928-siding bug). A row the
      // retracting source still OWNS here is stale by definition (service rows
      // can never be legitimately stamped — this very gate blocks it), so it is
      // cleared regardless of `when` (/gg Codex W1: the "reclassified as siding
      // near a live stop" case, where corroboration returns when=false).
      if (((svcCol?.get(i) as number) ?? 0) > 0) {
        if (retract && retractIds!.has(src[i])) {
          pax[i] = 0
          frt[i] = 0
          src[i] = 0
          if (div[i] !== 1) { div[i] = 1; divisorTouched = true }
          retracted++
          any = true
        }
        skippedService++
        continue
      }

      // #31.7 national-ownership gate — see the function doc. Placed AFTER the
      // retract/service arms (disowning must reach foreign rows; that is the
      // heal) and BEFORE match (stamping must not).
      if (countryGate && segmentWhollyOutside(countryGate, row.midLat, row.midLon, startLat, startLon, endLat, endLon)) {
        skippedForeign++
        continue
      }

      const m = match(row, i)
      if (!m) continue
      // Fail loud on a malformed match — TypedArrays silently coerce/truncate a bad
      // value (NaN→0, 70000→4464, -1→huge), exactly how the road "wrote zeros" bugs
      // slipped through. Train counts are non-negative ints; source_id fits Uint16;
      // divisor (when given) is a positive Uint8 — same fail-loud contract.
      if (
        !Number.isInteger(m.pax) || m.pax < 0 ||
        !Number.isInteger(m.frt) || m.frt < 0 ||
        !Number.isInteger(m.sourceId) || m.sourceId <= 0 || m.sourceId > 0xffff ||
        (m.divisor !== undefined && (!Number.isInteger(m.divisor) || m.divisor < 1 || m.divisor > 255))
      ) {
        throw new Error(`writeRailTrains: invalid match at row ${i} in ${arrowPath}: ${JSON.stringify(m)}`)
      }
      // Registry-membership + layer check (#31.4 twin): `shouldOverwrite` deliberately
      // treats an UNKNOWN existing id as legacy-to-replace, so without this a typo'd
      // positive id would sail through every downstream gate and stamp garbage
      // provenance. A rail writer accepts only registered RAILWAYS sources.
      if (SOURCES_BY_ID.get(m.sourceId)?.layer !== 'railways') {
        throw new Error(`writeRailTrains: sourceId ${m.sourceId} is not a registered railways source (row ${i} in ${arrowPath})`)
      }
      if (!shouldOverwrite(src[i], m.sourceId)) continue // priority gate OWNED here
      // Value-diff idempotence: an accepted match that reproduces exactly what
      // the row already carries (INCLUDING divisor) is still a match (counted
      // below, `onApplied` still fires — accepted-ness is the contract) but
      // must NOT force a rewrite, or a world-scale walk re-run over
      // already-correct data would rewrite every file it touches instead of
      // being a byte-no-op. `m.divisor === undefined` means "this match
      // doesn't manage a divisor" — the row's current value (already seeded,
      // possibly just reset by a same-pass retract above) rides through
      // unchanged rather than being forced back to some default.
      const newDivisor = m.divisor !== undefined ? m.divisor : div[i]
      const valueChanged = pax[i] !== m.pax || frt[i] !== m.frt || src[i] !== m.sourceId || div[i] !== newDivisor
      pax[i] = m.pax
      frt[i] = m.frt
      src[i] = m.sourceId
      if (div[i] !== newDivisor) { div[i] = newDivisor; divisorTouched = true }
      matched++
      if (valueChanged) any = true
      onApplied?.(row, i, m)
    }
    if (!any) return table // no change → withArrowWrite leaves bytes untouched
    updated = true

    // parallel_divisor is rebuilt (or newly materialized) ONLY when some row's
    // divisor value actually changed this pass (`divisorTouched`) — every
    // other row's byte is `div[i]`'s seeded-verbatim value, so the rebuilt
    // column is byte-identical to the original wherever nothing changed. A
    // hex whose column was absent before this pass stays absent unless
    // `divisorTouched` is true (matches writeRailParallelDivisor's "never
    // materialize an all-1 column" policy below — `divisorTouched` can only
    // be true when some row's final value differs from its seeded default).
    const nextDiv: Uint8Array | null = divisorTouched ? div : null

    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- mixed Vector/makeVector
    // record vs makeTable's TypedArray-only typing (see roads-arrow.ts).
    const cols: Record<string, any> = {}
    for (const f of table.schema.fields) {
      if (['trains_passenger', 'trains_freight', 'source_id'].includes(f.name)) continue
      if (nextDiv && f.name === 'parallel_divisor') continue
      cols[f.name] = table.getChild(f.name)!
    }
    cols['trains_passenger'] = makeVector(pax)
    cols['trains_freight'] = makeVector(frt)
    cols['source_id'] = makeVector(src)
    if (nextDiv) cols['parallel_divisor'] = makeVector(nextDiv)
    return makeTable(cols)
  })

  return { rows, matched, updated, skippedService, skippedForeign, retracted }
}

export interface WriteRailParallelDivisorResult {
  rows: number
  /** Rows whose stored divisor actually changed value. */
  changedRows: number
  /** True when the file was rewritten (false = byte-identical skip). */
  updated: boolean
}

/**
 * Rebuild ONLY the `parallel_divisor` column (Uint8) of one railways.arrow —
 * every other column copied verbatim, atomic 'file'-format write via
 * `withArrowWrite`, byte-identical skip when no row changes value (a missing
 * column is only materialized when some row actually needs a value ≠ 1 — the
 * engine readers default an absent column to 1, hex_store/source_loader_rail
 * `unwrap_or(1)`).
 *
 * `divisorForRow(i)` returns the divisor to store for row i, or `null` to keep
 * the row's existing value verbatim. Non-null values are floored at 1 (0 → 1:
 * a divisor can never mean less than "one track"); non-integer, negative or
 * >255 values throw with the file untouched — TypedArray coercion would
 * silently truncate them (the road "wrote zeros" bug class).
 *
 * POLICY-FREE by design: the column carries no provenance, so any write
 * asymmetry (e.g. enrich-railways-parallel.ts's only-raise-from-1 rule that
 * protects the CZ czpttKey-computed divisors) lives in the CALLER's decision
 * function — this writer stores exactly what it is told.
 */
export async function writeRailParallelDivisor(
  arrowPath: string,
  divisorForRow: (i: number) => number | null,
): Promise<WriteRailParallelDivisorResult> {
  let rows = 0
  let changedRows = 0
  let updated = false

  await withArrowWrite(arrowPath, (table: Table): Table => {
    const n = table.numRows
    rows = n
    if (n === 0) return table

    const exDiv = table.getChild('parallel_divisor')
    const next = new Uint8Array(n)
    let any = false
    for (let i = 0; i < n; i++) {
      // Raw existing value (not floored): a null-keep must preserve bytes, not heal.
      const existing = exDiv ? ((exDiv.get(i) as number) ?? 1) : 1
      const v = divisorForRow(i)
      if (v === null) {
        next[i] = existing
        continue
      }
      if (!Number.isInteger(v) || v < 0 || v > 255) {
        throw new Error(`writeRailParallelDivisor: invalid divisor at row ${i} in ${arrowPath}: ${v}`)
      }
      next[i] = Math.max(1, v) // divisor floor — never below "one track"
      if (next[i] !== existing) {
        changedRows++
        any = true
      }
    }
    if (!any) return table // no value change → withArrowWrite leaves bytes untouched
    updated = true

    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- mixed Vector/makeVector
    // record vs makeTable's TypedArray-only typing (see writeRailTrains above).
    const cols: Record<string, any> = {}
    for (const f of table.schema.fields) {
      if (f.name === 'parallel_divisor') continue
      cols[f.name] = table.getChild(f.name)!
    }
    cols['parallel_divisor'] = makeVector(next)
    return makeTable(cols)
  })

  return { rows, changedRows, updated }
}
