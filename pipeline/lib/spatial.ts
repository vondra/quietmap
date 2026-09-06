/**
 * Shared spatial helpers used by enrichment pipelines and bench tools.
 *
 * Most enrichers historically declared their own copies of `flatDist`,
 * `pointToSegmentDist`, `inBbox`, etc. — same formula, slight signature
 * drift across files (e.g. `pointInRing(lon, lat, ring)` vs
 * `pointInRing(lat, lon, ring)`). This module is the canonical source.
 *
 * Conventions:
 *   - Distances in metres.
 *   - Lat/lon order: lat first, EXCEPT for `pointInRing` which keeps the
 *     GeoJSON-native (lon, lat) order for parity with `scripts/build-h3-admin.ts`.
 *   - Bounding boxes: `[minLat, minLon, maxLat, maxLon]`.
 *   - Distance helpers use the flat-earth approximation (cosLat at the
 *     midpoint) — matches `engine/noise-compute/src/propagation/geo.rs::flat_dist`,
 *     accurate to <0.3 % at <50 km. Use `haversineM` if you specifically
 *     need spheroidal accuracy beyond that range.
 *   - Antimeridian: `flatDist`/`pointToSegmentDist`/`pointToSegmentParamT` wrap
 *     the longitude delta to [-180, 180] before projecting (mirrors
 *     `engine/osm-extract/src/microsegment.rs::flat_dist`'s own wrap) — a point
 *     trio straddling ±180° must read as physically close, not ~half the
 *     planet away. Grid/bbox indexes built on RAW lat/lon (`nodeKey`,
 *     `coordKey4dp`, and every cell-keyed grid in rail-graph.ts/
 *     rail-graph-metrics.ts) do NOT get this treatment — no railway segment in
 *     the dataset crosses ±180°; revisit if one ever does.
 */

/** Metres per degree of latitude / longitude-at-the-equator, flat-earth
 *  approximation (see module doc). Exported so every caller that needs the
 *  raw projection constants (rather than a ready-made distance/param helper)
 *  shares the SAME numbers instead of re-declaring a local copy that could
 *  drift — e.g. rail-graph.ts's bearing/heading math, which needs a
 *  t-parameter and a compass heading this module's own helpers don't return. */
export const M_PER_DEG_LAT = 110_540
export const M_PER_DEG_LON_EQ = 111_320

/**
 * Endpoint identity for road-segment topology: quantize to ~1 m so segments
 * that share a physical node compare equal. THE shared scheme for every pass
 * that chains segments by endpoints (service-tree, continuity-fill, R7 taper)
 * — the passes must agree on node identity or their topologies silently
 * diverge.
 *
 * `toFixed(5)` is a GRID SNAP, not a proximity rule: it buckets into ~1.1 m x
 * 0.7 m cells, so two endpoints merge when they land in the same cell and stay
 * apart when they straddle a boundary however close they are. That is
 * deliberate (91f034424) and load-bearing — on CZ rail it is the only link for
 * 34 crossovers, which exact-coordinate identity would sever. Measured on CZ
 * rail: it merges 129 of 366,919 coordinates, and of the 506 near pairs inside
 * one cell diagonal it merges 132 and splits 374.
 */
export function nodeKey(lat: number, lon: number): string {
  return `${lat.toFixed(5)}_${lon.toFixed(5)}`
}

/**
 * Station/stop identity: quantize to 4dp (~11 m) so two records at the same
 * physical location (duplicate platform rows, a GTFS child stop resolved to
 * its parent, a rail-graph node reached from two directions) compare equal.
 * The 4dp sibling of `nodeKey`'s 5dp — coarser on purpose: a station's GPS
 * varies more between sources than a road/rail segment's own extracted
 * vertex does. Used by the graph-walk's canonical pair keys, the rail-stops
 * sidecar's stop dedup, and GTFS station identity (`gtfs-stop-pairs.ts`'s
 * `stationKey`) — these call sites MUST stay byte-identical, or the same
 * physical station could hash to different keys across files.
 */
export function coordKey4dp(lat: number, lon: number): string {
  return `${lat.toFixed(4)},${lon.toFixed(4)}`
}

