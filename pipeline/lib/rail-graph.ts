/**
 * Rail-segment graph — the ONE topology SSOT for the rail graph-walk matcher,
 * the R15/R16 continuity auditors and enrichment-status metrics.
 * Pure, no file I/O — callers own reading Arrow/GTFS and writing stamps.
 *
 * `buildRailGraph` interns segment endpoints via `nodeKey()` (spatial.ts) and
 * then performs T-JUNCTION HEALING: the extractor's collinear-merge
 * (`engine/osm-extract/src/microsegment.rs::split`) swallows junction
 * vertices, so a branch's endpoint can land in the *middle* of another
 * segment's body instead of exactly on it — an endpoint-only graph would
 * leave that branch dangling, unreachable from the segment it physically
 * meets. Healing finds those mid-body touches and splits the touched
 * segment into sub-edges that share the branch's node, all still tagged
 * with the ORIGINAL segment's `key` so a walked stamp maps back to one
 * Arrow row regardless of how many sub-edges it now spans.
 *
 * Routing (`walkRailStationPairs`), the parallel-spread pass and the R15/R16
 * detectors live in the sibling `rail-graph-metrics.ts` (kept out of this
 * file to stay near the ~300-line target) — they import the types and
 * `effectiveRailTraffic`/`buildRailGraph` from here; no logic is duplicated
 * between the two files.
 */

import { nodeKey, flatDist, pointToSegmentDist, pointToSegmentParamT, M_PER_DEG_LAT, M_PER_DEG_LON_EQ } from './spatial.js'

// ── Tunables (cited at each use site) ───────────────────────────────────────

/** Rail families the graph WALK routes and stamps: standard heavy rail (0)
 *  and narrow gauge (3). Narrow gauge added 2026-07-16 (CH Step A: 826 snap
 *  + 643 unlocalized failures = the metre-gauge networks — RhB/MGB/zb run
 *  >100 trains/day yet the engine's narrow-gauge class default is 10/day,
 *  so walking real GTFS counts onto them is a large accuracy win, not just
 *  a snap fix; same class as CZ's Osoblaha). GTFS route_type=2 trains do
 *  not distinguish gauge, and the shortest path virtually never crosses a
 *  gauge break (physically separate tracks; the rare dual-gauge section is
 *  genuinely shared). Trams/light rail (1/2) stay on the 500 m stop-join;
 *  funicular (4) carries no timetable counts worth walking. Code list must
 *  stay consistent with `defaultRailTraffic` below and the engine's
 *  `RailType::from_u8` (emission/railway.rs) — three hand-synced tables. */
export function isWalkableRailType(railType: number): boolean {
  return railType === 0 || railType === 3
}

/** Bit for `RailGraph.nodeFamilyMask` — which walkable FAMILY an edge
 *  belongs to. A walk must stay within ONE family (2026-07-16 Codex review
 *  C1): standard and narrow tracks can share an OSM node (joint stations,
 *  dual-gauge throats), and a family-blind Dijkstra would happily route a
 *  standard-gauge pair over a shorter narrow shortcut — a physically
 *  impossible gauge switch, stamping the wrong track. */
export function walkFamilyBit(railType: number): number {
  return railType === 0 ? 1 : railType === 3 ? 2 : 0
}
/** The walkable families a two-attempt walk tries, standard first. */
export const WALK_FAMILY_MASKS = [1, 2] as const

/** A GTFS/CZPTT stop's GPS must resolve to a graph node within this radius or
 *  the pair is dropped (never a chord fallback — see plan Key decisions). */
export const STATION_SNAP_RADIUS_M = 300
/** Hard walk-length bound: `WALK_DETOUR_RATIO * greatCircle + WALK_DETOUR_SLACK_M`.
 *  Rejects a chord-vs-meander mismatch when the graph does not actually run
 *  anywhere near the straight line (the Tochovice-Březnice failure mode). */
export const WALK_DETOUR_RATIO = 2.5
export const WALK_DETOUR_SLACK_M = 2000
/** Ambiguity proxy: an alternate path within 20% of the best path's length... */
export const WALK_AMBIGUITY_LENGTH_RATIO = 1.2
/** ...AND sharing less than half the best path's edges is a genuinely
 *  different route, not a local detour around one busy junction — the pair
 *  fails as 'ambiguous' rather than guessing which corridor carries the count. */
export const WALK_AMBIGUITY_SHARED_EDGE_FRACTION = 0.5
/** Soft corridor constraint: with a GTFS `shapes.txt` polyline, edges farther
 *  than this from the shape are excluded from that pair's search entirely. */
export const SHAPE_CORRIDOR_TOLERANCE_M = 250
/** T-junction healing tolerance — see the module doc. Deliberately tight (the
 *  extractor's own vertex precision), so this never merges two genuinely
 *  distinct nearby tracks. */
export const T_JUNCTION_TOLERANCE_M = 1.0
/** A rail-flow jump at a node within this radius of a real stop is explained
 *  by boardings/alightings, not a matching bug (R15/R16 exemption). */
export const RAIL_STOP_EXEMPT_RADIUS_M = 300
/** R15/R16 fire threshold: effective traffic must jump more than this
 *  multiple across a degree-2 node with no junction/stop to explain it. */
