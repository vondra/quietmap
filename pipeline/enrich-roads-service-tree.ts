/**
 * Service-tree AADT enrichment for residential roads (v2: flow accumulation).
 *
 * Assigns each building to its nearest eligible segment, then uses multi-source
 * Dijkstra from root nodes (where residential meets higher-class roads) to orient
 * traffic flow. Accumulates trips bottom-up from leaves toward roots — dead-end
 * streets get only their local buildings, collector roads accumulate sub-branches.
 *
 * Trip generation per building lives in lib/trip-rates.ts (ITE-derived rates
 * for all building_type 0–13); the vehicle split and the residential
 * trips/dwelling multiplier are COUNTRY-dependent via the generated
 * lib/country-fleet.generated.ts table — interior hexes resolve their country
 * from h3r4-admin.bin, border hexes per segment midpoint through CGAZ
 * polygons (lib/hex-country.ts). The admin table is REQUIRED (fail-closed):
 * a missing table would silently stamp WORLD mix over the whole extract.
 *
 * Only modifies: road_class in [5..9] (local roads) whose provenance
 * service-tree may overwrite (shouldOverwrite gate — empty, self, or lower
 * rank; never measured census rows). Excludes motorway_link / trunk_link /
 * primary_link (10/11/12) — those carry highway flow that residential
 * accumulation drastically undercounts — and segments the engine's
 * normalize_road drops as non-emitting (tunnels, access=no): stamping those
 * would assign trips the acoustic model then throws away.
 * Sets source_id = service-tree-heuristic registry id (heuristic estimate).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-service-tree.ts
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-service-tree.ts --prefix 841e309
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-roads-service-tree.ts --bbox 17.5,-180,71.5,-65  # one country
 */

import { readFileSync, readdirSync, existsSync } from 'node:fs'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import { resolve } from 'node:path'
import { tableFromIPC, makeTable, makeVector } from 'apache-arrow'
import { SOURCE_ID_SERVICE_TREE_HEURISTIC } from './lib/source-ids.generated.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { nodeKey } from './lib/spatial.js'
import { MinHeap } from './lib/min-heap.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'
import { estimateBuildingLoad, type BuildingLoad } from './lib/trip-rates.js'
import { fleetForIso, type CountryFleet } from './lib/country-fleet.generated.js'
import { createHexCountryResolver, type HexCountryResolver } from './lib/hex-country.js'

const MY_SOURCE_ID = SOURCE_ID_SERVICE_TREE_HEURISTIC

const PREFIX = process.argv.includes('--prefix') ? process.argv[process.argv.indexOf('--prefix') + 1] : ''
const bboxArg = process.argv.includes('--bbox') ? process.argv[process.argv.indexOf('--bbox') + 1] : ''
const BBOX = bboxArg ? bboxArg.split(',').map(Number) as [number, number, number, number] : null
if (BBOX && (BBOX.length !== 4 || BBOX.some(n => !Number.isFinite(n)))) {
  console.error(`ERROR: --bbox must be minLat,minLon,maxLat,maxLon (got "${bboxArg}")`); process.exit(1)
}

const GRID_CELL = 0.0005       // building grid cell size in degrees (~55m at equator)

/**
 * Pack a (lat_idx, lon_idx) cell into one Smi-fitting number for
 * `Map<number, number[]>` lookup. The hot loop in `assignBuildingsGlobally`
 * visits hundreds of millions of cells per dense hex; a numeric key avoids
 * the string allocation a template literal would incur, and a *Smi-shaped*
 * numeric key avoids V8 promoting the Map to HeapNumber comparisons.
 *
 * Each component (latLocal, lonLocal) gets `GRID_KEY_BITS` of room. With
 * 14 bits per axis we can address ~16 k cells per dimension — at GRID_CELL
 * = 0.0005° that's ~880 km, far more than any single H3 r4 hex (~24 km).
 *
 * Indices are stored RELATIVE to a per-hex origin computed in
 * `buildBuildingGrid`, so they're always positive and stay well below
 * the bit limit. `(latLocal << GRID_KEY_BITS) | lonLocal` produces a
 * 28-bit unsigned value — comfortably inside V8's 31-bit Smi range.
 */
const GRID_KEY_BITS = 14
const GRID_KEY_MASK = (1 << GRID_KEY_BITS) - 1

/**
 * Building search radius in meters — only buildings within this distance of an
 * eligible road segment are attributed to it.
 *
 * Arbitrary; tuned by eye to approximate "one block frontage" (typical suburban
 * plot depth + setback). No standard backs this specific number — it is a
 * trade-off between capturing legitimate frontage and avoiding over-assignment
 * in dense grids.
 */
export const MAX_BUFFER_M = 50

// Residential trips/dwelling is COUNTRY-dependent (the "Commit 4b" per-country
// lookup the old global 3.68 constant deferred): lib/country-fleet.generated.ts
// carries vehicle_trips_per_occupied_dwelling per country with survey
// citations; WORLD_FLEET preserves the old 3.68 (4.0 base × 0.92 occupancy)
// exactly, so hexes without a country row behave as before.

/**
 * Floor for service-tree accumulated AADT — segments below this value are
 * clamped up to avoid degenerate 0-traffic rows.
 *
 * Arbitrary; chosen so the quietest dead-end cul-de-sac still emits at
 * a plausible "a few cars a day" level instead of zero.
 */
const MIN_AADT = 20

/**
 * A.3: per-class upper bound on service-tree accumulated trips. Not a
 * hierarchy-correct cap (1200 residential > 800 tertiary default is
 * intentional — dense urban residentials in Prague Karlín / Madrid Centro
 * reach 1500-2000 genuinely). This is a "pragmatic maximum" — anything
 * above it almost certainly means flow routing put too much through the
 * wrong segment. Class 8 (track) is excluded from eligibility entirely; no
 * cap entry needed.
 *
 * Ratios to `default_road_traffic` in engine: 5 residential 2.4×,
 * 6 living_street 2.5×, 7 service 1.6×, 9 unclassified 1.5×.
 *
 * Class 7 (service) is the one calibration outlier in the dict: service
 * roads cover everything from a 5-storey apartment driveway (~200 dw × 3.68
 * = 700 trips genuine) to a 30 m parking aisle (~5 trips). The OSM `service=*`
 * sub-tag would let us split these but the road schema doesn't preserve it
 * (engine/osm-extract/src/finalize.rs:124). Without that signal we pick a
 * cap of 400 — 1.6× the engine default of 250, leaves room for one mid-rise
 * apartment block, hard-clamps the Pasito-class 1700+ runaway. Apartment
 * driveways with >100 dw will still hit the cap; that's a known undercount
 * pending OSM `service` sub-tag extraction.
 */
