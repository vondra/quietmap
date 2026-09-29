/** Railway routing of station pairs on source-connected graphs; railways-finalize allocates over parallel tracks. */

import { haversineM, flatDist, pointToSegmentDist, pointToPolylineDist, wrapLonDeltaDeg, M_PER_DEG_LAT, M_PER_DEG_LON_EQ } from './spatial.js'
import { RailPairSearches } from './rail-pair-searches.js'
import { MinHeap } from './min-heap.js'
import {
  type RailGraph, type RailGraphEdge, type RailStationPairCount, type RailWalkResult, type RailFailedPairRecord,
  snapToNearestRailGraphNode, snapToNearestFamilyNode, nearestRailGraphNodeDistanceM,
  isWalkableRailType, walkFamilyBit, WALK_FAMILY_MASKS,
  WALK_DETOUR_RATIO, WALK_DETOUR_SLACK_M, WALK_AMBIGUITY_LENGTH_RATIO, WALK_AMBIGUITY_SHARED_EDGE_FRACTION,
  WALK_TWIN_MEDIAN_LATERAL_M, WALK_TWIN_P75_LATERAL_M, WALK_TWIN_FAR_LATERAL_M, WALK_TWIN_FAR_LENGTH_FRACTION, UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M,
  SHAPE_CORRIDOR_TOLERANCE_M,
} from './rail-graph.js'

// ── Bounded shortest path ────────────────────────────────────────────────────

/** Edge-reuse penalty for "find a different route" re-runs — shared by the
 *  ambiguity probe in `walkRailStationPairs` and
 *  `quarantineAmbiguousPathUnion`'s widening probes (which its doc calls
 *  "the same penalized re-run"): retuning one must retune the other. */
const PATH_REUSE_PENALTY = 3

/** Only walkable-family edges (heavy rail + narrow gauge — see
 *  `isWalkableRailType`) + traversal-only edges (crossovers) ever
 *  participate in a walk — a tram/light_rail/funicular edge can neither
 *  carry nor bridge a train-count station pair. */
function isRoutable(e: RailGraphEdge): boolean {
  return e.isTraversalOnly || isWalkableRailType(e.railType)
}

/** The stampable universe — a walkable-family edge that is a real track
 *  (never a traversal-only crossover). The AND-NOT sibling of `isRoutable`;
 *  every quarantine/twin/spread pass filters on exactly this. */
function isStampableRailEdge(e: { railType: number; isTraversalOnly: boolean }): boolean {
  return !e.isTraversalOnly && isWalkableRailType(e.railType)
}

interface ShortestPathResult {
  lengthM: number
  edgeIndices: Set<number>
  orderedEdges: number[]
}

/** Reusable Dijkstra scratch buffers, sized to ONE graph's `nodeCount`. REAL
 *  scale blocker this fixes: a Europe union graph (~5M nodes) x ~1e5 station
 *  pairs x 2 searches per pair (best path + the ambiguity probe's penalized
 *  re-run) would spend ~1e12 ops just re-filling `dist`/`prevEdge`/`prevNode`/
 *  `visited` from scratch on every call. `walkRailStationPairs` allocates ONE
 *  scratch per graph and threads it through every search on that graph;
 *  `touched` is how a search's cleanup cost scales with what IT visited,
 *  never with `nodeCount`. */
export interface DijkstraScratch {
  dist: Float64Array
  prevEdge: Int32Array
  prevNode: Int32Array
  visited: Uint8Array
  /** Node ids written by the search IN PROGRESS — drained (and used to reset
   *  the four arrays above) at the START of the next call that reuses this
   *  scratch, so a leftover value from a previous search can never leak into
   *  the next one. */
  touched: number[]
}

export function createDijkstraScratch(nodeCount: number): DijkstraScratch {
  return {
    dist: new Float64Array(nodeCount).fill(Infinity),
    prevEdge: new Int32Array(nodeCount).fill(-1),
    prevNode: new Int32Array(nodeCount).fill(-1),
    visited: new Uint8Array(nodeCount),
    touched: [],
  }
}

function resetDijkstraScratch(scratch: DijkstraScratch): void {
  for (const node of scratch.touched) {
    scratch.dist[node] = Infinity
    scratch.prevEdge[node] = -1
    scratch.prevNode[node] = -1
    scratch.visited[node] = 0
  }
  scratch.touched.length = 0
}

/** Dijkstra with an injectable edge weight (so the ambiguity probe can
 *  penalize the best path's own edges 3x without a second graph copy) and an
 *  optional edge filter (the shape-polyline soft corridor constraint).
 *  `lengthM` on the result is always the TRUE physical length recomputed
 *  from `edge.lengthM` during backtrack — independent of whatever weight
 *  function drove the search — so a penalized re-run still reports a
 *  comparable real-world distance.
 *
 *  `scratch`: pass one from `createDijkstraScratch` to reuse its buffers
 *  across many searches on the SAME graph (only the cells the PREVIOUS
 *  search touched are reset, never the full `nodeCount`-sized arrays) —
 *  omit it to allocate a throwaway scratch for one standalone call. Passing
 *  a scratch never changes the result, only where the buffers come from. */