export const RAIL_JUMP_RATIO = 3
/** R15/R16 floor: below this effective trains/day, ratio noise (2 vs 7) is
 *  not worth flagging even past RAIL_JUMP_RATIO. */
export const RAIL_MIN_EFFECTIVE_TRAINS_PER_DAY = 20
/** Parallel-track sibling search radius (segment-midpoint distance, metres). */
export const PARALLEL_SPREAD_RADIUS_M = 50
/** Twin-track ambiguity exemption (2026-07-16 Step-B refinement; REDESIGNED
 *  the same day in the DE v3 tuning round): before failing a pair as
 *  'ambiguous', the walk tests whether the alt path found by the penalized
 *  re-run is merely the PARALLEL TWIN of the best path (the sibling track of
 *  a double/quad-track corridor) rather than a genuinely different corridor.
 *  The verdict is a LENGTH-WEIGHTED QUANTILE gate on ONE metric — the
 *  lateral distance from each non-shared stampable alt edge (at its
 *  midpoint) to the nearest best-path stampable edge (`twinGateMetrics`,
 *  rail-graph-metrics.ts — no heading filter: length aggregation already
 *  reads a crossing route as far, while a heading filter would misfile
 *  excursion transition ramps as FAR): twin iff the
 *  length-weighted MEDIAN lateral <= WALK_TWIN_MEDIAN_LATERAL_M AND at most
 *  WALK_TWIN_FAR_LENGTH_FRACTION of that length sits >=
 *  WALK_TWIN_FAR_LATERAL_M away. The failed-pair diagnostic records the
 *  SAME numbers (`ambiguousGeometry.twinGate`), so tuning input and tuned
 *  gate can never drift apart.
 *
 *  Provenance (DE Step A v2 sidecar, 1 775 ambiguous pairs, 2026-07-16):
 *  the mass is sibling tracks — per-pair MEDIAN lateral p75 = 5.2 m (real
 *  track spacing), plus 477 pairs at 25-50 m (S-Bahn systems on their own
 *  parallel alignment); genuine dual corridors (Rhine left/right bank) sit
 *  >= 200 m away for most of their length (87 pairs). What failed v2's gate
 *  was the excursion around German stations/yards: per-pair MAX lateral
 *  p25-p75 = 293-552 m, right at/over the old 300 m contiguous-run cap
 *  (calibrated on ~300 m CZ station throats — German throats and yard
 *  bypasses run 0.5-2 km). The quantile gate replaces BOTH the 0.8
 *  length-fraction and the contiguous-run cap: a bounded excursion (<= 10 %
 *  of length) no longer votes, while a genuinely different corridor still
 *  fails on the median (its alt runs far away for >= half its length). The
 *  CZ regressions that shaped the v2 gate keep their verdicts under 50 m
 *  (fixtures assert them): the 8-stubs-padding repro's length-weighted
 *  median is ~58 m (> 50 -> ambiguous), corridors 120 m apart median 120,
 *  Vršovice->Holešovice via Libeň vs via Bubny hundreds of metres, the
 *  30 m island-platform throat median 8 m (-> twin). Heading is NOT
 *  gated (see twinGateMetrics). NOT the spread's token arms (15/50 m): the
 *  SPREAD divides acoustic energy so it must stay strict; twin
 *  CLASSIFICATION only separates "same corridor" from "different
 *  corridor". */
export const WALK_TWIN_MEDIAN_LATERAL_M = 50
/** Length-weighted p75 gate — the guard for the 50-500 m middle band the
 *  median + far gates alone leave open (2026-07-16 Codex review C2: a
 *  disjoint alternate with 59% of its length at 8 m and 41% at 100-200 m
 *  passed as a twin — median 8, nothing FAR — despite plausibly being a
 *  genuine alternate alignment). At most a quarter of the alt's non-shared
 *  length may sit beyond this: bounded station/yard excursions (DE v2:
 *  excursion apexes 293-552 m over well under 25% of multi-km pairs) stay
 *  twins; an alt spending 25%+ of its length 120+ m away reads as a
 *  separate alignment and stays ambiguous. */
export const WALK_TWIN_P75_LATERAL_M = 120
/** Lateral distance beyond which an alt edge counts as FAR — plainly not
 *  running alongside the best path. Distances are clamped here (the exact
 *  value past the cap changes no verdict, and it bounds the grid ring
 *  search). NOTE the DE v2 numbers: max-lateral p75 = 552 m, so a quarter+
 *  of real excursions REACH past this value — those pairs pass not because
 *  of the clamp but because the far part stays under
 *  WALK_TWIN_FAR_LENGTH_FRACTION of their length. The clamp separates
 *  "near the corridor" from "plainly elsewhere"; the fraction is what
 *  tolerates bounded excursions. */
export const WALK_TWIN_FAR_LATERAL_M = 500
/** Maximum fraction of alt's non-shared stampable LENGTH allowed at or
 *  beyond WALK_TWIN_FAR_LATERAL_M: bounded station/yard excursions on
 *  multi-km station pairs stay well under this; a genuinely different
 *  corridor spends most of its length far away and fails long before the
 *  median gate even matters. */