export const SERVICE_TREE_CAP_PER_CLASS: Record<number, number> = {
  5: 1200,
  6: 250,
  7: 400,
  9: 2000,
}

// Medium/heavy shares stay world-constant (local roads carry few trucks; no
// per-country signal exists) — inherited from normalize.rs::
// default_road_traffic(5). The MOTO share comes from the per-country fleet
// table: the old SPLIT_MOTO = 0.01 wrote 3 moto/day onto Thai guesthouse
// roads carrying ~100 moto/hour (owner report 2026-07-16, Krabi).
const SPLIT_MEDIUM = 0.01
const SPLIT_HEAVY = 0.02

// ---------- Geometry ----------
//
// Service-tree's hot building-assignment loop runs ~3.7 G distance probes
// for a Jakarta-class hex. To avoid paying `Math.cos` + degree→metre
// arithmetic per probe, the inner loop runs in *pre-projected* metres:
// `buildBuildingGrid` materialises `xs/ys` `Float64Array` once using a
// single hex-level cosLat from the average building latitude, and
// `assignBuildingsGlobally` projects segment endpoints the same way.
// Within a 24 km H3 r4 hex cosLat varies <0.05 % — well under the 50 m
// `MAX_BUFFER_M` heuristic — so the assignment is bit-identical.

const M_PER_DEG_LON_EQUATOR = 111320
const M_PER_DEG_LAT = 110540

/**
 * Distance from point `p` to segment `a → b`, all coordinates already
 * projected to local metres by the caller. Pure subtract/multiply/dot;
 * no trig, no degree→metre conversion.
 */
function pointToSegmentDistXY(
  px: number, py: number,
  ax: number, ay: number,
  bx: number, by: number,
): number {
  const dx = bx - ax, dy = by - ay
  const lenSq = dx * dx + dy * dy
  if (lenSq < 1e-6) {
    const ex = px - ax, ey = py - ay
    return Math.sqrt(ex * ex + ey * ey)
  }
  let t = ((px - ax) * dx + (py - ay) * dy) / lenSq
  t = Math.max(0, Math.min(1, t))
  const cx = ax + t * dx, cy = ay + t * dy
  const ex = px - cx, ey = py - cy
  return Math.sqrt(ex * ex + ey * ey)
}


function packGridKey(latLocal: number, lonLocal: number): number {
  return (latLocal << GRID_KEY_BITS) | (lonLocal & GRID_KEY_MASK)
}

// Per-building trip generation (ITE-derived rates, all building_type 0–13)
// lives in lib/trip-rates.ts::estimateBuildingLoad — one table shared with its
// golden-delta tests and mirrored into SPEC.md.

export function splitAADT(
  totalTrips: number,
  fleet: CountryFleet,
): { light: number; medium: number; heavy: number; moto: number } {
  // Integerized at the source: the columns are Int32 and the idempotence
  // no-op compares stored ints against this candidate — a float `light`
  // (22 !== 22.08) would flag every re-run as changed (#31 round-2 Codex).
  // Everything upstream (building loads, flow accumulation) stays float;
  // this is the single rounding point, so per-generator rounding can never
  // inflate a street's sum (/gg Codex).
  const total = Math.round(Math.max(totalTrips, MIN_AADT))
  const medium = Math.round(total * SPLIT_MEDIUM)
  const heavy = Math.round(total * SPLIT_HEAVY)
  const moto = Math.round(total * fleet.motoTrafficShare)
  const light = total - medium - heavy - moto
  return { light, medium, heavy, moto }
}

// ---------- Graph ----------
//
// Nodes are interned to dense integer ids during `buildGraph`; everything
// downstream addresses them via `Int32Array` instead of string keys. For a
// 100 k-segment Praha hex that is ~7-10 MB of template-literal strings and
// ~1.5 M Map-of-string operations the engine no longer pays per pass.

export interface GraphNode {
  eligibleEdges: number[]
  // True iff the node touches a real motor-vehicle exit. Tracks (cls 8) are
  // NOT exits — counting them was the Pasito Blanco bug where service road
  // OSM 69951934 inflated from ~30 trips/day to 1700+ via fake-root flow.
  // See buildGraph() for the three sources that flip this flag.
  hasExitEdge: boolean
}

export interface Graph {
  nodes: GraphNode[]                    // indexed by node id
  segNodeIds: Int32Array                // length 2*n: [start_id, end_id, …]
  eligible: Uint8Array                  // 1 byte per segment
}