export function dijkstraShortestPath(
  graph: RailGraph,
  fromNode: number,
  toNode: number,
  edgeFilter: ((edge: RailGraphEdge) => boolean) | null,
  edgeWeight: (edgeIdx: number, edge: RailGraphEdge) => number,
  scratch?: DijkstraScratch,
): ShortestPathResult | null {
  if (scratch) resetDijkstraScratch(scratch)
  const { dist, prevEdge, prevNode, visited } = scratch ?? createDijkstraScratch(graph.nodeCount)
  const touch = (node: number): void => { if (scratch) scratch.touched.push(node) }

  dist[fromNode] = 0
  touch(fromNode)
  const heap = new MinHeap()
  heap.push(0, fromNode)

  while (heap.size > 0) {
    const { dist: d, node } = heap.pop()
    if (visited[node]) continue
    visited[node] = 1
    if (node === toNode) break
    for (const edgeIdx of graph.adjacency[node]) {
      const e = graph.edges[edgeIdx]
      if (!isRoutable(e)) continue
      if (edgeFilter && !edgeFilter(e)) continue
      const other = e.nodeA === node ? e.nodeB : e.nodeA
      if (visited[other]) continue
      const nd = d + edgeWeight(edgeIdx, e)
      if (nd < dist[other]) {
        if (dist[other] === Infinity) touch(other) // first time this search reaches `other`
        dist[other] = nd
        prevEdge[other] = edgeIdx
        prevNode[other] = node
        heap.push(nd, other)
      }
    }
  }

  if (dist[toNode] === Infinity) return null

  const orderedEdges: number[] = []
  let lengthM = 0
  let cur = toNode
  while (cur !== fromNode) {
    const edgeIdx = prevEdge[cur]
    if (edgeIdx === -1) return null // defensive: should be unreachable given dist[toNode] finite
    orderedEdges.push(edgeIdx)
    lengthM += graph.edges[edgeIdx].lengthM
    cur = prevNode[cur]
  }
  orderedEdges.reverse()
  return { lengthM, edgeIndices: new Set(orderedEdges), orderedEdges }
}

// ── Segment geometry ─────────────────────────────────────────────────────────

/** Indexes each source piece's edge by `key`. Construction emits
 *  exactly one edge per input, so callers use the edge geometry as-is. */
function collectSegmentGeometry(graph: RailGraph): Map<string, RailGraphEdge> {
  const map = new Map<string, RailGraphEdge>()
  for (const e of graph.edges) map.set(e.key, e)
  return map
}

/** Compass heading (degrees, 0-360) of a->b using the same flat-earth
 *  projection as spatial.ts, including its antimeridian wrap (a corridor
 *  straddling ±180° must not compute a ~180°-off heading from the raw
 *  unwrapped delta). Only ever compared mod 180 (see headingDeltaMod180), so
 *  a-b vs b-a direction never matters. */
function headingDeg(aLat: number, aLon: number, bLat: number, bLon: number): number {
  const cosLat = Math.cos(((aLat + bLat) / 2) * Math.PI / 180)
  const dx = wrapLonDeltaDeg(bLon - aLon) * M_PER_DEG_LON_EQ * cosLat
  const dy = (bLat - aLat) * M_PER_DEG_LAT
  return (Math.atan2(dx, dy) * 180 / Math.PI + 360) % 360
}

function headingDeltaMod180(h1: number, h2: number): number {
  const diff = Math.abs(h1 - h2) % 180
  return Math.min(diff, 180 - diff)
}

/** Grid cell for the twin-gate lateral search. ~110 m of latitude — segment
 *  BODIES are indexed by their bbox cells (a 250 m microsegment spans a
 *  handful), the query ring around a midpoint is computed latitude-aware per
 *  query, so a body within the search radius always shares a ring cell.
 *  UNWRAPPED at the antimeridian, like rail-graph.ts's SPATIAL_INDEX_CELL_DEG
 *  grids (same reasoning: no railway segment in the dataset crosses ±180°;
 *  revisit if one ever does). */
const TWIN_GATE_GRID_CELL_DEG = 0.001

// ── Twin-track ambiguity exemption ──────────────────────────────────────────

/** Length-weighted distance from non-shared alternative edges to the best
 *  path. Classification and failure diagnostics share these metrics. Unlike
 *  acoustic allocation, this permits station throats and short yard excursions.
 *  Traversal-only differences have no stampable length and return null. */