/**
 * Wraps a longitude DELTA (degrees, `lon2 - lon1`) to [-180, 180] — the short
 * way around the globe. Mirrors `engine/osm-extract/src/microsegment.rs::
 * flat_dist`'s own antimeridian handling: without it, a point/segment trio
 * that straddles ±180° (e.g. 179.9° -> -179.9°, physically ~22 km apart)
 * projects to a ~359.8° delta and reports ~40,000 km instead. Exported so
 * every DISTANCE helper below shares one wrap instead of each re-deriving it
 * (and so rail-graph-metrics.ts's `headingDeg`, which duplicates the same
 * lon-delta-then-project pattern for a compass bearing rather than a
 * distance, can reuse it too).
 */
export function wrapLonDeltaDeg(deltaDeg: number): number {
  if (deltaDeg > 180) return deltaDeg - 360
  if (deltaDeg < -180) return deltaDeg + 360
  return deltaDeg
}

/**
 * Flat-earth distance in metres between two (lat, lon) points. Same
 * algorithm as the engine's `flat_dist` — accurate to <0.3 % at <50 km.
 * Hot-loop callers that have a hex-level cosLat should pre-project to
 * x/y instead of paying a `Math.cos` per call (see
 * `pipeline/enrich-roads-service-tree.ts::pointToSegmentDistXY`).
 */
export function flatDist(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const cosLat = Math.cos(((lat1 + lat2) / 2) * Math.PI / 180)
  const dx = wrapLonDeltaDeg(lon2 - lon1) * M_PER_DEG_LON_EQ * cosLat
  const dy = (lat2 - lat1) * M_PER_DEG_LAT
  return Math.sqrt(dx * dx + dy * dy)
}

/** Alias so callers that grew up calling it `flatDistM` keep working
 *  without a local copy. */
export const flatDistM = flatDist

/**
 * Spheroidal great-circle distance in metres. ~10× more expensive than
 * `flatDist`; only worth it for distances >50 km where flat-earth drifts
 * past 0.3 %. The 19 enrichers that previously declared `haversineM`
 * locally were all using it for sub-1-km buffer checks where flat-earth
 * is fine — they can switch to `flatDist` if they want the speed.
 */
export function haversineM(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const R = 6_371_008.8
  const φ1 = lat1 * Math.PI / 180
  const φ2 = lat2 * Math.PI / 180
  const Δφ = (lat2 - lat1) * Math.PI / 180
  const Δλ = (lon2 - lon1) * Math.PI / 180
  const a = Math.sin(Δφ / 2) ** 2 + Math.cos(φ1) * Math.cos(φ2) * Math.sin(Δλ / 2) ** 2
  return 2 * R * Math.asin(Math.sqrt(a))
}

/**
 * Distance from point `p` to segment `a → b`, all in (lat, lon) degrees.
 * Single-shot helper — for hot loops that compute many distances against
 * the same segment, pre-project once and use a pure-arithmetic kernel
 * (see service-tree's `pointToSegmentDistXY`).
 *
 * Projects `p` and `b` as longitude DELTAS relative to `a` (each wrapped via
 * `wrapLonDeltaDeg`) rather than each point's absolute `lon * constant` — the
 * pre-2026-07-16 version used absolute projection, which blows up for a
 * segment/point trio straddling ±180° even though only the differences ever
 * feed the math (mirrors `microsegment.rs::perp_distance_to_chord`).
 */
export function pointToSegmentDist(
  pLat: number, pLon: number,
  aLat: number, aLon: number,
  bLat: number, bLon: number,
): number {
  const cosLat = Math.cos(pLat * Math.PI / 180)
  const px = wrapLonDeltaDeg(pLon - aLon) * M_PER_DEG_LON_EQ * cosLat
  const py = (pLat - aLat) * M_PER_DEG_LAT
  const bx = wrapLonDeltaDeg(bLon - aLon) * M_PER_DEG_LON_EQ * cosLat
  const by = (bLat - aLat) * M_PER_DEG_LAT
  const lenSq = bx * bx + by * by
  if (lenSq < 1e-6) return flatDist(pLat, pLon, aLat, aLon)
  let t = (px * bx + py * by) / lenSq
  t = Math.max(0, Math.min(1, t))
  const cx = t * bx
  const cy = t * by
  return Math.sqrt((px - cx) * (px - cx) + (py - cy) * (py - cy))
}