export function buildGraph(table: any): Graph {
  const n = table.numRows
  const startLat = table.getChild('start_lat')!
  const startLon = table.getChild('start_lon')!
  const endLat = table.getChild('end_lat')!
  const endLon = table.getChild('end_lon')!
  const roadClass = table.getChild('road_class')!
  const existingSourceId = table.getChild('source_id')
  const tunnelCol = table.getChild('tunnel')
  const accessCol = table.getChild('access')

  // Intern (lat, lon) pairs into dense ids 0..numNodes-1. The string key
  // is only used during construction; the rest of the pipeline never sees
  // it again.
  const nodeIdByKey = new Map<string, number>()
  const nodes: GraphNode[] = []

  function internNode(key: string): number {
    let id = nodeIdByKey.get(key)
    if (id === undefined) {
      id = nodes.length
      nodes.push({ eligibleEdges: [], hasExitEdge: false })
      nodeIdByKey.set(key, id)
    }
    return id
  }

  const segNodeIds = new Int32Array(n * 2)
  const eligible = new Uint8Array(n)

  for (let i = 0; i < n; i++) {
    const sKey = nodeKey(startLat.get(i) as number, startLon.get(i) as number)
    const eKey = nodeKey(endLat.get(i) as number, endLon.get(i) as number)
    const sId = internNode(sKey)
    const eId = internNode(eKey)
    segNodeIds[i * 2] = sId
    segNodeIds[i * 2 + 1] = eId

    const cls = (roadClass.get(i) as number) ?? 5
    const existingId = existingSourceId ? (existingSourceId.get(i) as number) ?? 0 : 0
    // Mirror the engine's normalize_road drop rule (normalize/road.rs): a
    // tunnel never emits, access=no / motor_vehicle_no (codes 2/4) emits only
    // with a MEASURED stamp — and service-tree is a heuristic, so a trip
    // assigned to such a segment is thrown away by the acoustic model. Keep
    // them out of eligibility (no assignment, no stamp); they still fall into
    // the exit branch below via `isLocalMotor`, which is right — traffic
    // continues through a tunnel / behind a gate, so flow drains toward it.
    // `tunnel` is an Arrow Bool column (returns boolean, NOT 0/1 — a `!== 0`
    // compare here once disqualified every segment, /gg diff review).
    const engineDropsEmission = tunnelCol ? Boolean(tunnelCol.get(i)) : false
    const access = accessCol ? ((accessCol.get(i) as number) ?? 0) : 0
    // Eligibility (in routing graph): local motor cls 5–9 *except* track. A.3:
    // tracks would pick up ~24/day from flow accumulation against a real ~1/day.
    // Links 10–12 excluded too — residential accumulation undercounts highway-
    // derived ramp traffic, so they stay at source_id=0 → engine class default.
    const isLocalMotor = cls >= 5 && cls <= 9 && cls !== 8
    if (isLocalMotor && !engineDropsEmission && access !== 2 && access !== 4
        && shouldOverwrite(existingId, MY_SOURCE_ID)) {
      eligible[i] = 1
      nodes[sId].eligibleEdges.push(i)
      nodes[eId].eligibleEdges.push(i)
    } else if (cls < 5 || (cls >= 10 && cls <= 12) || isLocalMotor) {
      // Real motor exit. Three sources fold here:
      //   - higher-class road (cls 0–4) or link (cls 10–12)
      //   - local motor non-overwriteable by us (already filled by measured
      //     source) — must still root the adjacent service-tree component, else
      //     pseudo-root pulls flow inward instead of out toward the measured neighbour.
      nodes[sId].hasExitEdge = true
      nodes[eId].hasExitEdge = true
    }
  }

  return { nodes, segNodeIds, eligible }
}

// ---------- Connected components ----------

export interface Component {
  segments: number[]
  rootNodes: Set<number>     // global node ids; small per component
}

export function findComponents(graph: Graph): Component[] {
  const { nodes, segNodeIds, eligible } = graph
  // Visited as Uint8Array (one byte per segment) — `Set<number>.has/add` runs
  // ~5–10× slower for the millions of probes a dense urban hex incurs.
  const visited = new Uint8Array(eligible.length)
  const components: Component[] = []

  // Queue is a plain array indexed by `head` so we never call `Array.shift()`
  // — V8's shift is O(n) per call, which made `findComponents` O(N²) on
  // giant components and was the dominant cost on dense urban hexes.
  const queue: number[] = []

  for (let i = 0; i < eligible.length; i++) {
    if (!eligible[i] || visited[i]) continue

    const comp: Component = { segments: [], rootNodes: new Set() }
    queue.length = 0
    queue.push(i)
    visited[i] = 1

    let head = 0
    while (head < queue.length) {
      const seg = queue[head++]
      comp.segments.push(seg)

      const sId = segNodeIds[seg * 2]
      const eId = segNodeIds[seg * 2 + 1]
      for (let endSel = 0; endSel < 2; endSel++) {
        const nodeId = endSel === 0 ? sId : eId
        const node = nodes[nodeId]
        if (node.hasExitEdge) {
          comp.rootNodes.add(nodeId)
        }
        const edges = node.eligibleEdges
        for (let k = 0; k < edges.length; k++) {
          const adj = edges[k]
          if (!visited[adj]) {
            visited[adj] = 1
            queue.push(adj)
          }
        }
      }
    }

    components.push(comp)
  }

  return components
}

// ---------- Building spatial grid ----------

export interface BuildingGrid {
  lats: Float64Array
  lons: Float64Array
  /** Pre-projected building coords in metres (using the per-hex `mPerDegLon`).
   *  Inner loop reads these instead of `lats/lons` so it never has to call
   *  `Math.cos` or multiply by `111320 * cosLat` per probe. */
  xs: Float64Array
  ys: Float64Array
  /** Hex-level metres-per-degree-longitude — `Math.cos(avgLat) * 111_320`.
   *  Caller projects segment endpoints with the same factor so building
   *  and segment coords share the same local frame. */
  mPerDegLon: number
  types: Uint8Array
  floors: Uint8Array
  areas: (number | null)[]
  // Per-hex local-coord origin (cell indices). Subtract before packing into
  // the grid key so values stay small and the resulting key fits V8 Smi.
  latOriginIdx: number
  lonOriginIdx: number
}