function twinGateMetrics(
  graph: RailGraph, best: ShortestPathResult, alt: ShortestPathResult,
): { medianLateralM: number; p75LateralM: number; farLengthFraction: number } | null {
  const bestStampableIndices: number[] = []
  for (const idx of best.edgeIndices) {
    const e = graph.edges[idx]
    if (isStampableRailEdge(e)) bestStampableIndices.push(idx)
  }

  // Grid-accelerate over best's own (small) edge set: bbox cell -> edge indices.
  const bestGrid = new Map<string, number[]>()
  for (const idx of bestStampableIndices) {
    const e = graph.edges[idx]
    const laMin = Math.floor(Math.min(e.startLat, e.endLat) / TWIN_GATE_GRID_CELL_DEG)
    const laMax = Math.floor(Math.max(e.startLat, e.endLat) / TWIN_GATE_GRID_CELL_DEG)
    const loMin = Math.floor(Math.min(e.startLon, e.endLon) / TWIN_GATE_GRID_CELL_DEG)
    const loMax = Math.floor(Math.max(e.startLon, e.endLon) / TWIN_GATE_GRID_CELL_DEG)
    for (let la = laMin; la <= laMax; la++) {
      for (let lo = loMin; lo <= loMax; lo++) {
        const cell = `${la}_${lo}`
        const arr = bestGrid.get(cell)
        if (arr) arr.push(idx); else bestGrid.set(cell, [idx])
      }
    }
  }

  // No explicit empty-bestStampable guard needed: an empty grid makes every
  // alt edge read as FAR -> farLengthFraction 1 -> "not a twin", matching
  // the old explicit `return false`.
  const samples: Array<{ lateralM: number; lenM: number }> = []
  let totalLenM = 0
  let farLenM = 0
  for (const altIdx of alt.edgeIndices) {
    if (best.edgeIndices.has(altIdx)) continue // shared with best — not part of "how different is alt"
    const e = graph.edges[altIdx]
    if (!isStampableRailEdge(e)) continue
    totalLenM += e.lengthM

    const midLat = (e.startLat + e.endLat) / 2
    const midLon = (e.startLon + e.endLon) / 2
    const latSpanM = TWIN_GATE_GRID_CELL_DEG * M_PER_DEG_LAT
    const lonSpanM = TWIN_GATE_GRID_CELL_DEG * M_PER_DEG_LON_EQ * Math.max(0.05, Math.cos(midLat * Math.PI / 180))
    const dyMax = Math.max(1, Math.ceil(WALK_TWIN_FAR_LATERAL_M / latSpanM))
    const dxMax = Math.max(1, Math.ceil(WALK_TWIN_FAR_LATERAL_M / lonSpanM))
    const gy = Math.floor(midLat / TWIN_GATE_GRID_CELL_DEG)
    const gx = Math.floor(midLon / TWIN_GATE_GRID_CELL_DEG)

    let nearestLateralM = WALK_TWIN_FAR_LATERAL_M // FAR clamp — "no best edge anywhere near"
    const probed = new Set<number>()
    for (let dy = -dyMax; dy <= dyMax; dy++) {
      for (let dx = -dxMax; dx <= dxMax; dx++) {
        const arr = bestGrid.get(`${gy + dy}_${gx + dx}`)
        if (!arr) continue
        for (const bestIdx of arr) {
          if (probed.has(bestIdx)) continue
          probed.add(bestIdx)
          const b = graph.edges[bestIdx]
          const d = pointToSegmentDist(midLat, midLon, b.startLat, b.startLon, b.endLat, b.endLon)
          if (d < nearestLateralM) nearestLateralM = d
        }
      }
    }
    if (nearestLateralM >= WALK_TWIN_FAR_LATERAL_M) farLenM += e.lengthM
    samples.push({ lateralM: nearestLateralM, lenM: e.lengthM })
  }

  if (totalLenM === 0) return null

  samples.sort((a, b) => a.lateralM - b.lateralM)
  // STRICTLY-greater-than quantile crossings (2026-07-16 Codex review INFO
  // 1): at an exact 50/50 near/far split the FAR half decides — "a genuine
  // corridor runs far for >= half its length" must fail the gate at the
  // boundary, matching the constant's doc.
  let cumLenM = 0
  let medianLateralM = samples[samples.length - 1].lateralM
  let p75LateralM = samples[samples.length - 1].lateralM
  let medianSet = false
  for (const s of samples) {
    cumLenM += s.lenM
    if (!medianSet && cumLenM > totalLenM / 2) { medianLateralM = s.lateralM; medianSet = true }
    if (cumLenM > totalLenM * 0.75) { p75LateralM = s.lateralM; break }
  }
  return { medianLateralM, p75LateralM, farLengthFraction: farLenM / totalLenM }
}

/** Classify the corridor independently of the stricter acoustic allocation radii. */
function altPathIsParallelTwin(metrics: ReturnType<typeof twinGateMetrics>): boolean {
  if (metrics === null) return true // alt differs from best only via traversal-only crossovers — trivially the same route
  return metrics.medianLateralM <= WALK_TWIN_MEDIAN_LATERAL_M &&
    metrics.p75LateralM <= WALK_TWIN_P75_LATERAL_M &&
    metrics.farLengthFraction <= WALK_TWIN_FAR_LENGTH_FRACTION
}