export const WALK_TWIN_FAR_LENGTH_FRACTION = 0.10
/** Chord-vicinity band radius (`quarantineChordVicinity`) — TWO roles since
 *  the 2026-07-16 quarantine redesign: (a) the WHOLE quarantine of an
 *  UNLOCALIZED pair (neither endpoint snapped onto the graph — e.g. the
 *  Osoblaha narrow-gauge trains before rail_type 3 became walkable), and
 *  (b) the corridor-band half of `quarantineGraphlessPair`, unioned onto
 *  EVERY snapFailed/disconnected/detourRejected pair. Tuning it moves both.
 *  Every stampable segment whose midpoint lies within this distance of the
 *  pair's straight chord is quarantined (2026-07-16 Step-B refinement,
 *  plan item 2). An un-snappable pair can't localize a
 *  bounded graph search the way a snapped one can (there is no node to flood
 *  from), so its evidence is scoped by raw chord proximity instead — and,
 *  unlike the pre-refinement design, this withholding stays local to the
 *  chord's vicinity rather than suppressing the silent residual for the
 *  entire run. */
export const UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M = 5000

/** Grid cell size (degrees) for every proximity index in this module: the
 *  T-junction segment-body grid, the node snap grid and the rail-stops grid.
 *  ~1.1 km at the equator — far coarser than T_JUNCTION_TOLERANCE_M (so a
 *  handful of neighbour cells always cover it) and comfortably coarser than
 *  STATION_SNAP_RADIUS_M / RAIL_STOP_EXEMPT_RADIUS_M (both 300 m), so a
 *  ±1-cell neighbourhood already suffices in the common case; the snap/stop
 *  queries still compute the exact ring needed for the caller's radius.
 *
 *  UNWRAPPED at the antimeridian: every grid cell key below is derived from
 *  RAW lat/lon degrees (`Math.floor(lat/lon / SPATIAL_INDEX_CELL_DEG)`), with
 *  no ±180° wraparound — unlike spatial.ts's DISTANCE helpers
 *  (`flatDist`/`pointToSegmentDist`/`pointToSegmentParamT`), which do wrap. A
 *  segment/node/stop straddling ±180° would land in wildly different cells on
 *  each side and never find its true neighbours. No railway in the dataset
 *  crosses it; revisit if one ever does. */
const SPATIAL_INDEX_CELL_DEG = 0.01

// ── Segment input + graph shape ─────────────────────────────────────────────

export interface RailGraphSegmentInput {
  /** Caller-stable id, e.g. `${hexId}:${rowIdx}` — stamps key off this, not
   *  off any graph-internal (post-healing) edge id. */
  key: string
  osmId: string
  /** Engine codes: 0 rail, 1 tram, 2 light_rail, 3 narrow, 4 funicular. */
  railType: number
  /** 0 main, 1 branch, 2 industrial. */
  usage: number
  /** Crossover etc.: connects topology so a route CAN pass through it, but is
   *  never itself the recipient of a stamp. */
  isTraversalOnly: boolean
  /** `ref || name || ''` — the corridor identity used to gate parallel-track
   *  spread; '' means "no reliable corridor identity", see applyParallelSpread. */
  corridorToken: string
  startLat: number
  startLon: number
  endLat: number
  endLon: number
  lengthM: number
}

export interface RailGraphNode {
  lat: number
  lon: number
}

/** One (post-healing) graph edge. May be a whole input segment (no T-junction
 *  touched it) or one sub-edge of a segment that was split — `parentKey`
 *  always identifies the ORIGINAL `RailGraphSegmentInput.key` either way. */
export interface RailGraphEdge {
  nodeA: number
  nodeB: number
  lengthM: number
  parentKey: string
  osmId: string
  railType: number
  usage: number
  isTraversalOnly: boolean
  corridorToken: string
  startLat: number
  startLon: number
  endLat: number
  endLon: number
}

export interface RailGraph {
  nodeCount: number
  edgeCount: number
  // Internal structure consumed by rail-graph-metrics.ts (routing, parallel
  // spread, the R15/R16 detectors). Plain fields, not hidden behind a
  // private/symbol boundary, so the sibling module can build directly on top
  // of one graph instance instead of recomputing it.
  nodes: RailGraphNode[]
  edges: RailGraphEdge[]
  /** node id -> incident edge indices (both directions). */
  adjacency: number[][]
  /** node id -> component id, computed over the FULL healed edge set
   *  (every railType, including traversal-only) — a topological fact about
   *  the physical network, independent of which edges a given walk may
   *  route through. */
  componentOfNode: Int32Array
  /** grid cell ("latCell_lonCell") -> node ids, for snap queries. */
  nodeGrid: Map<string, number[]>
  /** node id -> OR of `walkFamilyBit` over its incident edges
   *  (traversal-only crossovers set BOTH bits — family-agnostic snap
   *  anchors): which walkable families a station can snap onto here. */
  nodeFamilyMask: Uint8Array
}

// ── Construction ─────────────────────────────────────────────────────────────

interface WorkEdge {
  nodeA: number
  nodeB: number
  lengthM: number
  parentKey: string
  osmId: string
  railType: number
  usage: number
  isTraversalOnly: boolean
  corridorToken: string
  startLat: number
  startLon: number
  endLat: number
  endLon: number
}