export function buildBuildingGrid(table: any): BuildingGrid {
  const n = table.numRows
  const latCol = table.getChild('centroid_lat')!
  const lonCol = table.getChild('centroid_lon')!
  const typeCol = table.getChild('building_type')!
  const floorCol = table.getChild('floors')!
  const areaCol = table.getChild('area_m2')

  const lats = new Float64Array(n)
  const lons = new Float64Array(n)
  const types = new Uint8Array(n)
  const flrs = new Uint8Array(n)
  const areas: (number | null)[] = new Array(n)

  // First pass: load coords + payload, track min cell idx for the per-hex
  // origin used by the Smi-fitting grid key, and accumulate the latitude
  // sum so we can derive a single `cosLat` for the whole hex.
  let minLatIdx = Infinity
  let minLonIdx = Infinity
  let latSum = 0
  for (let i = 0; i < n; i++) {
    const lat = latCol.get(i) as number
    const lon = lonCol.get(i) as number
    lats[i] = lat
    lons[i] = lon
    types[i] = (typeCol.get(i) as number) ?? 0
    flrs[i] = (floorCol.get(i) as number) ?? 0
    const a = areaCol?.get(i)
    areas[i] = a != null ? a as number : null
    const li = Math.floor(lat / GRID_CELL)
    const oi = Math.floor(lon / GRID_CELL)
    if (li < minLatIdx) minLatIdx = li
    if (oi < minLonIdx) minLonIdx = oi
    latSum += lat
  }
  // Pull origin one cell below the min so the segment-bbox lookup buffer
  // (latCells / lonCells) never produces a negative local coord.
  const latOriginIdx = (Number.isFinite(minLatIdx) ? minLatIdx : 0) - 1
  const lonOriginIdx = (Number.isFinite(minLonIdx) ? minLonIdx : 0) - 1

  // One cosLat for the whole hex. Across a 24 km r4 hex (≈0.22° lat span)
  // cos varies <0.05 % — well under the 50 m MAX_BUFFER_M threshold.
  const avgLat = n > 0 ? latSum / n : 0
  const mPerDegLon = M_PER_DEG_LON_EQUATOR * Math.cos(avgLat * Math.PI / 180)

  // Second pass: pre-project every building into local metres. (Spatial
  // bucketing now lives in assignBuildingsGlobally's per-segment grid; the
  // old per-building grid here was dead after the building-outer rewrite.)
  const xs = new Float64Array(n)
  const ys = new Float64Array(n)
  for (let i = 0; i < n; i++) {
    xs[i] = lons[i] * mPerDegLon
    ys[i] = lats[i] * M_PER_DEG_LAT
  }

  return {
    lats, lons, xs, ys, mPerDegLon,
    types, floors: flrs, areas,
    latOriginIdx, lonOriginIdx,
  }
}

// ---------- Flow accumulation per component ----------

/**
 * Assign every building to its single nearest eligible segment across the
 * whole hex (A.3: global bestSeg). Previously this ran per-component, so a
 * building within 50 m of segments in two disconnected components was
 * counted in each — inflating flow on both sides of a primary-road split.
 * One pass over all eligible segments + bucketed building grid.
 *
 * Returns `segIdx → {dwellings, trips}` (sums of `estimateBuildingLoad` for
 * every building whose closest eligible segment is `segIdx` — dwellings from
 * residential arms, direct trips from activity arms). Buildings outside
 * MAX_BUFFER_M from every eligible segment are simply omitted from the
 * totals. Per-component consumers (`flowAccumulate`) then look up the load
 * by segment in O(1) instead of re-iterating every building.
 */
export function assignBuildingsGlobally(
  eligibleSegments: number[],
  startLat: any, startLon: any, endLat: any, endLon: any,
  bg: BuildingGrid,
): Map<number, BuildingLoad> {
  const n = bg.lats.length
  const bestSegArr = new Int32Array(n).fill(-1)

  if (eligibleSegments.length === 0) return new Map()

  // Bbox padding: ±cells covering MAX_BUFFER_M of slop in each direction,
  // using the same hex-level `mPerDegLon` (= 111_320 × cosLat) the building
  // xs were projected with — keeps the bbox check coordinate-consistent.
  const lonCells = Math.ceil(MAX_BUFFER_M / (bg.mPerDegLon * GRID_CELL))
  const latCells = Math.ceil(MAX_BUFFER_M / (M_PER_DEG_LAT * GRID_CELL))

  const xs = bg.xs
  const ys = bg.ys
  const lats = bg.lats
  const lons = bg.lons
  const mLon = bg.mPerDegLon
  const latOff = bg.latOriginIdx
  const lonOff = bg.lonOriginIdx

  // Invert the old segment-outer scan, which probed every building in each
  // segment's bbox cells → O(segments × buildings_in_dense_core): quadratic
  // in metro density (4M buildings collapse to ~625/cell, every segment then
  // scans tens of thousands of mostly-out-of-range buildings) and the cause
  // of multi-hour hangs on the densest hexes. Instead bucket every eligible
  // segment into the grid cells its 250 m-capped extent (± buffer) covers,
  // then for each building gather only the segments registered in its own
  // cell. A building's candidate set is the handful of segments whose frontage
  // passes within MAX_BUFFER_M — O(1-few) in real street geometry (road
  // spacing bounds it, unlike building density) — so total work is ~O(buildings).
  //
  // Byte-identical to the old code: a segment is registered in cell C iff
  // C ∈ its bbox range (exactly when the old loop would have probed that
  // building for that segment), grid lists are filled in eligibleSegments
  // order so candidates are visited in that order, and the per-building loop
  // below uses the identical pointToSegmentDistXY + `dist < bestDist` compare —
  // so the assigned nearest segment per building is bit-identical (NOT merely
  // monotonic-equal: a squared compare could flip a sub-ulp near-tie), ties
  // resolved the same way (earliest eligible segment wins).
  const E = eligibleSegments.length
  const segIdA = new Int32Array(E)
  const sxA = new Float64Array(E)
  const syA = new Float64Array(E)
  const exA = new Float64Array(E)
  const eyA = new Float64Array(E)
  const segGrid = new Map<number, number[]>()

  for (let e = 0; e < E; e++) {
    const seg = eligibleSegments[e]
    segIdA[e] = seg
    const sLat = startLat.get(seg) as number
    const sLon = startLon.get(seg) as number
    const eLat = endLat.get(seg) as number
    const eLon = endLon.get(seg) as number
    sxA[e] = sLon * mLon
    syA[e] = sLat * M_PER_DEG_LAT
    exA[e] = eLon * mLon
    eyA[e] = eLat * M_PER_DEG_LAT

    const gMinLat = Math.floor(Math.min(sLat, eLat) / GRID_CELL) - latCells
    const gMaxLat = Math.floor(Math.max(sLat, eLat) / GRID_CELL) + latCells
    const gMinLon = Math.floor(Math.min(sLon, eLon) / GRID_CELL) - lonCells
    const gMaxLon = Math.floor(Math.max(sLon, eLon) / GRID_CELL) + lonCells

    for (let gLat = gMinLat; gLat <= gMaxLat; gLat++) {
      const latPart = (gLat - latOff) << GRID_KEY_BITS
      for (let gLon = gMinLon; gLon <= gMaxLon; gLon++) {
        const key = latPart | ((gLon - lonOff) & GRID_KEY_MASK)
        let list = segGrid.get(key)
        if (!list) { list = []; segGrid.set(key, list) }
        list.push(e)
      }
    }
  }

  for (let bi = 0; bi < n; bi++) {
    const key = ((Math.floor(lats[bi] / GRID_CELL) - latOff) << GRID_KEY_BITS)
      | ((Math.floor(lons[bi] / GRID_CELL) - lonOff) & GRID_KEY_MASK)
    const cands = segGrid.get(key)
    if (!cands) continue
    const bx = xs[bi]
    const by = ys[bi]
    let bestDist = Infinity
    let bestE = -1
    for (let c = 0; c < cands.length; c++) {
      const e = cands[c]
      const dist = pointToSegmentDistXY(bx, by, sxA[e], syA[e], exA[e], eyA[e])
      if (dist <= MAX_BUFFER_M && dist < bestDist) { bestDist = dist; bestE = e }
    }
    if (bestE >= 0) bestSegArr[bi] = segIdA[bestE]
  }

  // Aggregate to `seg → {dwellings, trips}` in one O(n) walk so each
  // component consumer can read its segments by O(1) lookup instead of
  // iterating every building. estimateBuildingLoad is invoked once per
  // assigned building (was once per component-membership previously, same
  // total). Dwellings stay integers (associative sums → order-independent);
  // trips are floats and round only at splitAADT.
  const segLoad = new Map<number, BuildingLoad>()
  for (let bi = 0; bi < n; bi++) {
    const seg = bestSegArr[bi]
    if (seg < 0) continue
    // estimateBuildingLoad returns a fresh object per call, so the first
    // building's load can be stored (and later mutated) directly.
    const load = estimateBuildingLoad(bg.types[bi], bg.floors[bi], bg.areas[bi])
    const acc = segLoad.get(seg)
    if (acc) {
      acc.dwellings += load.dwellings
      acc.trips += load.trips
    } else {
      segLoad.set(seg, load)
    }
  }
  return segLoad
}