/** Find a comparably short alternative corridor, excluding the same line's parallel tracks. */
export function findAmbiguousRailAlternative(
  graph: RailGraph, fromNode: number, toNode: number, best: ShortestPathResult,
  filter: ((edge: RailGraphEdge) => boolean) | null, scratch: DijkstraScratch,
): { path: ShortestPathResult; twinGate: ReturnType<typeof twinGateMetrics> } | null {
  const alt = dijkstraShortestPath(graph, fromNode, toNode, filter,
    (index, edge) => edge.lengthM * (best.edgeIndices.has(index) ? PATH_REUSE_PENALTY : 1), scratch)
  if (!alt || alt.lengthM > WALK_AMBIGUITY_LENGTH_RATIO * best.lengthM || best.edgeIndices.size === 0) return null
  let shared = 0
  for (const index of alt.edgeIndices) if (best.edgeIndices.has(index)) shared++
  if (shared / best.edgeIndices.size >= WALK_AMBIGUITY_SHARED_EDGE_FRACTION) return null
  const twinGate = twinGateMetrics(graph, best, alt)
  return altPathIsParallelTwin(twinGate) ? null : { path: alt, twinGate }
}

/** Failure diagnostics reuse the corridor classification's metrics. */
function summarizeAmbiguousGeometry(
  graph: RailGraph, best: ShortestPathResult, alt: ShortestPathResult,
  twinGate: ReturnType<typeof twinGateMetrics>,
): RailFailedPairRecord['ambiguousGeometry'] {
  const bestStampableIndices: number[] = []
  for (const idx of best.edgeIndices) {
    const e = graph.edges[idx]
    if (isStampableRailEdge(e)) bestStampableIndices.push(idx)
  }
  if (bestStampableIndices.length === 0) return undefined

  const lateralByEdgeM: number[] = []
  let closestLateralM = Infinity
  let headingDeltaAtClosestDeg = 0
  for (const altIdx of alt.edgeIndices) {
    if (best.edgeIndices.has(altIdx)) continue // shared with best — not part of the "how different is alt" question
    const e = graph.edges[altIdx]
    if (!isStampableRailEdge(e)) continue
    const midLat = (e.startLat + e.endLat) / 2
    const midLon = (e.startLon + e.endLon) / 2
    const hAlt = headingDeg(e.startLat, e.startLon, e.endLat, e.endLon)

    let nearestLateralM = Infinity
    let nearestHeadingDeltaDeg = 0
    for (const bIdx of bestStampableIndices) {
      const b = graph.edges[bIdx]
      const d = pointToSegmentDist(midLat, midLon, b.startLat, b.startLon, b.endLat, b.endLon)
      if (d < nearestLateralM) {
        nearestLateralM = d
        nearestHeadingDeltaDeg = headingDeltaMod180(hAlt, headingDeg(b.startLat, b.startLon, b.endLat, b.endLon))
      }
    }
    if (nearestLateralM === Infinity) continue
    lateralByEdgeM.push(nearestLateralM)
    if (nearestLateralM < closestLateralM) {
      closestLateralM = nearestLateralM
      headingDeltaAtClosestDeg = nearestHeadingDeltaDeg
    }
  }
  if (lateralByEdgeM.length === 0) return undefined

  lateralByEdgeM.sort((a, b) => a - b)
  return {
    lateralSpreadM: {
      min: lateralByEdgeM[0],
      median: lateralByEdgeM[Math.floor(lateralByEdgeM.length / 2)],
      max: lateralByEdgeM[lateralByEdgeM.length - 1],
    },
    headingDeltaDeg: headingDeltaAtClosestDeg,
    // The gate's own numbers — the SAME object the twin verdict was computed
    // from (threaded by the caller, never re-derived).
    twinGate: twinGate ?? undefined,
  }
}

// ── Per-pair failure quarantine ─────────────────────────────────────────────

/** Bounded Dijkstra distance flood — no target node, no backtrack. Returns
 *  node -> TRUE path cost for every node settled within `boundM`; once a
 *  popped node's OWN distance exceeds the bound, nothing reachable through
 *  it can still be inside, so relaxation stops there. Reuses `scratch`
 *  exactly like `dijkstraShortestPath` (reset at entry, `touched` drained by
 *  the next reuse); the result Map is copied OUT of the scratch so a second
 *  flood on the same scratch (the ellipse case below) can't clobber it. */
function floodNodeDistancesWithinBound(
  graph: RailGraph, fromNode: number, boundM: number, scratch: DijkstraScratch,
): Map<number, number> {
  resetDijkstraScratch(scratch)
  const { dist, visited } = scratch
  const within = new Map<number, number>()
  dist[fromNode] = 0
  scratch.touched.push(fromNode)
  const heap = new MinHeap()
  heap.push(0, fromNode)

  while (heap.size > 0) {
    const { dist: d, node } = heap.pop()
    if (visited[node]) continue
    visited[node] = 1
    if (d > boundM) continue // beyond the bound — record nothing, relax nothing further
    within.set(node, d)

    for (const edgeIdx of graph.adjacency[node]) {
      const e = graph.edges[edgeIdx]
      if (!isRoutable(e)) continue
      const other = e.nodeA === node ? e.nodeB : e.nodeA
      if (visited[other]) continue
      const nd = d + e.lengthM
      if (nd < dist[other]) {
        if (dist[other] === Infinity) scratch.touched.push(other)
        dist[other] = nd
        heap.push(nd, other)
      }
    }
  }
  return within
}