function gridCellsForBbox(minLat: number, minLon: number, maxLat: number, maxLon: number): string[] {
  const cells: string[] = []
  const laMin = Math.floor(minLat / SPATIAL_INDEX_CELL_DEG)
  const laMax = Math.floor(maxLat / SPATIAL_INDEX_CELL_DEG)
  const loMin = Math.floor(minLon / SPATIAL_INDEX_CELL_DEG)
  const loMax = Math.floor(maxLon / SPATIAL_INDEX_CELL_DEG)
  for (let la = laMin; la <= laMax; la++) {
    for (let lo = loMin; lo <= loMax; lo++) cells.push(`${la}_${lo}`)
  }
  return cells
}

/** T-junction healing (see module doc): grid-accelerated so a country-scale
 *  segment set doesn't pay an O(nodes * segments) scan. For every node, find
 *  segments whose body passes within T_JUNCTION_TOLERANCE_M and record the
 *  hit; once every node has been checked, split each touched segment at ALL
 *  its hits in one pass (collecting hits first, instead of splitting as we
 *  go, avoids the order-dependent bugs a live mutate-while-scanning approach
 *  would have — e.g. a second hit landing on a sub-edge created by the
 *  first). Sub-edges are pushed in geometric order (start -> ... -> end) so
 *  a caller can reconstruct one segment's overall geometry by taking the
 *  first and last sub-edge with its `parentKey` (see rail-graph-metrics.ts's
 *  `collectSegmentGeometry`). */
function healTJunctions(nodes: RailGraphNode[], workEdges: WorkEdge[]): RailGraphEdge[] {
  const segmentGrid = new Map<string, number[]>()
  workEdges.forEach((e, idx) => {
    const minLat = Math.min(e.startLat, e.endLat), maxLat = Math.max(e.startLat, e.endLat)
    const minLon = Math.min(e.startLon, e.endLon), maxLon = Math.max(e.startLon, e.endLon)
    for (const cell of gridCellsForBbox(minLat, minLon, maxLat, maxLon)) {
      const arr = segmentGrid.get(cell)
      if (arr) arr.push(idx); else segmentGrid.set(cell, [idx])
    }
  })

  const hits = new Map<number, Array<{ nodeId: number; t: number }>>()
  for (let nodeId = 0; nodeId < nodes.length; nodeId++) {
    const { lat, lon } = nodes[nodeId]
    const cy = Math.floor(lat / SPATIAL_INDEX_CELL_DEG)
    const cx = Math.floor(lon / SPATIAL_INDEX_CELL_DEG)
    const candidates = new Set<number>()
    for (let dy = -1; dy <= 1; dy++) {
      for (let dx = -1; dx <= 1; dx++) {
        const arr = segmentGrid.get(`${cy + dy}_${cx + dx}`)
        if (arr) for (const idx of arr) candidates.add(idx)
      }
    }
    for (const idx of candidates) {
      const e = workEdges[idx]
      if (e.nodeA === nodeId || e.nodeB === nodeId) continue // already an endpoint, not a T-junction
      const d = pointToSegmentDist(lat, lon, e.startLat, e.startLon, e.endLat, e.endLon)
      if (d > T_JUNCTION_TOLERANCE_M) continue
      const t = pointToSegmentParamT(lat, lon, e.startLat, e.startLon, e.endLat, e.endLon)
      if (t <= 1e-6 || t >= 1 - 1e-6) continue // projects onto an endpoint — nodeKey already merged this one
      const arr = hits.get(idx) ?? []
      arr.push({ nodeId, t })
      hits.set(idx, arr)
    }
  }

  const finalEdges: RailGraphEdge[] = []
  workEdges.forEach((e, idx) => {
    const hitList = hits.get(idx)
    if (!hitList || hitList.length === 0) {
      finalEdges.push({ ...e })
      return
    }
    hitList.sort((a, b) => a.t - b.t)
    const chain: Array<{ nodeId: number; t: number }> = [{ nodeId: e.nodeA, t: 0 }]
    for (const h of hitList) {
      if (Math.abs(h.t - chain[chain.length - 1].t) < 1e-9) continue // duplicate split point
      chain.push(h)
    }
    chain.push({ nodeId: e.nodeB, t: 1 })
    for (let i = 0; i < chain.length - 1; i++) {
      finalEdges.push({
        ...e,
        nodeA: chain[i].nodeId,
        nodeB: chain[i + 1].nodeId,
        lengthM: e.lengthM * (chain[i + 1].t - chain[i].t),
      })
    }
  })
  return finalEdges
}

/** BFS components over the FULL edge set (array-indexed queue — no
 *  `Array.shift()` per `enrich-roads-service-tree.ts::findComponents`'s
 *  documented reason: O(1) push/pop vs O(n) shift on a dense component). */
function computeComponents(nodeCount: number, adjacency: number[][], edges: RailGraphEdge[]): Int32Array {
  const componentOfNode = new Int32Array(nodeCount).fill(-1)
  let compId = 0
  const queue: number[] = []
  for (let start = 0; start < nodeCount; start++) {
    if (componentOfNode[start] !== -1) continue
    queue.length = 0
    queue.push(start)
    componentOfNode[start] = compId
    let head = 0
    while (head < queue.length) {
      const node = queue[head++]
      for (const edgeIdx of adjacency[node]) {
        const e = edges[edgeIdx]
        const other = e.nodeA === node ? e.nodeB : e.nodeA
        if (componentOfNode[other] === -1) {
          componentOfNode[other] = compId
          queue.push(other)
        }
      }
    }
    compId++
  }
  return componentOfNode
}