export function flowAccumulate(
  comp: Component,
  segNodeIds: Int32Array,
  lengthCol: any,
  segLoadGlobal: Map<number, BuildingLoad>,
  fleetForSeg: (seg: number) => CountryFleet,
): Map<number, number> {
  // Component-local node ids: dense 0..K-1, mapped from the global ids
  // that appear in this component's segments. Per-component dense ids let
  // the Dijkstra distance / parent / sorted state live in `Float64Array`
  // / `Int32Array` instead of `Map<string, …>`, which was the hottest
  // remaining service-tree path on dense urban hexes.
  const globalToLocal = new Map<number, number>()
  const localToGlobal: number[] = []
  const localAdj: number[][] = []
  function intern(globalId: number): number {
    let local = globalToLocal.get(globalId)
    if (local === undefined) {
      local = localToGlobal.length
      localToGlobal.push(globalId)
      localAdj.push([])
      globalToLocal.set(globalId, local)
    }
    return local
  }

  // Build component-local adjacency keyed by dense local ids.
  const segLocalEnds: { a: number; b: number }[] = new Array(comp.segments.length)
  for (let i = 0; i < comp.segments.length; i++) {
    const seg = comp.segments[i]
    const a = intern(segNodeIds[seg * 2])
    const b = intern(segNodeIds[seg * 2 + 1])
    segLocalEnds[i] = { a, b }
    localAdj[a].push(seg)
    localAdj[b].push(seg)
  }
  // segIdx -> (localA, localB): keyed by global seg index so Step 2/3 can
  // look up the two endpoints without touching the global Int32Array.
  const segLocalLookup = new Map<number, { a: number; b: number }>()
  for (let i = 0; i < comp.segments.length; i++) {
    segLocalLookup.set(comp.segments[i], segLocalEnds[i])
  }

  const numLocal = localToGlobal.length

  // --- Step 1: pull per-component local trips out of the global
  // segment→load map. Each segment is only ever in one component's
  // segments list, so this is a direct lookup. Multiplying integer
  // dwellings by the country trips/dwelling once per segment keeps segFlow
  // independent of building-iteration order (integer addition associative,
  // floats aren't); activity trips are already per-segment sums.
  const segFlow = new Map<number, number>()
  for (const seg of comp.segments) {
    const load = segLoadGlobal.get(seg)
    segFlow.set(seg, load ? load.dwellings * fleetForSeg(seg).tripsPerDwelling + load.trips : 0)
  }

  // --- Step 2: Multi-source Dijkstra from root nodes ---
  const dist = new Float64Array(numLocal)
  dist.fill(Infinity)
  const downSeg = new Int32Array(numLocal)
  downSeg.fill(-1)

  // Translate root nodes to local ids; also handle the "no roots → pick
  // highest-degree node as pseudo-root" fallback in local space.
  const localRoots: number[] = []
  for (const globalId of comp.rootNodes) {
    const local = globalToLocal.get(globalId)
    if (local !== undefined) localRoots.push(local)
  }
  if (localRoots.length === 0) {
    let best = 0, bestDeg = -1
    for (let l = 0; l < numLocal; l++) {
      const d = localAdj[l].length
      if (d > bestDeg) { bestDeg = d; best = l }
    }
    localRoots.push(best)
  }

  const pq = new MinHeap()
  for (const r of localRoots) { dist[r] = 0; pq.push(0, r) }

  while (pq.size > 0) {
    const { dist: d, node: u } = pq.pop()
    if (d > dist[u]) continue

    const edges = localAdj[u]
    for (let k = 0; k < edges.length; k++) {
      const seg = edges[k]
      const ends = segLocalLookup.get(seg)!
      const v = ends.a === u ? ends.b : ends.a
      const len = Math.max(1, (lengthCol.get(seg) as number) ?? 1)
      const newDist = d + len
      if (newDist < dist[v]) {
        dist[v] = newDist
        downSeg[v] = seg
        pq.push(newDist, v)
      }
    }
  }

  // --- Step 3: Bottom-up accumulation ---
  // Indices 0..numLocal-1 sorted by descending dist — leaves first, roots
  // last. Backed by an `Int32Array` so the sort comparator only touches
  // primitive Float64 reads.
  const sortedLocals = new Int32Array(numLocal)
  for (let i = 0; i < numLocal; i++) sortedLocals[i] = i
  // Convert to a regular array for sort (TypedArray sort is numeric-only;
  // we want comparator-based descending-by-dist). Numeric ids → no string
  // hash work in the comparator.
  const sortedArr = Array.from(sortedLocals)
  sortedArr.sort((a, b) => dist[b] - dist[a])

  for (const u of sortedArr) {
    let inflow = 0
    const edges = localAdj[u]
    const distU = dist[u]
    for (let k = 0; k < edges.length; k++) {
      const seg = edges[k]
      const ends = segLocalLookup.get(seg)!
      const other = ends.a === u ? ends.b : ends.a
      if (dist[other] > distU) {
        inflow += segFlow.get(seg)!
      }
    }

    const dSeg = downSeg[u]
    if (dSeg !== -1) {
      segFlow.set(dSeg, segFlow.get(dSeg)! + inflow)
    }
  }

  return segFlow
}