/** GRAPHLESS-PAIR quarantine — the shape for snapFailed / disconnected /
 *  detourRejected (2026-07-16 DE/NL quarantine redesign, third iteration):
 *  those pairs' trains exist and only the GRAPH failed to place them, so
 *  the evidence region must not depend on an admissible graph path
 *  existing. Two honest, TIGHT parts, unioned:
 *
 *  1. GRAPH FINGERS from each snapped end: a bounded distance flood, but a
 *     node u qualifies only when
 *     `distGraph(focus, u) + distGeo(u, otherFocus) <= bound` — the
 *     straight-line remainder is a lower bound on ANY real path through u,
 *     so this is still an upper bound on where a within-bound train path
 *     can run, yet it hugs actual corridors instead of flooding a disc.
 *     Iteration history: a plain flood-ball (radius = the full bound,
 *     second focus ignored) blanketed whole national networks off a single
 *     cross-border leg (DE: quarantinedKm pinned at ~95 000 km); the pure
 *     GEOMETRIC two-focus ellipse fixed that but stayed near-circular for
 *     the typical short-chord pair (bound = 2.5c + 2 km makes the minor
 *     axis ~sqrt((2.5c+2)^2 - c^2) ~ 2.3c + slack) and ~100 failed pairs
 *     still tiled most of a small country (NL: 18 653 of ~19 750 km).
 *  2. CHORD VICINITY (`quarantineChordVicinity`, the unlocalized pair's own
 *     shape): covers the direct physical corridor even where the graph is
 *     fragmented — a live line's missing MIDDLE piece belongs to no
 *     flooded component (the CH Berner-Oberland fragmentation case) yet
 *     must stay protected from the silent residual.
 *
 *  An end that did not snap contributes no finger (there is no node to
 *  flood from); its GPS still serves as the other focus of the snapped
 *  end's finger criterion, and the chord band still covers its corridor. */
/** Graph-reach cap for one finger's flood (metres). A DELIBERATE narrowing
 *  of the bound-honest criterion (distGraph + geoRemainder <= the pair's
 *  own bound): segments beyond 50 km of graph reach that the criterion
 *  alone would admit are NOT quarantined. Trade-off accepted 2026-07-16 —
 *  without the cap a single long leg (DE: one 679 km Basel->Hamburg
 *  night-train leg, bound ~1 700 km) floods a whole national network back
 *  to the blanket the redesign removes; the residual exposure (a mid-haul
 *  pair's admissible corridor 50+ km from both stations AND 5+ km off the
 *  chord) carries marginal counts on corridors that dense walked short
 *  legs cover independently. */
const FINGER_FLOOD_CAP_M = 50_000

function quarantineGraphlessPair(
  graph: RailGraph,
  geomByKey: Map<string, RailGraphEdge>,
  cp: { fromLat: number; fromLon: number; toLat: number; toLon: number },
  fromNode: number,
  toNode: number,
  boundM: number,
  scratch: DijkstraScratch,
  quarantine: Set<string>,
): void {
  quarantineChordVicinity(geomByKey, cp, boundM, quarantine)
  const fingers: Array<[number, number, number]> = [
    [fromNode, cp.toLat, cp.toLon],
    [toNode, cp.fromLat, cp.fromLon],
  ]
  for (const [node, otherLat, otherLon] of fingers) {
    if (node === -1) continue
    const within = floodNodeDistancesWithinBound(graph, node, Math.min(boundM, FINGER_FLOOD_CAP_M), scratch)
    for (const [u, d] of within) {
      const nu = graph.nodes[u]
      if (d + flatDist(nu.lat, nu.lon, otherLat, otherLon) > boundM) continue
      for (const edgeIdx of graph.adjacency[u]) {
        const e = graph.edges[edgeIdx]
        if (isStampableRailEdge(e)) quarantine.add(e.key)
      }
    }
  }
}

/** AMBIGUOUS-PATH-UNION quarantine (2026-07-16, third iteration — REPLACES
 *  the graph-distance ellipse): an ambiguous pair's trains run on ONE of a
 *  small set of discrete corridors, each within
 *  `WALK_AMBIGUITY_LENGTH_RATIO` of the best path — that is the ambiguity
 *  criterion itself, so the honest evidence region is exactly the UNION of
 *  those candidate paths, not everything an admissible path could touch.
 *  WHY the ellipse had to go: for a long-haul leg the ellipse is a
 *  country-sized lens (DE: a 415 km Frankfurt->Berlin ICE leg, ambiguous
 *  via-Erfurt vs via-Hannover — bound ~1 040 km — blanketed most of the
 *  network on its own; 19 such >100 km legs kept DE quarantine pinned at
 *  ~94 % of stampable km through every other improvement). Candidates
 *  beyond best+alt are found by iterating the same penalized re-run the
 *  ambiguity probe uses (penalty accumulates over every already-used
 *  edge), until a probe exceeds the ratio or the iteration cap — each
 *  round either discovers a genuinely new corridor or a near-duplicate of
 *  a known one (both correctly belong to the union). */