function buildNodeGrid(nodes: RailGraphNode[]): Map<string, number[]> {
  const grid = new Map<string, number[]>()
  for (let id = 0; id < nodes.length; id++) {
    const key = `${Math.floor(nodes[id].lat / SPATIAL_INDEX_CELL_DEG)}_${Math.floor(nodes[id].lon / SPATIAL_INDEX_CELL_DEG)}`
    const arr = grid.get(key)
    if (arr) arr.push(id); else grid.set(key, [id])
  }
  return grid
}

export function buildRailGraph(segments: RailGraphSegmentInput[]): RailGraph {
  const nodeIdByKey = new Map<string, number>()
  const nodes: RailGraphNode[] = []
  function internNode(lat: number, lon: number): number {
    const k = nodeKey(lat, lon)
    let id = nodeIdByKey.get(k)
    if (id === undefined) {
      id = nodes.length
      nodes.push({ lat, lon })
      nodeIdByKey.set(k, id)
    }
    return id
  }

  const workEdges: WorkEdge[] = segments.map((seg) => ({
    nodeA: internNode(seg.startLat, seg.startLon),
    nodeB: internNode(seg.endLat, seg.endLon),
    lengthM: seg.lengthM,
    parentKey: seg.key,
    osmId: seg.osmId,
    railType: seg.railType,
    usage: seg.usage,
    isTraversalOnly: seg.isTraversalOnly,
    corridorToken: seg.corridorToken,
    startLat: seg.startLat,
    startLon: seg.startLon,
    endLat: seg.endLat,
    endLon: seg.endLon,
  }))

  const edges = healTJunctions(nodes, workEdges)

  const adjacency: number[][] = Array.from({ length: nodes.length }, () => [])
  edges.forEach((e, idx) => {
    adjacency[e.nodeA].push(idx)
    if (e.nodeB !== e.nodeA) adjacency[e.nodeB].push(idx)
  })

  const componentOfNode = computeComponents(nodes.length, adjacency, edges)

  const nodeFamilyMask = new Uint8Array(nodes.length)
  for (const e of edges) {
    // Traversal-only crossovers are family-agnostic SNAP anchors (a station
    // GPS can sit nearest a throat node whose only edges are crossovers) —
    // they set BOTH bits. This cannot re-open a gauge switch: the walk's
    // family filter still rejects the other family's stampable edges on the
    // crossover's far side.
    const bit = e.isTraversalOnly ? 3 : walkFamilyBit(e.railType)
    nodeFamilyMask[e.nodeA] |= bit
    nodeFamilyMask[e.nodeB] |= bit
  }

  return {
    nodeCount: nodes.length,
    edgeCount: edges.length,
    nodes,
    edges,
    adjacency,
    componentOfNode,
    nodeGrid: buildNodeGrid(nodes),
    nodeFamilyMask,
  }
}

/** Box-scan a graph's node grid for the nearest node within `radiusM`
 *  (returns `null` when nothing falls inside the scanned box). Shared core
 *  of `snapToNearestRailGraphNode` (which additionally enforces `d <=
 *  radiusM` on the result — the box itself can extend slightly past a true
 *  circle at its corners) and `nearestRailGraphNodeDistanceM` (which wants
 *  the true nearest distance with NO radius cutoff at all, see that
 *  function's doc). */
function nearestNodeInGrid(graph: RailGraph, lat: number, lon: number, radiusM: number, familyMask = 0): { nodeId: number; distM: number } | null {
  const latSpanM = SPATIAL_INDEX_CELL_DEG * M_PER_DEG_LAT
  const lonSpanM = SPATIAL_INDEX_CELL_DEG * M_PER_DEG_LON_EQ * Math.max(0.05, Math.cos(lat * Math.PI / 180))
  const dyMax = Math.max(1, Math.ceil(radiusM / latSpanM))
  const dxMax = Math.max(1, Math.ceil(radiusM / lonSpanM))
  const gy = Math.floor(lat / SPATIAL_INDEX_CELL_DEG)
  const gx = Math.floor(lon / SPATIAL_INDEX_CELL_DEG)
  let best = -1
  let bestDist = Infinity
  for (let dy = -dyMax; dy <= dyMax; dy++) {
    for (let dx = -dxMax; dx <= dxMax; dx++) {
      const arr = graph.nodeGrid.get(`${gy + dy}_${gx + dx}`)
      if (!arr) continue
      for (const nodeId of arr) {
        if (familyMask !== 0 && (graph.nodeFamilyMask[nodeId] & familyMask) === 0) continue
        const n = graph.nodes[nodeId]
        const d = flatDist(lat, lon, n.lat, n.lon)
        if (d < bestDist) { bestDist = d; best = nodeId }
      }
    }
  }
  return best === -1 ? null : { nodeId: best, distM: bestDist }
}

/** Nearest graph node to `(lat, lon)` within `maxSnapM`, or -1. Never falls
 *  back to a chord match (Key decisions: "snap failure => drop pair"); the
 *  caller (walkRailStationPairs) records the failure instead of guessing. */