// ---------- Debug hook ----------

function parseDebugOsmId(): number | null {
  const raw = process.env.DEBUG_OSM_ID
  if (!raw) return null
  const n = Number(raw)
  if (!Number.isFinite(n)) {
    console.error(`[service-tree] DEBUG_OSM_ID=${raw} not numeric — ignored`)
    return null
  }
  return n
}

function debugFlow(
  segFlow: Map<number, number>,
  osmIdCol: any,
  target: number,
  segLoad: Map<number, BuildingLoad>,
  fleetForSeg: (seg: number) => CountryFleet,
) {
  for (const [seg, trips] of segFlow) {
    if (Number(osmIdCol.get(seg)) !== target) continue
    const load = segLoad.get(seg)
    const localTrips = load ? load.dwellings * fleetForSeg(seg).tripsPerDwelling + load.trips : 0
    console.error(`  [DEBUG seg ${seg} osm=${target}] local_dw=${load?.dwellings ?? 0} local_activity_trips=${(load?.trips ?? 0).toFixed(1)} local_trips=${localTrips.toFixed(1)} TOTAL_FLOW=${trips.toFixed(0)} through_flow=${(trips - localTrips).toFixed(0)}`)
  }
}

// ---------- Process one hex ----------

/** #31.5: the whole read→compute→write runs INSIDE `withArrowWrite` — the same
 *  advisory-lockfile (PID-liveness stale recovery) + tmp + rename + schema/batch-shape preservation every other road
 *  writer uses. The previous raw `writeFileSync(tableToIPC(...))` dropped
 *  schema metadata and collapsed record batches (voiding the extractors'
 *  `qm_batch_bboxes` popup pruning), had no lock against a concurrent writer,
 *  and re-serialized the file even when no row changed. Returning the input
 *  table on any no-op path keeps the hex byte-identical. */