/**
 * Same flat-earth projection as `pointToSegmentDist`, but returns the
 * clamped parameter `t` along `a -> b` (0 at `a`, 1 at `b`) instead of the
 * distance. Kept as a SEPARATE function rather than having
 * `pointToSegmentDist` return both: the overwhelming majority of call sites
 * only want the distance, and T-junction healing (rail-graph.ts) is the only
 * caller that needs the projected LOCATION on the segment rather than merely
 * how close a point sits to it. Same delta-relative-to-`a` antimeridian wrap
 * as `pointToSegmentDist` (see its doc) — the two must stay in lockstep or a
 * ±180°-straddling segment would heal at a different point than it measures.
 */
export function pointToSegmentParamT(
  pLat: number, pLon: number,
  aLat: number, aLon: number,
  bLat: number, bLon: number,
): number {
  const cosLat = Math.cos(pLat * Math.PI / 180)
  const px = wrapLonDeltaDeg(pLon - aLon) * M_PER_DEG_LON_EQ * cosLat
  const py = (pLat - aLat) * M_PER_DEG_LAT
  const bx = wrapLonDeltaDeg(bLon - aLon) * M_PER_DEG_LON_EQ * cosLat
  const by = (bLat - aLat) * M_PER_DEG_LAT
  const lenSq = bx * bx + by * by
  if (lenSq < 1e-6) return 0
  const t = (px * bx + py * by) / lenSq
  return Math.max(0, Math.min(1, t))
}

/**
 * Min distance (m) from point `(pLat, pLon)` to a polyline given as
 * `[lon, lat]` vertices (GeoJSON-native order). Returns `Infinity` for an
 * empty line, the point-to-vertex distance for a single vertex.
 *
 * Matching a road segment to the nearest point ON a census section's line —
 * instead of to the section's centroid — places the boundary between two
 * adjacent sections at the real junction rather than on the perpendicular
 * bisector of their centroids (which drifts hundreds of metres when the
 * sections differ in length). 14 country enrichers carried a private copy of
 * this; this is the canonical one.
 */
export function pointToPolylineDist(
  pLat: number,
  pLon: number,
  coords: ReadonlyArray<readonly [number, number]>,
): number {
  if (coords.length === 0) return Infinity
  if (coords.length === 1) return flatDist(pLat, pLon, coords[0][1], coords[0][0])
  let best = Infinity
  for (let i = 0; i < coords.length - 1; i++) {
    const d = pointToSegmentDist(pLat, pLon, coords[i][1], coords[i][0], coords[i + 1][1], coords[i + 1][0])
    if (d < best) best = d
  }
  return best
}

/**
 * Inclusive bounding-box test. `bbox = [minLat, minLon, maxLat, maxLon]`.
 * Most enrichers used this exact convention with a 4-tuple; a handful
 * passed a wider `number[]` — the wider type is implicit-compatible.
 */
export function inBbox(
  lat: number,
  lon: number,
  bbox: readonly [number, number, number, number],
): boolean {
  return lat >= bbox[0] && lat <= bbox[2] && lon >= bbox[1] && lon <= bbox[3]
}

/**
 * Ray-cast point-in-polygon test. **Lon-first arg order**, matching
 * GeoJSON's native (lon, lat) ring ordering and the
 * `scripts/build-h3-admin.ts` convention. The 3-or-so enrichers that
 * declared a `pointInRing(lat, lon, ring)` variant had their args
 * swapped relative to GeoJSON; migrate carefully if porting them over.
 */
export function pointInRing(lon: number, lat: number, ring: ReadonlyArray<readonly [number, number]>): boolean {
  let inside = false
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const xi = ring[i][0], yi = ring[i][1]
    const xj = ring[j][0], yj = ring[j][1]
    if (((yi > lat) !== (yj > lat)) && (lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi)) {
      inside = !inside
    }
  }
  return inside
}