export function snapToNearestRailGraphNode(
  graph: RailGraph,
  lat: number,
  lon: number,
  maxSnapM: number = STATION_SNAP_RADIUS_M,
): number {
  const hit = nearestNodeInGrid(graph, lat, lon, maxSnapM)
  return hit && hit.distM <= maxSnapM ? hit.nodeId : -1
}

/** Family-filtered snap WITH the hit distance — the two-attempt walk (Codex
 *  C1) snaps each endpoint per walkable family and prefers the family whose
 *  stations sit closer to its own tracks (an RhB station GPS lies on the
 *  metre-gauge platform, metres from narrow track and tens of metres from
 *  any SBB node — snap distance IS the gauge evidence GTFS lacks). */
export function snapToNearestFamilyNode(
  graph: RailGraph,
  lat: number,
  lon: number,
  familyMask: number,
  maxSnapM: number = STATION_SNAP_RADIUS_M,
): { nodeId: number; distM: number } | null {
  const hit = nearestNodeInGrid(graph, lat, lon, maxSnapM, familyMask)
  return hit && hit.distM <= maxSnapM ? hit : null
}

/** DIAGNOSTIC-ONLY (DE Step A v2, 2026-07-16 failure analysis, fix 3): the
 *  TRUE distance to the nearest graph node, with no `STATION_SNAP_RADIUS_M`
 *  cutoff at all — unlike `snapToNearestRailGraphNode` (which by design
 *  never reports a near-miss, only pass/fail: "snap failure => drop pair"),
 *  a v3 twin-gate tuning pass needs to tell "this stop missed the 300 m
 *  radius by 20 m" from "this stop is 5 km from any rail at all", since the
 *  fix differs (raise the radius vs. the pair is simply not near this
 *  network). Called ONLY for a pair whose endpoint already failed to snap
 *  (never on the hot walk path).
 *
 *  Search: widen the box radius geometrically from `STATION_SNAP_RADIUS_M`
 *  (x4 per retry), CLAMPED to one final pass exactly AT `ceilingM` (Codex
 *  review item 2, 2026-07-16: a bare x4 ladder jumps 76.8 km -> 307 km and
 *  never scans the declared-ceiling band at all — verified: a node at
 *  99 486 m returned Infinity). On the first non-empty box, do ONE refining
 *  rescan with the hit's own distance as the radius (Codex review item 1:
 *  box != circle — the first non-empty BOX's best can be beaten by a nearer
 *  node in a cell the box did not cover, verified counterexample 1961.86 m
 *  reported where 1669.15 m exists; the refining box fully covers the circle
 *  of radius `hit.distM`, and the true nearest is <= that, so the rescan's
 *  best IS the true nearest). Returns `Infinity` for an empty graph or when
 *  every box up to and including the ceiling pass is empty. */
export function nearestRailGraphNodeDistanceM(
  graph: RailGraph,
  lat: number,
  lon: number,
  ceilingM: number = 200_000,
): number {
  if (graph.nodeCount === 0) return Infinity
  let radiusM = STATION_SNAP_RADIUS_M
  for (;;) {
    const hit = nearestNodeInGrid(graph, lat, lon, radiusM)
    if (hit) {
      // Refining rescan (item 1): `hit.distM` upper-bounds the true nearest,
      // and a box of that radius covers the whole circle it defines — the
      // rescan can never come back empty (the hit itself is inside it).
      return nearestNodeInGrid(graph, lat, lon, hit.distM)!.distM
    }
    if (radiusM >= ceilingM) return Infinity
    radiusM = Math.min(radiusM * 4, ceilingM)
  }
}

// ── Effective traffic (engine zero-defaulting mirror) ───────────────────────

/** Per-column class default, TS mirror of
 *  `engine/noise-compute/src/emission/railway.rs::default_traffic` — keep the
 *  two tables in sync by hand; there is no codegen link between them.
 *  `RailType::from_u8` maps ANY unrecognized code to `Rail`, so the `default`
 *  arm below covers both railType 0 and an out-of-range value on purpose. */
function defaultRailTraffic(railType: number, usage: number): [pax: number, frt: number] {
  switch (railType) {
    case 1: return [120, 0] // tram
    case 2: return [80, 0]  // light_rail
    case 3: return [10, 0]  // narrow_gauge
    case 4: return [40, 0]  // funicular
    default: // 0 = heavy rail, and the engine's from_u8 fallback for unknown codes
      switch (usage) {
        case 0: return [80, 20] // main
        case 1: return [30, 5]  // branch
        case 2: return [0, 15]  // industrial siding
        default: return [40, 10] // unknown usage
      }
  }
}

/** TS mirror of `normalize_rail`'s zero-defaulting + parallel-divisor scale:
 *  each of pax/frt independently falls back to its class default ONLY where
 *  the column itself is 0 (a real 0 is indistinguishable from "unmeasured" —
 *  this is the engine's own convention, not a choice made here), then both
 *  are divided by `max(1, parallelDivisor)`. Used by the R15/R16 detectors so
 *  they compare what the engine actually renders, not the raw stamped ints. */