async function processHex(
  hexId: string,
  countryResolver: HexCountryResolver,
): Promise<{ enriched: number; totalResidential: number } | null> {
  const roadsPath = resolve(H3R4_DIR, hexId, 'roads.arrow')
  const buildingsPath = resolve(H3R4_DIR, hexId, 'buildings.arrow')
  if (!existsSync(roadsPath) || !existsSync(buildingsPath)) return null

  let result: { enriched: number; totalResidential: number } | null = null
  await withArrowWrite(roadsPath, (roadTable) => {
    const n = roadTable.numRows
    if (n === 0) return roadTable

    const buildingTable = tableFromIPC(readFileSync(buildingsPath))
    if (buildingTable.numRows === 0) return roadTable

    const graph = buildGraph(roadTable)

    let eligibleCount = 0
    for (let i = 0; i < n; i++) if (graph.eligible[i]) eligibleCount++
    if (eligibleCount === 0) return roadTable

    const components = findComponents(graph)
    if (components.length === 0) return roadTable

    const bg = buildBuildingGrid(buildingTable)
    const startLat = roadTable.getChild('start_lat')!
    const startLon = roadTable.getChild('start_lon')!
    const endLat = roadTable.getChild('end_lat')!
    const endLon = roadTable.getChild('end_lon')!
    const lengthCol = roadTable.getChild('length_m')!

    // A.3: global building→segment assignment. One pass over every eligible
    // segment across all components — each building now lands on exactly one
    // segment, no more double-counting across primary-road-split components.
    // (Spread `push(...comp.segments)` overflows the V8 call stack when a
    // single urban component holds >100 k segments — use explicit loops.)
    let eligibleCapacity = 0
    for (const comp of components) eligibleCapacity += comp.segments.length
    const eligibleSegments: number[] = new Array(eligibleCapacity)
    let writeIdx = 0
    for (const comp of components) {
      const segs = comp.segments
      for (let i = 0; i < segs.length; i++) eligibleSegments[writeIdx++] = segs[i]
    }
    const segLoadGlobal = assignBuildingsGlobally(
      eligibleSegments, startLat, startLon, endLat, endLon, bg,
    )

    // Country context: interior hexes use one fleet row for every segment
    // (the overwhelmingly common case — zero per-segment cost); border hexes
    // resolve each segment's midpoint through the CGAZ candidate polygons,
    // memoized per segment (each segment is consulted by both the flow seed
    // and splitAADT).
    const hexFleet = fleetForIso(countryResolver.hexIso(hexId))
    let fleetForSeg: (seg: number) => CountryFleet
    if (!countryResolver.isBorderHex(hexId)) {
      fleetForSeg = () => hexFleet
    } else {
      const segFleetCache = new Map<number, CountryFleet>()
      fleetForSeg = (seg) => {
        let fleet = segFleetCache.get(seg)
        if (fleet === undefined) {
          const midLat = (((startLat.get(seg) as number) ?? 0) + ((endLat.get(seg) as number) ?? 0)) / 2
          const midLon = (((startLon.get(seg) as number) ?? 0) + ((endLon.get(seg) as number) ?? 0)) / 2
          fleet = fleetForIso(countryResolver.isoAt(hexId, midLat, midLon))
          segFleetCache.set(seg, fleet)
        }
        return fleet
      }
    }
    // Flow accumulation per component, reading the precomputed seg→load
    // map by direct lookup (no per-component re-scan of every building).
    const segAADT = new Map<number, { light: number; medium: number; heavy: number; moto: number }>()
    const roadClassCol = roadTable.getChild('road_class')
    const debugTarget = parseDebugOsmId()
    const osmIdCol = debugTarget !== null ? roadTable.getChild('osm_id') : undefined
    for (const comp of components) {
      const segFlow = flowAccumulate(comp, graph.segNodeIds, lengthCol, segLoadGlobal, fleetForSeg)
      if (osmIdCol) debugFlow(segFlow, osmIdCol, debugTarget!, segLoadGlobal, fleetForSeg)
      for (const [seg, trips] of segFlow) {
        const cls = (roadClassCol?.get(seg) as number) ?? 5
        const capped = Math.min(trips, SERVICE_TREE_CAP_PER_CLASS[cls] ?? Infinity)
        segAADT.set(seg, splitAADT(capped, fleetForSeg(seg)))
      }
    }

    if (segAADT.size === 0) return roadTable

    // Write back — EC pattern: copy existing values first
    const existingLight = roadTable.getChild('aadt_light')
    const existingMed = roadTable.getChild('aadt_medium')
    const existingHvy = roadTable.getChild('aadt_heavy')
    const existingMoto = roadTable.getChild('aadt_moto')
    const existingSourceId = roadTable.getChild('source_id')
    const aadtLight = new Int32Array(n)
    const aadtMedium = new Int32Array(n)
    const aadtHeavy = new Int32Array(n)
    const aadtMoto = new Int32Array(n)
    const sourceId = new Uint16Array(n)

    for (let i = 0; i < n; i++) {
      aadtLight[i] = (existingLight?.get(i) as number) ?? 0
      aadtMedium[i] = (existingMed?.get(i) as number) ?? 0
      aadtHeavy[i] = (existingHvy?.get(i) as number) ?? 0
      aadtMoto[i] = (existingMoto?.get(i) as number) ?? 0
      sourceId[i] = existingSourceId ? (existingSourceId.get(i) as number) ?? 0 : 0
    }

    // speed_taper is a derived annotation (see lib/roads-arrow.ts RoadAadt):
    // claiming a row VOIDS any taper ramp computed from its old state — this
    // bulk path must clear it exactly like the shared writer does, or a stale
    // graded speed would hide behind the service-tree stamp (/gg Codex).
    const existingTaper = roadTable.getChild('speed_taper')
    let taperCol: Uint8Array | null = null

    let enriched = 0
    let valueChanged = false
    for (const [seg, aadt] of segAADT) {
      // Eligibility was already gated via shouldOverwrite() in buildGraph().
      // Whole-row atomic write — payload + dataset_id together.
      if (!shouldOverwrite(sourceId[seg], MY_SOURCE_ID)) continue
      if (
        aadtLight[seg] !== aadt.light || aadtMedium[seg] !== aadt.medium ||
        aadtHeavy[seg] !== aadt.heavy || aadtMoto[seg] !== aadt.moto ||
        sourceId[seg] !== MY_SOURCE_ID
      ) valueChanged = true
      aadtLight[seg] = aadt.light
      aadtMedium[seg] = aadt.medium
      aadtHeavy[seg] = aadt.heavy
      aadtMoto[seg] = aadt.moto
      sourceId[seg] = MY_SOURCE_ID
      if (existingTaper && ((existingTaper.get(seg) as number) ?? 0) !== 0) {
        if (!taperCol) {
          taperCol = new Uint8Array(n)
          for (let k = 0; k < n; k++) taperCol[k] = (existingTaper.get(k) as number) ?? 0
        }
        taperCol[seg] = 0
      }
      enriched++
    }

    // Convergence sweep: rows WE stamped on an earlier run that are no longer
    // eligible (tunnel/access exclusion added 2026-07, or a class/provenance
    // change since) must be RETRACTED to empty, or a re-run would preserve
    // stamps a fresh extract could never produce (Dobříš alone carried 722
    // such tunnel/access rows, /gg diff review). Second run sees source_id 0
    // → skips → byte-identical, so idempotence is preserved.
    for (let i = 0; i < n; i++) {
      if (sourceId[i] !== MY_SOURCE_ID || graph.eligible[i] === 1) continue
      valueChanged = true
      aadtLight[i] = 0
      aadtMedium[i] = 0
      aadtHeavy[i] = 0
      aadtMoto[i] = 0
      sourceId[i] = 0
      if (existingTaper && ((existingTaper.get(i) as number) ?? 0) !== 0) {
        if (!taperCol) {
          taperCol = new Uint8Array(n)
          for (let k = 0; k < n; k++) taperCol[k] = (existingTaper.get(k) as number) ?? 0
        }
        taperCol[i] = 0
      }
    }

    // Idempotent re-run: buildGraph pre-gates shouldOverwrite, so every
    // accepted row usually just RESTATES what is already on disk (the old
    // `enriched === 0` no-op was unreachable for non-empty segAADT — /gg #31).
    // Compare values instead: nothing moved and no taper cleared → return the
    // input table and withArrowWrite leaves the file byte-identical, which is
    // what keeps a world-chain re-run from rewriting every hex it visits.
    if (!valueChanged && !taperCol) {
      result = { enriched, totalResidential: eligibleCount }
      return roadTable
    }

    const columns: Record<string, any> = {}
    const rebuilt = ['aadt_light', 'aadt_medium', 'aadt_heavy', 'aadt_moto', 'source_id']
    if (taperCol) rebuilt.push('speed_taper')
    for (const field of roadTable.schema.fields) {
      if (rebuilt.includes(field.name)) continue
      columns[field.name] = roadTable.getChild(field.name)!
    }
    columns['aadt_light'] = makeVector(aadtLight)
    columns['aadt_medium'] = makeVector(aadtMedium)
    columns['aadt_heavy'] = makeVector(aadtHeavy)
    columns['aadt_moto'] = makeVector(aadtMoto)
    columns['source_id'] = makeVector(sourceId)
    if (taperCol) columns['speed_taper'] = makeVector(taperCol)
    result = { enriched, totalResidential: eligibleCount }
    return makeTable(columns)
  })
  return result
}