const AMBIGUOUS_PATH_UNION_MAX_PROBES = 6
function quarantineAmbiguousPathUnion(
  graph: RailGraph,
  fromNode: number,
  toNode: number,
  best: ShortestPathResult,
  alt: ShortestPathResult,
  famFilter: ((e: RailGraphEdge) => boolean) | null,
  scratch: DijkstraScratch,
  quarantine: Set<string>,
): void {
  const usedEdges = new Set<number>()
  const addPath = (p: ShortestPathResult): void => {
    for (const idx of p.edgeIndices) {
      usedEdges.add(idx)
      const e = graph.edges[idx]
      if (isStampableRailEdge(e)) quarantine.add(e.key)
    }
  }
  addPath(best)
  addPath(alt)
  // BOUNDED CANDIDATE ENUMERATION, not a completeness proof (2026-07-16
  // Codex review, both passes): the probe optimizes PENALIZED length while
  // admission is PHYSICAL length, so a within-ratio route sharing many
  // already-used edges can hide behind a cheaper re-run of a known route.
  // The penalty therefore ESCALATES whenever a probe returns nothing new
  // (x3 -> x9 -> x27 ...), pushing the search off known corridors before
  // the probe budget runs out — this resolves the reviewed repro (third
  // corridor at 1.10x hidden behind best at penalized 3.00x) without a
  // full k-shortest-paths machine. A graph with more distinct within-ratio
  // corridors than the probe budget still leaves the excess unquarantined
  // — accepted: >6 near-equal disjoint corridors between one station pair
  // does not occur on real rail networks.
  let penalty = PATH_REUSE_PENALTY
  for (let probe = 0; probe < AMBIGUOUS_PATH_UNION_MAX_PROBES; probe++) {
    const next = dijkstraShortestPath(
      graph, fromNode, toNode, famFilter,
      (edgeIdx, e) => e.lengthM * (usedEdges.has(edgeIdx) ? penalty : 1),
      scratch,
    )
    if (!next) break
    if (next.lengthM <= WALK_AMBIGUITY_LENGTH_RATIO * best.lengthM) {
      let hadNew = false
      for (const idx of next.edgeIndices) if (!usedEdges.has(idx)) { hadNew = true; break }
      addPath(next)
      if (hadNew) continue
    }
    // Nothing new (or over-ratio) at this penalty — escalate and retry;
    // once even a heavily-penalized run finds no fresh within-ratio route,
    // the enumeration has converged.
    if (penalty > PATH_REUSE_PENALTY ** 3) break
    penalty *= PATH_REUSE_PENALTY
  }
}

/** Quarantines every stampable segment whose midpoint lies within
 *  `UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M` of a pair's straight chord.
 *  Standalone shape for an UNLOCALIZED pair (no snapped node to flood from
 *  at all — proximity to the raw chord is the next-tightest evidence
 *  available), and the corridor-band half of `quarantineGraphlessPair`. `geomByKey` is the SAME map
 *  the collection built once by `walkRailStationPairs`. */
function quarantineChordVicinity(
  geomByKey: Map<string, RailGraphEdge>,
  cp: { fromLat: number; fromLon: number; toLat: number; toLon: number },
  boundM: number,
  quarantine: Set<string>,
): void {
  // The band never exceeds the pair's own admissibility bound — a 500 m
  // leg (bound ~3.25 km) must not withhold tracks 5 km away (Codex review
  // W2: the fixed radius over-quarantined short pairs).
  const radiusM = Math.min(UNLOCALIZED_PAIR_QUARANTINE_RADIUS_M, boundM)
  // Chord bbox expanded by the radius — rejects the vast majority of a
  // country's segments on two comparisons before any distance math (this
  // scan runs per FAILED pair over every stampable geometry; DE-scale:
  // ~700k geoms x ~750 failed pairs).
  const radLatDeg = radiusM / M_PER_DEG_LAT
  const latLo = Math.min(cp.fromLat, cp.toLat) - radLatDeg
  const latHi = Math.max(cp.fromLat, cp.toLat) + radLatDeg
  const radLonDeg = radiusM /
    (M_PER_DEG_LON_EQ * Math.max(0.05, Math.cos(((cp.fromLat + cp.toLat) / 2) * Math.PI / 180)))
  const lonLo = Math.min(cp.fromLon, cp.toLon) - radLonDeg
  const lonHi = Math.max(cp.fromLon, cp.toLon) + radLonDeg
  for (const [key, g] of geomByKey) {
    if (!isStampableRailEdge(g)) continue
    const midLat = (g.startLat + g.endLat) / 2
    if (midLat < latLo || midLat > latHi) continue
    const midLon = (g.startLon + g.endLon) / 2
    if (midLon < lonLo || midLon > lonHi) continue
    if (pointToSegmentDist(midLat, midLon, cp.fromLat, cp.fromLon, cp.toLat, cp.toLon) <= radiusM) {
      quarantine.add(key)
    }
  }
}