export function effectiveRailTraffic(
  pax: number,
  frt: number,
  railType: number,
  usage: number,
  parallelDivisor: number,
): { pax: number; frt: number; total: number } {
  const [defPax, defFrt] = defaultRailTraffic(railType, usage)
  const divisor = Math.max(1, parallelDivisor)
  const effPax = (pax > 0 ? pax : defPax) / divisor
  const effFrt = (frt > 0 ? frt : defFrt) / divisor
  return { pax: effPax, frt: effFrt, total: effPax + effFrt }
}

// ── Rail-stops sidecar index (R15/R16 stop exemption) ───────────────────────

export interface RailStopsIndex {
  queryWithinRadius(lat: number, lon: number, radiusM: number): boolean
}

/** Grid-accelerated point-in-radius test over a rail-stops sidecar (station
 *  platforms), same cell scheme as the node/segment grids above. */
export function buildRailStopsIndex(stops: Array<{ lat: number; lon: number }>): RailStopsIndex {
  const grid = new Map<string, Array<{ lat: number; lon: number }>>()
  for (const s of stops) {
    const key = `${Math.floor(s.lat / SPATIAL_INDEX_CELL_DEG)}_${Math.floor(s.lon / SPATIAL_INDEX_CELL_DEG)}`
    const arr = grid.get(key)
    if (arr) arr.push(s); else grid.set(key, [s])
  }
  return {
    queryWithinRadius(lat: number, lon: number, radiusM: number): boolean {
      const latSpanM = SPATIAL_INDEX_CELL_DEG * M_PER_DEG_LAT
      const lonSpanM = SPATIAL_INDEX_CELL_DEG * M_PER_DEG_LON_EQ * Math.max(0.05, Math.cos(lat * Math.PI / 180))
      const dyMax = Math.max(1, Math.ceil(radiusM / latSpanM))
      const dxMax = Math.max(1, Math.ceil(radiusM / lonSpanM))
      const gy = Math.floor(lat / SPATIAL_INDEX_CELL_DEG)
      const gx = Math.floor(lon / SPATIAL_INDEX_CELL_DEG)
      for (let dy = -dyMax; dy <= dyMax; dy++) {
        for (let dx = -dxMax; dx <= dxMax; dx++) {
          const arr = grid.get(`${gy + dy}_${gx + dx}`)
          if (!arr) continue
          for (const s of arr) {
            if (flatDist(lat, lon, s.lat, s.lon) <= radiusM) return true
          }
        }
      }
      return false
    },
  }
}

// ── Types shared with rail-graph-metrics.ts (route/auditor inputs+outputs) ──

export interface RailStationPairCount {
  fromLat: number
  fromLon: number
  toLat: number
  toLon: number
  pax: number
  frt: number
  /** [lat, lon]; soft corridor constraint when present (GTFS `shapes.txt`). */
  shapePolyline?: Array<[number, number]>
}

export interface RailWalkResult {
  stampsBySegmentKey: Map<string, { pax: number; frt: number; divisor: number }>
  /** Parallel-track divisor for EVERY stampable segment that has >=1 sibling
   *  (`applyParallelSpread`'s sibling probe), INDEPENDENT of `stampsBySegmentKey`
   *  — a segment can have a sibling and carry zero traffic from either side
   *  (both unwalked) and still needs its divisor recorded, or a silent-residual
   *  stamp landing there later would render at the wrong (undivided) count.
   *  Absent key = no sibling found = divisor 1 (rail-walk-enrich.ts's silent
   *  branch reads this with `?? 1`). */
  divisorBySegmentKey: Map<string, number>
  failures: { snapFailed: number; disconnected: number; detourRejected: number; ambiguous: number }
  /** See `RailFailedPairRecord`'s doc for the reason-specific diagnostics
   *  each entry carries. */
  failedPairChords: RailFailedPairRecord[]
  /** Every stampable segment (`RailGraphSegmentInput.key`) inside a FAILED
   *  pair's own evidence region — strictly tighter than whole-component gating
   *  (2026-07-16 Step-B refinement, /gg fix batch item 4, review round +
   *  the same-day quarantine redesign). The shape follows the FAILURE
   *  REASON (see each function in rail-graph-metrics.ts):
   *  'ambiguous' -> the union of the pair's own candidate paths
   *  (`quarantineAmbiguousPathUnion`) — admissible corridors exist, the
   *  walk just can't pick one, so exactly those corridors are withheld.
   *  'detourRejected' / 'disconnected' / 'snapFailed' -> chord-vicinity
   *  band + capped graph fingers (`quarantineGraphlessPair`): no admissible
   *  GRAPH path exists, yet the timetable's trains run somewhere along the
   *  pair's corridor — the GRAPH is what failed, so silent/retract must
   *  stay away from it (Čelákovice–Čelákovice zastávka: a detour-rejected
   *  commuter line must not go silent at 2+1/day).
   *  NEITHER end snapped -> the chord band alone
   *  (`quarantineChordVicinity`, `UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M`).
   *  Retract and the silent residual (rail-walk-enrich.ts) withhold on
   *  SEGMENT membership here, never on the segment's whole component. */
  quarantinedSegmentKeys: Set<string>
  /** Pairs where NEITHER endpoint snapped onto the graph (both ends failed
   *  `snapToNearestRailGraphNode`) — quarantined via the chord-vicinity
   *  radius above (there is no node to flood a bounded search from), NOT by
   *  suppressing the silent residual globally (2026-07-16 Step-B refinement
   *  dropped that semantics — it quarantined entire clean runs behind one
   *  unrelated stray pair). This count is stats-only telemetry now. */
  unlocalizedPairs: number
  pairsWalked: number
  pairsTotal: number
}

