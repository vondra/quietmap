/** Map fan of one road/railway microsegment: per-slice cause + triangles.
 *
 * The popup profile shows a single ray (receiver → closest point) while the
 * dB number averages a whole angular fan. This module turns the engine's
 * ScreeningFanTrace into map geometry so the fan itself becomes visible.
 * Pure TypeScript — no React, unit-tested in frontend/test/.
 */
import type { SegmentTrace } from '../../types/noise'

/** What blocks one fan slice. Vegetation is NOT here: the engine resolves
 * forest once on the characteristic ray, so per-slice forest state does not
 * exist — coloring it would invent data. */
export type FanSliceKind = 'clear' | 'building' | 'terrain' | 'mixed'

/** A slice counts as terrain-shadowed at this 1 kHz A_terrain. Below it the
 * Maekawa term is ripple, not shadow — keep it green. */
export const FAN_TERRAIN_DB_THRESHOLD = 1.0

export function classifyFanSlice(slice: { blocked: boolean; terrain_db: number }): FanSliceKind {
  const hill = slice.terrain_db >= FAN_TERRAIN_DB_THRESHOLD
  if (slice.blocked && hill) return 'mixed'
  if (slice.blocked) return 'building'
  if (hill) return 'terrain'
  return 'clear'
}

/** Slice colors, one truth for the deck.gl fills and the popup legend chips.
 * Solid translucent fills — readable on light and satellite basemaps. The
 * CSS chips derive from the same RGB with a uniform legibility alpha (tiny
 * dots need more opacity than map fills), so hues can't drift apart. */
export const FAN_SLICE_RGBA: Record<FanSliceKind, [number, number, number, number]> = {
  clear: [34, 197, 94, 64],
  building: [220, 38, 38, 84],
  terrain: [217, 119, 6, 90],
  mixed: [127, 29, 29, 120],
}

const cssOf = (c: readonly [number, number, number, number]): string =>
  `rgba(${c[0]}, ${c[1]}, ${c[2]}, 0.65)`

export const FAN_SLICE_CSS: Record<FanSliceKind, string> = {
  clear: cssOf(FAN_SLICE_RGBA.clear),
  building: cssOf(FAN_SLICE_RGBA.building),
  terrain: cssOf(FAN_SLICE_RGBA.terrain),
  mixed: cssOf(FAN_SLICE_RGBA.mixed),
}

export const FAN_SLICE_LABEL: Record<FanSliceKind, string> = {
  clear: 'open',
  building: 'building / barrier',
  terrain: 'terrain',
  mixed: 'building + terrain',
}

// Engine mirror (noise-compute constants.rs): equirectangular meters per
// degree. Display-only triangulation — exact constants keep slices aligned
// with the engine's own ray∩segment solve.
const M_PER_DEG_LAT = 110_540.0
const M_PER_DEG_LON_EQ = 111_320.0

const DEG = Math.PI / 180

/** Receiver-centred ray∩segment solve, mirroring SegFan::at (seg_sampling.rs):
 * the point on the segment seen at absolute azimuth `az`. Null when the ray
 * is parallel to the segment (meets it at infinity — never invent a point),
 * misses the segment line, or lands within a metre of the receiver. */
function pointOnSegmentAt(
  ax: number,
  ay: number,
  ex: number,
  ey: number,
  toLonLat: (x: number, y: number) => [number, number],
  az: number,
): [number, number] | null {
  const ux = Math.cos(az)
  const uy = Math.sin(az)
  const crE = ux * ey - uy * ex
  const crA = ux * ay - uy * ax
  if (Math.abs(crE) <= 1e-12) return null
  const tc = Math.max(0, Math.min(1, -crA / crE))
  const sx = ax + tc * ex
  const sy = ay + tc * ey
  if (!Number.isFinite(sx) || !Number.isFinite(sy)) return null
  if (Math.hypot(sx, sy) < 1.0) return null
  return toLonLat(sx, sy)
}

/** Fan highlight for an expanded segment row: one triangle per engine
 * interval (colored by cause), the characteristic ray, and the segment
 * itself — as a single FeatureCollection for the map highlight layers.
 * Null when there is no fan to draw (point sources, Doc 29 aircraft,
 * scalar-only ground-ops traces, degenerate geometry). */
export function segmentFanHighlight(trace: SegmentTrace): GeoJSON.FeatureCollection | null {
  if (trace.propagation.model !== 'cnossos') return null
  // Scalar-only traces (aircraft ground-ops) carry no screening object.
  const fan = trace.propagation.screening?.fan
  if (!fan || fan.intervals.length === 0) return null
  const profile = trace.propagation.path_profile
  const { rcv_lat, rcv_lon } = profile
  if (!Number.isFinite(rcv_lat) || !Number.isFinite(rcv_lon)) return null
  if ((rcv_lat === 0 && rcv_lon === 0) || trace.dist_m <= 0) return null
  for (const v of [trace.start_lat, trace.start_lon, trace.end_lat, trace.end_lon, trace.cp_lat, trace.cp_lon]) {
    if (!Number.isFinite(v)) return null
  }

  const mLon = M_PER_DEG_LON_EQ * Math.max(0.01, Math.cos(rcv_lat * DEG))
  const toXY = (lat: number, lon: number): [number, number] => [
    (lon - rcv_lon) * mLon,
    (lat - rcv_lat) * M_PER_DEG_LAT,
  ]
  const toLonLat = (x: number, y: number): [number, number] => [
    rcv_lon + x / mLon,
    rcv_lat + y / M_PER_DEG_LAT,
  ]
  const [ax, ay] = toXY(trace.start_lat, trace.start_lon)
  const [bx, by] = toXY(trace.end_lat, trace.end_lon)
  const ex = bx - ax
  const ey = by - ay
  if (ex * ex + ey * ey < 1e-6) return null
  const [cx, cy] = toXY(trace.cp_lat, trace.cp_lon)
  if (Math.hypot(cx, cy) < 1.0) return null
  const cpAz = Math.atan2(cy, cx)

  // Interval degrees are offsets from the characteristic-ray azimuth —
  // the engine stores them that way (arc_screening.rs FanTrace::push:
  // `from_deg: (start - cp_azimuth).to_degrees()`), so each boundary maps
  // to an absolute azimuth by adding cpAz.
  const features: GeoJSON.Feature[] = []
  const ordered = [...fan.intervals].sort((a, b) => a.from_deg - b.from_deg)
  for (const interval of ordered) {
    if (!(interval.to_deg > interval.from_deg)) continue
    const p0 = pointOnSegmentAt(ax, ay, ex, ey, toLonLat, cpAz + interval.from_deg * DEG)
    const p1 = pointOnSegmentAt(ax, ay, ex, ey, toLonLat, cpAz + interval.to_deg * DEG)
    if (!p0 || !p1) continue
    features.push({
      type: 'Feature',
      properties: { fanKind: classifyFanSlice(interval) },
      geometry: { type: 'Polygon', coordinates: [[[rcv_lon, rcv_lat], p0, p1, [rcv_lon, rcv_lat]]] },
    })
  }
  if (features.length === 0) return null

  features.push({
    type: 'Feature',
    properties: { lineKind: 'cp' },
    geometry: { type: 'LineString', coordinates: [[rcv_lon, rcv_lat], [trace.cp_lon, trace.cp_lat]] },
  })
  features.push({
    type: 'Feature',
    properties: { lineKind: 'segment' },
    geometry: {
      type: 'LineString',
      coordinates: [
        [trace.start_lon, trace.start_lat],
        [trace.end_lon, trace.end_lat],
      ],
    },
  })
  return { type: 'FeatureCollection', features }
}