function shapeEdgeFilter(shapePolyline: Array<[number, number]>): (edge: RailGraphEdge) => boolean {
  const lonLatCoords: Array<[number, number]> = shapePolyline.map(([lat, lon]) => [lon, lat])
  return (edge: RailGraphEdge) => {
    const midLat = (edge.startLat + edge.endLat) / 2
    const midLon = (edge.startLon + edge.endLon) / 2
    return pointToPolylineDist(midLat, midLon, lonLatCoords) <= SHAPE_CORRIDOR_TOLERANCE_M
  }
}

export function walkRailStationPairs(graph: RailGraph, pairs: RailStationPairCount[]): RailWalkResult {
  const stampsBySegmentKey = new Map<string, { pax: number; frt: number }>()
  const failures = { snapFailed: 0, disconnected: 0, detourRejected: 0, ambiguous: 0 }
  const failedPairChords: RailWalkResult['failedPairChords'] = []
  const quarantinedSegmentKeys = new Set<string>()
  // Neither end snapped — stats-only telemetry now (see RailWalkResult doc);
  // the pair's own quarantine is handled via quarantineUnlocalizedPairChord
  // below instead of a global suppression flag.
  let unlocalizedPairs = 0

  const searches = new RailPairSearches()
  for (const pair of pairs) searches.add(pair)
  const pairsTotal = searches.size
  let pairsWalked = 0

  const geomByKey = collectSegmentGeometry(graph)

  // ONE scratch for every search over this graph (see DijkstraScratch doc) —
  // a country/world union graph runs up to two searches (best path + the
  // ambiguity probe's penalized re-run) per distinct search, plus now up to
  // two bounded quarantine floods per FAILED pair, and re-allocating
  // nodeCount-sized arrays that many times is the real scale blocker.
  const scratch = createDijkstraScratch(graph.nodeCount)

  for (const cp of searches.values()) {
    // DE Step A v2 diagnostics (2026-07-16 failure analysis, fix 3):
    // `diagnostics` carries the reason-specific fields (`RailFailedPairRecord`'s
    // doc) — `ambiguousGeometry` for 'ambiguous', `snapDistanceM` for
    // 'snapFailed' — computed by the caller ONLY on the failure path that
    // needs it, never here unconditionally.
    const fail = (
      reason: RailFailedPairRecord['reason'],
      diagnostics?: Pick<RailFailedPairRecord, 'ambiguousGeometry' | 'snapDistanceM' | 'detourGeometry'>,
    ) => {
      failures[reason]++
      failedPairChords.push({
        fromLat: cp.fromLat, fromLon: cp.fromLon, toLat: cp.toLat, toLon: cp.toLon, reason,
        ...diagnostics,
      })
    }

    // This pair's OWN detour bound — identical formula to the
    // `detourRejected` gate below, computed unconditionally up front because
    // the quarantine shapes need it on EVERY failure path, not only that one
    // (2026-07-16 Step-B refinement, plan item 2).
    const greatCircleM = haversineM(cp.fromLat, cp.fromLon, cp.toLat, cp.toLon)
    const bound = WALK_DETOUR_RATIO * greatCircleM + WALK_DETOUR_SLACK_M
    // Per-failure-reason quarantine shapes (item 4 + review round + the
    // 2026-07-16 DE redesign, both iterations):
    // snapFailed/disconnected/detourRejected take chord band + capped graph
    // fingers (their trains exist, the GRAPH failed to place them — both
    // coordinates are always known); AMBIGUOUS takes the union of its own
    // candidate paths (admissible corridors exist, the walk just cannot
    // pick one). See each function's doc.
    const quarantineGraphless = (fromNodeId: number, toNodeId: number): void => {
      quarantineGraphlessPair(graph, geomByKey, cp, fromNodeId, toNodeId, bound, scratch, quarantinedSegmentKeys)
    }


    // FAMILY-LOCKED two-attempt walk (2026-07-16 Codex review C1): standard
    // and narrow tracks can share an OSM node (joint stations, dual-gauge
    // throats), and a family-blind Dijkstra would route a standard-gauge
    // pair over a shorter narrow shortcut — a physically impossible gauge
    // switch stamping the wrong track. Each endpoint snaps PER FAMILY;
    // attempts run in order of total snap distance (a station's GPS sits on
    // its own family's platform — snap distance is the gauge evidence GTFS
    // lacks), and every search of an attempt (best path, ambiguity probe,
    // path-union widening) is filtered to that ONE family + crossovers.
    type FamilySnap = NonNullable<ReturnType<typeof snapToNearestFamilyNode>>
    const familyAttempts = WALK_FAMILY_MASKS
      .map((mask) => ({
        mask,
        from: snapToNearestFamilyNode(graph, cp.fromLat, cp.fromLon, mask),
        to: snapToNearestFamilyNode(graph, cp.toLat, cp.toLon, mask),
      }))
      .filter((a): a is { mask: (typeof WALK_FAMILY_MASKS)[number]; from: FamilySnap; to: FamilySnap } =>
        a.from !== null && a.to !== null)
      .sort((a, b) => (a.from.distM + a.to.distM) - (b.from.distM + b.to.distM))

    if (familyAttempts.length === 0) {
      // No single family snaps BOTH ends (covers true snap failures and the
      // rare cross-family pair — a leg cannot change gauge mid-run, so both
      // classify the same way). Quarantine foci: the family-blind nearest
      // snaps, preserving the pre-family behavior for evidence shapes.
      const fromNode = snapToNearestRailGraphNode(graph, cp.fromLat, cp.fromLon)
      const toNode = snapToNearestRailGraphNode(graph, cp.toLat, cp.toLon)
      quarantineGraphless(fromNode, toNode)
      if (fromNode === -1 && toNode === -1) {
        unlocalizedPairs++ // quarantine: quarantineGraphlessPair above already laid the chord band; with no snapped node there are no fingers
      }
      // DE Step A v2 diagnostics (fix 3): the TRUE distance to the nearest
      // graph node for whichever end(s) failed to snap — `null` for an end
      // that DID snap; `'unreachable'` when nothing lies within the search
      // ceiling, NEVER `Infinity` (JSON-persisted; Infinity serializes to
      // null — the "snapped fine" sentinel; Codex review item 3).
      const snapDistanceForUnsnappedEnd = (lat: number, lon: number): number | 'unreachable' => {
        const d = nearestRailGraphNodeDistanceM(graph, lat, lon)
        return Number.isFinite(d) ? d : 'unreachable'
      }
      fail('snapFailed', {
        snapDistanceM: {
          from: fromNode === -1 ? snapDistanceForUnsnappedEnd(cp.fromLat, cp.fromLon) : null,
          to: toNode === -1 ? snapDistanceForUnsnappedEnd(cp.toLat, cp.toLon) : null,
        },
      })
      continue
    }

    // Try each snappable family; the FIRST attempt's failure is what the
    // pair reports if none walks (it is the family the stations most
    // plausibly belong to). An AMBIGUOUS verdict is final for the pair —
    // ambiguity is a genuine within-family property, not a reason to try
    // the other gauge.
    let walked: { mask: number; fromNode: number; toNode: number; best: ShortestPathResult; famFilter: (e: RailGraphEdge) => boolean } | null = null
    let primaryFailure: (() => void) | null = null
    for (const att of familyAttempts) {
      const fromNode = att.from.nodeId
      const toNode = att.to.nodeId
      const famFilter = (e: RailGraphEdge): boolean => e.isTraversalOnly || walkFamilyBit(e.railType) === att.mask

      if (graph.componentOfNode[fromNode] !== graph.componentOfNode[toNode]) {
        primaryFailure ??= () => {
          quarantineGraphless(fromNode, toNode)
          fail('disconnected')
        }
        continue
      }

      const shapeFilter = cp.shapePolyline ? shapeEdgeFilter(cp.shapePolyline) : null
      const bestFilter = shapeFilter ? (e: RailGraphEdge) => famFilter(e) && shapeFilter(e) : famFilter
      const best = dijkstraShortestPath(graph, fromNode, toNode, bestFilter, (_i, e) => e.lengthM, scratch)
      if (!best) {
        // Same topological component yet no route through this family's
        // routable edges alone — folded into 'disconnected' (same class of
        // problem at a finer grain).
        primaryFailure ??= () => {
          quarantineGraphless(fromNode, toNode)
          fail('disconnected')
        }
        continue
      }

      if (best.lengthM > bound) {
        primaryFailure ??= () => {
          quarantineGraphless(fromNode, toNode)
          // Diagnostics for bound tuning (CH Alpine meanders: rack/spiral
          // lines legitimately exceed 2.5x on short chords).
          fail('detourRejected', { detourGeometry: { bestPathM: best.lengthM, boundM: bound } })
        }
        continue
      }

      walked = { mask: att.mask, fromNode, toNode, best, famFilter }
      break
    }

    if (!walked) {
      primaryFailure!()
      continue
    }
    const { fromNode, toNode, best, famFilter } = walked

    // A real shape already resolves the corridor; otherwise test alternatives.
    if (!cp.shapePolyline) {
      const alternative = findAmbiguousRailAlternative(graph, fromNode, toNode, best, famFilter, scratch)
      if (alternative) {
        const { path: alt, twinGate } = alternative
        quarantineAmbiguousPathUnion(graph, fromNode, toNode, best, alt, famFilter, scratch, quarantinedSegmentKeys)
        fail('ambiguous', { ambiguousGeometry: summarizeAmbiguousGeometry(graph, best, alt, twinGate) })
        continue
      }
    }

    pairsWalked++
    for (const edgeIdx of best.edgeIndices) {
      const e = graph.edges[edgeIdx]
      if (!isStampableRailEdge(e)) continue // crossovers/other families connect, never stamped
      const existing = stampsBySegmentKey.get(e.key) ?? { pax: 0, frt: 0 }
      existing.pax += cp.pax
      existing.frt += cp.frt
      stampsBySegmentKey.set(e.key, existing)
    }
  }

  return {
    stampsBySegmentKey, failures, failedPairChords,
    quarantinedSegmentKeys, unlocalizedPairs, pairsWalked, pairsTotal,
  }
}
