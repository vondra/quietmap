/** Shared road scans and atomic AADT writes; country-specific loaders supply matching. */

import { existsSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { makeVector, makeTable, type Table } from 'apache-arrow'
import { cellToLatLng } from 'h3-js'
import { shouldOverwrite, withArrowWrite } from './provenance.js'
import { isMeasured, SOURCES_BY_ID } from './sources.js'
import { makeOwnershipGate, hasCountryPolygon, segmentWhollyOutside } from './country-polygon.js'
import { inBbox } from './spatial.js'

/** National-ownership gate for a stamped `source_id` — so no per-country
 *  enricher can forget it (the rail countryGate is opt-in per caller; this is
 *  automatic and unforgettable, #33). Anchored on the PROVENANCE TIER, not on
 *  the key string: only `national-measured` / `national-proxy` sources are
 *  country-scoped and gated. A continental (`eu-city-traffic`) or global source
 *  legitimately spans many countries and is NEVER gated — so a future global
 *  source keyed like `us-…` can't be misread as US-only and silently blocked
 *  from the rest of the world (/gg #33 Gemini CRITICAL). The country then comes
 *  from the national key's `cc-` prefix; a national source whose ISO has no
 *  CGAZ polygon (logged once) stays ungated — the heal/auditor still catch it —
 *  rather than dropping every row to a silent all-false gate. Cached per
 *  source_id for the run. */
const gateBySourceId = new Map<number, ((lat: number, lon: number) => boolean) | null>()
const missingGateWarned = new Set<string>()
function nationalGateForSource(sourceId: number): ((lat: number, lon: number) => boolean) | null {
  const cached = gateBySourceId.get(sourceId)
  if (cached !== undefined) return cached
  let gate: ((lat: number, lon: number) => boolean) | null = null
  const source = SOURCES_BY_ID.get(sourceId)
  const isNational = source?.provenance === 'national-measured' || source?.provenance === 'national-proxy'
  const m = isNational ? /^([a-z]{2})-/.exec(source!.key) : null
  if (m) {
    const iso = m[1].toUpperCase()
    // hasCountryPolygon separates "not a mappable country" (ungate — a bbox
    // territory) from an I/O failure loading CGAZ (propagates LOUD, never
    // silently ungates a declared national source — /gg #33 Codex).
    if (hasCountryPolygon(iso)) {
      gate = makeOwnershipGate(iso)
    } else if (!missingGateWarned.has(iso)) {
      console.warn(`  writeRoadAadt: no CGAZ polygon for ${iso} (source ${source!.key}) — country gate skipped for it`)
      missingGateWarned.add(iso)
    }
  }
  gateBySourceId.set(sourceId, gate)
  return gate
}

/** OSM `road_class` collapsed to the 0..4 major-road rank used by the
 *  source-class ↔ OSM-class compatibility gate (0 motorway, 1 trunk, 2 primary,
 *  3 secondary, 4 tertiary; link codes 10/11/12 collapse to their parent).
 *  Everything below tertiary (residential/living_street/service/track/
 *  unclassified) maps to 6 — TWO past tertiary, so it stays out of reach of
 *  any 0..4 source rank even at the ±1 tolerance. (A stale "maps to 5" claim
 *  here once misled a review into a phantom coverage-leak finding.)
 *  Each enricher pairs this with a source-side rank map (e.g. HPMS F_SYSTEM in
 *  enrich-roads-us.ts) and rejects matches where the ranks differ by more than
 *  ROAD_CLASS_RANK_TOLERANCE — the fix for the "Papermill Drive" cross-class
 *  proximity bug (a secondary inheriting a nearby interstate's 184k AADT). */
export const osmRoadClassRank = (roadClass: number): number =>
  roadClass <= 4 ? roadClass : roadClass === 10 ? 0 : roadClass === 11 ? 1 : roadClass === 12 ? 2 : 6

/** Max |source rank − OSM rank| a match may span: 1 tolerates the usual
 *  source ↔ OSM off-by-one tagging variance while still blocking
 *  interstate→secondary (|0 − 3| = 3). From the US pilot (a2a35343). */
export const ROAD_CLASS_RANK_TOLERANCE = 1

/** AADT per CNOSSOS class + the provenance id to stamp on one matched road row.
 *  `sourceId` is per-row on purpose — DE writes Autobahn vs Bundesstraße ids in
 *  a single pass (`enrich-roads-de.ts`). */
export interface RoadAadt {
  light: number
  medium: number
  heavy: number
  moto: number
  sourceId: number
  /** R7 taper graded effective speed (km/h, 1-254) → the dedicated
   *  `speed_taper` column; the OSM `speed_limit` tag is NEVER written by any
   *  enricher. `speed_taper` is a derived annotation, not owned data: EVERY
   *  accepted write clears it unless the payload restates it (a data change
   *  voids the refinement computed from the old state — /gg Codex+Gemini
   *  consensus on the ownership-aliasing bug class), and every retract clears
   *  it too. The column is created on first use; absent reads as 0. */
  speedTaper?: number
}

/** Read-only view of one road row, handed to the match callback. */
export interface RoadRow {
  ref: string | null
  roadClass: number
  /** The source_id already on the row. Let the closure fast-exit BEFORE expensive
   *  matching when a higher-priority dataset owns the row:
   *  `if (!shouldOverwrite(row.existingSourceId, MY_ID)) return null`. The writer
   *  re-applies the same gate, so this is a perf shortcut, never the safety net. */
  existingSourceId: number
  startLat: number
  startLon: number
  endLat: number
  endLon: number
  midLat: number
  midLon: number
  /** OSM way name (null when untagged). City adapters name-match against
   *  municipal counter street names; national censuses keep ref-matching. */
  name: string | null
  /** OSM way id — direct join key for municipal datasets that publish it
   *  (e.g. Montpellier's osm_id column). */
  osmId: number | null
}

export interface WriteRoadResult {
  rows: number
  /** Accepted matches, including those that leave stored values unchanged. */
  matched: number
  /** True only when final stored values changed. Accepted matches are counted separately. */
  updated: boolean
  /** Rows whose `road_class` fell outside the source's `coverage` set — skipped
   *  before `match` ran, so a major-road dataset cannot stamp a minor road. */
  skipped: number
  /** Rows skipped by `countryGate` (segment wholly outside the source's country)
   *  — never offered to `match`; the retract arm still reached them (#33). */
  skippedForeign: number
  /** Rows this dataset previously owned and has now disowned via `retract`
   *  (stamp + AADT reset to zero, open for lower-priority enrichers). */
  retracted: number
}

/** Self-healing: disown rows a dataset previously stamped but would no longer
 *  claim under its CURRENT matching rules (e.g. the ŘSD count that landed on a
 *  service street through a free-text ref — "Zelená 20" → "20" → I/20).
 *  `when` is evaluated for every row whose `source_id` equals `sourceId`,
 *  BEFORE the coverage gate — retraction is about disowning, not matching, so
 *  it must reach rows whose class the dataset no longer even covers. A retract
 *  zeroes the four AADT columns and the stamp; the row becomes free for the
 *  service-tree / continuity passes on their next run. A dataset can only ever
 *  retract rows it owns — `sourceId` is compared against the row, not trusted
 *  from the caller's match logic. */
export interface RoadRetract {
  sourceId: number
  when: (row: RoadRow, i: number) => boolean
}

/**
 * Seed-from-existing → run `match` per row → apply only behind the
 * `shouldOverwrite` priority gate → rebuild the five columns (aadt_light/medium/
 * heavy/moto Int32 + source_id Uint16, all other columns copied verbatim) → atomic
 * 'file'-format write via `withArrowWrite`. Returns the original table unchanged
 * when final values are unchanged, including retract/reclaim, leaving the file untouched.
 *
 * `match(row, i)` is invoked for every row WITHIN `coverage` (return `null` = no
 * match) — count per-class totals there. Rows whose `road_class` is outside
 * `coverage` are skipped before `match` (tallied in `result.skipped`), so a
 * dataset that only surveys major roads physically cannot stamp a minor one.
 * `onApplied(row, i)` fires only after the priority gate accepts a match — count
 * per-class matched there (the count must follow the gate, not precede it).
 *
 * National ownership (#33) is AUTOMATIC: a match whose `sourceId` carries a
 * `cc-` country prefix is refused on any segment wholly outside that country
 * (start+mid+end all foreign — `nationalGateForSource` + `segmentWhollyOutside`;
 * `result.skippedForeign`). No enricher passes or can forget it — unlike the
 * rail `countryGate`, which is per-caller opt-in. `heal-road-country-bleed`
 * clears legacy foreign stamps via the `retract` arm.
 */
export async function writeRoadAadt(
  arrowPath: string,
  match: (row: RoadRow, i: number) => RoadAadt | null,
  onApplied?: (row: RoadRow, i: number, applied: RoadAadt) => void,
  coverage?: ReadonlySet<number>,
  retract?: RoadRetract,
): Promise<WriteRoadResult> {
  let rows = 0
  let matched = 0
  let skipped = 0
  let skippedForeign = 0
  let retracted = 0
  let updated = false

  await withArrowWrite(arrowPath, (table: Table): Table => {
    const n = table.numRows
    rows = n
    if (n === 0) return table

    const refCol = table.getChild('ref')
    const nameCol = table.getChild('name')
    const osmIdCol = table.getChild('osm_id')
    const sLat = table.getChild('start_lat')
    const sLon = table.getChild('start_lon')
    const eLat = table.getChild('end_lat')
    const eLon = table.getChild('end_lon')
    const clsCol = table.getChild('road_class')
    if (!sLat || !sLon) return table // malformed hex — never touch

    const exL = table.getChild('aadt_light')
    const exM = table.getChild('aadt_medium')
    const exH = table.getChild('aadt_heavy')
    const exMo = table.getChild('aadt_moto')
    const exSrc = table.getChild('source_id')
    const exTaper = table.getChild('speed_taper')

    const light = new Int32Array(n)
    const medium = new Int32Array(n)
    const heavy = new Int32Array(n)
    const moto = new Int32Array(n)
    const src = new Uint16Array(n)
    for (let i = 0; i < n; i++) {
      light[i] = (exL?.get(i) as number) ?? 0
      medium[i] = (exM?.get(i) as number) ?? 0
      heavy[i] = (exH?.get(i) as number) ?? 0
      moto[i] = (exMo?.get(i) as number) ?? 0
      src[i] = (exSrc?.get(i) as number) ?? 0
    }
    // speed_taper materializes LAZILY on the first row whose value actually
    // changes — the overwhelmingly common AADT-only write on taper-free rows
    // (every national enricher, world-wide) must not pay an O(n) rebuild of a
    // column it never touches; untouched it flows through the generic
    // verbatim-copy path below (and is never invented on schemas without it).
    let taper: Uint8Array | null = null
    const taperAt = (i: number): number => (taper ? taper[i] : ((exTaper?.get(i) as number) ?? 0))
    const setTaper = (i: number, v: number): void => {
      if (taperAt(i) === v) return
      if (!taper) {
        taper = new Uint8Array(n)
        for (let k = 0; k < n; k++) taper[k] = (exTaper?.get(k) as number) ?? 0
      }
      taper[i] = v
    }

    for (let i = 0; i < n; i++) {
      const startLat = sLat.get(i) as number
      const startLon = sLon.get(i) as number
      const endLat = (eLat?.get(i) as number) ?? startLat
      const endLon = (eLon?.get(i) as number) ?? startLon
      const row: RoadRow = {
        ref: (refCol?.get(i) as string | null) ?? null,
        roadClass: (clsCol?.get(i) as number) ?? 5,
        existingSourceId: src[i],
        startLat,
        startLon,
        endLat,
        endLon,
        midLat: (startLat + endLat) / 2,
        midLon: (startLon + endLon) / 2,
        name: (nameCol?.get(i) as string | null) ?? null,
        osmId: osmIdCol ? Number(osmIdCol.get(i)) : null,
      }
      // Self-heal BEFORE the coverage gate: a row this dataset owns but would
      // no longer claim is disowned even when its class now falls outside
      // coverage (that is precisely the mis-stamp being healed).
      if (retract && src[i] === retract.sourceId && retract.when(row, i)) {
        light[i] = 0
        medium[i] = 0
        heavy[i] = 0
        moto[i] = 0
        src[i] = 0
        setTaper(i, 0) // a disowned row's taper refinement is void with it
        retracted++
        // NO `continue` — fall through so `match` may re-claim the row in this
        // SAME pass (mirrors writeRailTrains; /gg Codex+Gemini consensus on the
        // rail twin, re-confirmed by /gg #31 round 2 here): a real measurement
        // that happens to trip the retract fingerprint is re-stamped
        // immediately, never dropped for a cycle. Such a row counts in BOTH
        // `retracted` and `matched`.
      }
      // Class gate (centralized so no enricher can forget it): a source whose
      // `coverage` set omits this row's `road_class` never reaches `match`, so a
      // major-road dataset can't bleed onto a residential/service street — the
      // Knoxville "Papermill Pointe Way" = 211,587 AADT bug. road_class codes:
      // 0 motorway..4 tertiary, 5 residential, 6 living_street, 7 service,
      // 8 track, 9 unclassified, 10/11/12 links (engine inputs.rs).
      if (coverage && !coverage.has(row.roadClass)) { skipped++; continue }
      const m = match(row, i)
      if (!m) continue
      // Fail loud on a malformed match (missing/NaN column, or a 0/unknown source_id):
      // TypedArrays silently coerce undefined/NaN to 0, which is exactly how the
      // IT/SA "wrote zeros" bugs slipped through. A migration typo aborts here.
      if (
        !Number.isFinite(m.light) || !Number.isFinite(m.medium) ||
        !Number.isFinite(m.heavy) || !Number.isFinite(m.moto) ||
        !Number.isInteger(m.sourceId) || m.sourceId <= 0
      ) {
        throw new Error(`writeRoadAadt: invalid match at row ${i} in ${arrowPath}: ${JSON.stringify(m)}`)
      }
      if (
        m.speedTaper !== undefined &&
        (!Number.isInteger(m.speedTaper) || m.speedTaper < 1 || m.speedTaper > 254)
      ) {
        throw new Error(`writeRoadAadt: invalid speedTaper at row ${i} in ${arrowPath}: ${JSON.stringify(m)}`)
      }
      // Registry-membership + layer check (#31.4): `shouldOverwrite` deliberately
      // treats an UNKNOWN existing id as legacy-to-replace, so a typo'd positive
      // id (e.g. 65000) would otherwise pass every downstream gate — including
      // the measured-zero guard below, since isMeasured(unknown) is false. A
      // road writer accepts only registered ROADS sources.
      if (SOURCES_BY_ID.get(m.sourceId)?.layer !== 'roads') {
        throw new Error(`writeRoadAadt: sourceId ${m.sourceId} is not a registered roads source (row ${i} in ${arrowPath})`)
      }
      // #33 national-ownership gate, auto-derived from the stamped id's country
      // prefix: a national road census must not STAMP a segment wholly outside
      // its own country (start+mid+end all foreign — the shared R9 predicate;
      // border-straddlers stay claimable). Ungated ref-matching within a bbox
      // that reaches into neighbours stamped ~200k foreign rows (world gate
      // 2026-07-12: TR into Greece/Iran, JP's grid over Korea). The retract arm
      // above already passed — disowning foreign rows IS the heal, stamping
      // them is the bug. World-scoped heuristics (no cc- key) return null here.
      const gate = nationalGateForSource(m.sourceId)
      if (gate && segmentWhollyOutside(gate, row.midLat, row.midLon, startLat, startLon, endLat, endLon)) {
        skippedForeign++
        continue
      }
      // Measured sources publish counts — an all-zero payload under a measured
      // id is a loader/join failure (the auditor's R7 zero-write shape), never
      // data: a closed road is ABSENT from a census, not surveyed as 0/0/0/0
      // (#31.4). Zero AADT stays legal for non-measured tiers: the R7 taper
      // (baseline rank) stamps speed-only rows with zero AADT by design.
      if (
        m.light === 0 && m.medium === 0 && m.heavy === 0 && m.moto === 0 &&
        isMeasured(m.sourceId)
      ) {
        throw new Error(`writeRoadAadt: all-zero AADT from measured source at row ${i} in ${arrowPath}: ${JSON.stringify(m)}`)
      }
      if (!shouldOverwrite(src[i], m.sourceId)) continue // priority gate OWNED here
      light[i] = m.light
      medium[i] = m.medium
      heavy[i] = m.heavy
      moto[i] = m.moto
      src[i] = m.sourceId
      // Derived-annotation rule (see RoadAadt.speedTaper): the write RESTATES
      // the taper refinement or clears it — new data voids a ramp computed
      // from the old state, so a census overwrite of a tapered row can never
      // strand a stale graded speed behind a fresh stamp.
      setTaper(i, m.speedTaper ?? 0)
      matched++
      onApplied?.(row, i, m)
    }
    if (matched === 0 && retracted === 0) return table
    const replacements: Record<string, Int32Array | Uint16Array | Uint8Array> = {
      aadt_light: light, aadt_medium: medium, aadt_heavy: heavy, aadt_moto: moto,
      source_id: src,
    }
    if (taper) replacements.speed_taper = taper
    // Compare the final typed values: retract/reclaim and integer coercion can
    // accept a match without changing what the next painter will read.
    updated = Object.entries(replacements).some(([name, values]) => {
      const existing = table.getChild(name)
      return values.some((value, i) => value !== (existing?.get(i) ?? 0))
    })
    if (!updated) return table

    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- Arrow mixes existing Vectors and typed columns.
    const cols: Record<string, any> = {}
    for (const field of table.schema.fields) {
      if (!Object.hasOwn(replacements, field.name)) cols[field.name] = table.getChild(field.name)!
    }
    for (const [name, values] of Object.entries(replacements)) cols[name] = makeVector(values)
    return makeTable(cols)
  })

  return { rows, matched, updated, skipped, skippedForeign, retracted }
}

/**
 * Hex ids whose H3 centroid is inside `bbox = [minLat, minLon, maxLat, maxLon]`
 * AND that contain `layerFile`. The bbox is MANDATORY: there is no "scan every
 * hex" entry point, so the CZ/europe full-planet scan (121k hexes, ~40 min)
 * cannot be written again. Replaces the bespoke readdir+cellToLatLng loop that
 * 9+ country enrichers each re-implemented.
 */
export function iterateCountryHexes(
  h3r4Dir: string,
  bbox: readonly [number, number, number, number],
  layerFile = 'roads.arrow',
): string[] {
  if (!existsSync(h3r4Dir)) return []
  const out: string[] = []
  for (const d of readdirSync(h3r4Dir)) {
    if (d.length !== 15 || !d.endsWith('ffffffff')) continue
    try {
      const [lat, lon] = cellToLatLng(d)
      if (inBbox(lat, lon, bbox) && existsSync(resolve(h3r4Dir, d, layerFile))) out.push(d)
    } catch {
      /* invalid h3 id */
    }
  }
  return out
}