/** Per-failed-pair chord + reason, EXTENDED with reason-specific diagnostics
 *  (DE Step A v2, 2026-07-16 failure analysis, fix 3) — the input a v3
 *  twin-gate tuning pass needs, computed ONLY for the failed pair that
 *  actually needs it (never on the hot walk path, per that failure
 *  analysis's "keep it cheap" constraint):
 *  - `ambiguousGeometry` ('ambiguous' only): how far apart (laterally) and
 *    how differently-oriented the alt path runs from the best path's own
 *    edges (`summarizeAmbiguousGeometry`, rail-graph-metrics.ts) — a small
 *    spread + a small heading delta that STILL failed the twin exemption
 *    (`altPathIsParallelTwin`) is exactly the signal `WALK_TWIN_*` tuning
 *    needs; a large spread confirms a genuinely different corridor.
 *  - `snapDistanceM` ('snapFailed', including the neither-end-snapped
 *    subset `unlocalizedPairs` counts): the TRUE distance from each
 *    unsnapped endpoint to the nearest graph node
 *    (`nearestRailGraphNodeDistanceM`, ignoring `STATION_SNAP_RADIUS_M`).
 *    Per endpoint: `null` = this end snapped fine (nothing to diagnose);
 *    a number = true metres to the nearest node; `'unreachable'` = nothing
 *    within that function's search ceiling. The string sentinel exists
 *    because these records are JSON-persisted (rail-stops sidecar) and
 *    `Infinity` serializes to `null` — which would silently re-label a
 *    totally-unreachable stop as "snapped fine" (Codex review item 3,
 *    2026-07-16).
 *  - `detourGeometry` ('detourRejected' only): the graph's own best route
 *    length vs the bound it failed — the tuning input for the detour
 *    ratio/slack (added 2026-07-16; CH Alpine rack/spiral lines
 *    legitimately exceed 2.5x on short chords).
 *  Absent for 'disconnected': a component split already names its own
 *  cause. */
export interface RailFailedPairRecord {
  fromLat: number; fromLon: number; toLat: number; toLon: number
  reason: 'snapFailed' | 'disconnected' | 'detourRejected' | 'ambiguous'
  ambiguousGeometry?: {
    lateralSpreadM: { min: number; median: number; max: number }
    headingDeltaDeg: number
    /** The twin GATE's own numbers for this pair (`twinGateMetrics` —
     *  length-weighted, FAR-clamped; no heading filter, see that function's
     *  doc), so a tuning pass reads exactly what the verdict was computed
     *  from. Absent when the alt path has no non-shared stampable length. */
    twinGate?: { medianLateralM: number; p75LateralM: number; farLengthFraction: number }
  }
  snapDistanceM?: { from: number | 'unreachable' | null; to: number | 'unreachable' | null }
  /** 'detourRejected' only: the graph's own best route length vs the bound
   *  it failed — the tuning input for the detour ratio/slack (CH Alpine
   *  rack/spiral lines legitimately exceed 2.5x on short chords). */
  detourGeometry?: { bestPathM: number; boundM: number }
}

export interface RailEndpointRow {
  key: string
  osmId: string
  railType: number
  usage: number
  service: number
  sourceId: number
  pax: number
  frt: number
  parallelDivisor: number
  startLat: number
  startLon: number
  endLat: number
  endLon: number
}

export interface RailContinuityViolation {
  endpointLat: number
  endpointLon: number
  aKey: string
  bKey: string
  aSourceId: number
  bSourceId: number
  ratio: number
  effA: { pax: number; frt: number; total: number }
  effB: { pax: number; frt: number; total: number }
  column: 'pax' | 'frt' | 'total'
}

/** The rail-stops sidecar on disk (`data/prepared/{year}/rail-stops/{scope}.json`)
 *  — written by exactly one writer (`rail-walk-enrich.ts`'s
 *  `writeStopsSidecarIfAny`) and read by exactly one reader
 *  (`rail-endpoint-rows.ts`'s `loadRailStopsIndex`, the R15/R16 stop
 *  exemption). Types live here (rail-graph.ts is the types home) so writer
 *  and reader can never drift on the envelope shape. */
export interface RailStopsSidecarV1 {
  version: 1
  year: string
  scope: string
  extractFingerprint: string
  feeds: string[]
  generatedAt: string
  stops: Array<{ lat: number; lon: number }>
  /** Every failed pair's chord + reason + diagnostics from THIS SAME walk
   *  (DE Step A v2, 2026-07-16 failure analysis, fix 3) — additive field on
   *  the ONE existing per-run JSON artifact a walk produces, so a v3
   *  twin-gate tuning pass can inspect WHY pairs failed without re-running
   *  the whole enrichment (no new file family). Optional: absent on a
   *  sidecar written before this field existed; `loadRailStopsIndex`
   *  (rail-endpoint-rows.ts, the R15/R16 stop exemption reader) never reads
   *  it, so that reader's contract is unaffected either way. */
  failedPairChords?: RailFailedPairRecord[]
}