// ---------- Main ----------

async function main() {
  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // --bbox scopes to one region via the shared iterateCountryHexes (cellToLatLng
  // + inBbox, hexes with roads.arrow inside the box) — used by the road re-stamp
  // to refill only the fixed country; else the full tree (optionally a --prefix
  // slice). Sorted so START_INDEX / SHARD slicing is reproducible across runs
  // (readdirSync order isn't filesystem-guaranteed).
  const hexDirs = (BBOX
    ? iterateCountryHexes(H3R4_DIR, BBOX)
    : readdirSync(H3R4_DIR).filter(d => !d.startsWith('.') && (!PREFIX || d.startsWith(PREFIX)))
  ).sort()
  const rawStart = parseInt(process.env.START_INDEX || '0', 10)
  let START_INDEX = Number.isFinite(rawStart)
    ? Math.min(Math.max(0, rawStart), hexDirs.length)
    : 0
  let END_INDEX = hexDirs.length

  // SHARD="i/n" splits the sorted hex list into n disjoint contiguous slices for n
  // parallel processes. Safe because processHex reads/writes only its own hex's arrows
  // (no cross-slice file contention); the per-hex flow accumulation itself is single-threaded.
  if (process.env.SHARD) {
    const m = /^(\d+)\/(\d+)$/.exec(process.env.SHARD)
    const i = m ? Number(m[1]) : NaN
    const n = m ? Number(m[2]) : NaN
    if (!m || n <= 0 || i >= n) {
      console.error(`ERROR: invalid SHARD="${process.env.SHARD}" (expected i/n with 0 <= i < n)`)
      process.exit(1)
    }
    START_INDEX = Math.floor((i * hexDirs.length) / n)
    END_INDEX = Math.floor(((i + 1) * hexDirs.length) / n)
  }

  let rangeSuffix = ''
  if (process.env.SHARD) rangeSuffix = ` | shard ${process.env.SHARD} → [${START_INDEX}, ${END_INDEX})`
  else if (START_INDEX > 0) rangeSuffix = ` (resume from #${START_INDEX})`

  // FAIL-CLOSED: national fleet mix + trips/dwelling depend on the hex→country
  // table; running without it would silently stamp WORLD values over the whole
  // extract (/gg Codex CRITICAL). osm-to-h3r4.sh builds it before this pass.
  const countryResolver = createHexCountryResolver(resolve(H3R4_DIR, '..', '..', 'h3r4-admin.bin'))

  // Staleness guard: a WORLD-scope run against an admin table from an older
  // extract would leave every hex the old table doesn't know on WORLD fleet.
  // Coastal hexes legitimately miss centroids (their centre is over water),
  // so the gate is a coverage share, and scoped runs (--bbox/--prefix over
  // coastal regions like Krabi) only warn.
  const covered = hexDirs.reduce((acc, h) => acc + (countryResolver.hexIso(h) !== undefined ? 1 : 0), 0)
  if (hexDirs.length > 0 && covered / hexDirs.length < 0.5) {
    const msg = `h3r4-admin.bin covers only ${covered}/${hexDirs.length} hexes in scope — stale or foreign table?`
    if (!BBOX && !PREFIX) throw new Error(`${msg} Rebuild it: cd scripts && DATA_YEAR=${YEAR} npm run build:h3-admin`)
    console.error(`WARNING: ${msg}`)
  }

  console.log(`Service-tree AADT enrichment (v2: flow accumulation)`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Hexes: ${hexDirs.length} total${PREFIX ? ` (prefix: ${PREFIX})` : ''}${BBOX ? ` (bbox: ${BBOX.join(',')})` : ''}${rangeSuffix}`)

  const startTime = Date.now()
  let lastProgress = startTime
  let hexesProcessed = START_INDEX
  let hexesEnriched = 0
  let totalSegmentsEnriched = 0
  let totalResidential = 0

  for (let hi = START_INDEX; hi < END_INDEX; hi++) {
    const hexId = hexDirs[hi]
    const result = await processHex(hexId, countryResolver)

    if (result) {
      hexesEnriched++
      totalSegmentsEnriched += result.enriched
      totalResidential += result.totalResidential
    }
    hexesProcessed++

    const now = Date.now()
    if (now - lastProgress >= 10_000) {
      lastProgress = now
      const elapsed = ((now - startTime) / 1000).toFixed(0)
      process.stdout.write(`\r  [${elapsed}s] ${hexesProcessed}/${END_INDEX} hexes, ${hexesEnriched} enriched, ${totalSegmentsEnriched} segments`)
    }
  }

  const elapsed = ((Date.now() - startTime) / 1000).toFixed(1)
  console.log(`\n\n=== Results (${elapsed}s) ===`)
  console.log(`  Hexes: ${hexesProcessed} processed, ${hexesEnriched} enriched`)
  console.log(`  Segments: ${totalSegmentsEnriched} enriched / ${totalResidential} eligible residential`)
  if (totalResidential > 0) {
    console.log(`  Coverage: ${(totalSegmentsEnriched / totalResidential * 100).toFixed(1)}%`)
  }
}

// Run main only when this file is invoked as a script — not when imported by tests.
if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((err) => {
    console.error('Error:', err)
    process.exit(1)
  })
}
