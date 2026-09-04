import { memo, useState } from 'react'
import { AIRCRAFT_SUBTYPE, type SegmentTrace } from '../../types/noise'
import { HoverText } from '../ui/info-tip'
import { ldenToColor } from '../../utils/noise-colors'
import { SegmentExpanded } from './SegmentExpanded'
import { segmentFanHighlight } from './fanGeometry'
import { SOURCE_LABELS, flipLatLon, formatDist, lineStringFromLatLon, subtypeLabel } from './shared'

function segmentName(t: SegmentTrace): string {
  if (t.name && t.name.length > 0) return t.name
  return SOURCE_LABELS[t.kind] ?? t.kind
}

function highlightGeometry(t: SegmentTrace) {
  if (t.kind === 'building' || t.kind === 'industrial') {
    return { type: 'Point', coordinates: [t.start_lon, t.start_lat] }
  }
  // Road/railway segments with an engine screening fan: draw the fan itself
  // (per-slice triangles + characteristic ray) instead of the bare segment —
  // the dB number averages the fan, so the map should show the fan.
  if (t.kind === 'road' || t.kind === 'railway') {
    const fan = segmentFanHighlight(t)
    if (fan) return fan
  }
  // Aircraft sub-types carry their own geometry shape:
  //   GROUND = polyline → MultiLineString
  //   AIRBORNE = start/end → LineString (fall-through)
  //   CRUISE = hex_polygon → Polygon (ring already closed by backend)
  if (t.kind === 'aircraft') {
    if (t.aircraft_subtype === AIRCRAFT_SUBTYPE.GROUND && t.polyline && t.polyline.length >= 2) {
      // One continuous ADS-B trajectory → single LineString. (If the
      // polyline ever encodes disjoint runs we'd switch to
      // MultiLineString; backend currently emits one trajectory per
      // contiguous ground run, so LineString matches the source.)
      return { type: 'LineString', coordinates: t.polyline.map(flipLatLon) }
    }
    if (t.aircraft_subtype === AIRCRAFT_SUBTYPE.CRUISE && t.hex_polygon && t.hex_polygon.length >= 4) {
      return { type: 'Polygon', coordinates: [t.hex_polygon.map(flipLatLon)] }
    }
  }
  return lineStringFromLatLon([t.start_lat, t.start_lon], [t.end_lat, t.end_lon])
}

const POWER_SUM_HINT =
  'Per-segment received Lden shown is the segment-alone level.\n' +
  'Grouped "Noise source" Lden pools segments in energy, not dB:\n' +
  '  L_total = 10·log₁₀(Σᵢ 10^(Lᵢ/10))'

// Aircraft NPD uses 3D slant. For airborne, both `dist_m` and
// `d_slant_m` are currently sourced from CPA's `d_p_m`, so reading
// `d_slant_m` is equivalent today but stays correct if the airborne
// emit ever changes to populate horizontal in `dist_m`. For cruise,
// `dist_m` is hardcoded to 0 (receiver sits inside the R8 hex), so
// slant is the only meaningful distance there.
function displayDistance(t: SegmentTrace): number {
  if (
    t.kind === 'aircraft' &&
    (t.aircraft_subtype === AIRCRAFT_SUBTYPE.AIRBORNE ||
      t.aircraft_subtype === AIRCRAFT_SUBTYPE.CRUISE)
  ) {
    return t.d_slant_m
  }
  return t.dist_m
}

export const SegmentRow = memo(SegmentRowImpl)

function SegmentRowImpl({
  trace,
  onHighlight,
}: {
  trace: SegmentTrace
  onHighlight?: (geometry: unknown | null) => void
}) {
  const [expanded, setExpanded] = useState(false)
  const lden = trace.received_lden.full
  const subtype = subtypeLabel(trace.kind, trace.subtype)
  const headingName = segmentName(trace)
  const showSubtype = !headingName.toLowerCase().startsWith(String(trace.subtype).toLowerCase())

  const handleToggle = () => {
    const next = !expanded
    setExpanded(next)
    onHighlight?.(next ? highlightGeometry(trace) : null)
  }

  return (
    <div className="border-b border-border/30 last:border-b-0">
      <button
        onClick={handleToggle}
        className="w-full py-1 text-left hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <div className="flex items-baseline gap-1.5 text-xs px-0">
          <span className="truncate flex-1">
            <span className="font-medium text-foreground">{headingName}</span>
            {trace.is_dominant_of_group && (
              <span
                className="text-[10px] text-amber-500 ml-0.5"
                title="Dominant segment of its Noise source group"
              >
                ⭑
              </span>
            )}
            {showSubtype && <span className="text-muted-foreground/60"> · {subtype}</span>}
          </span>
          <span className="text-muted-foreground/60 shrink-0 w-14 text-right tabular-nums">
            {formatDist(Math.round(displayDistance(trace)))}
          </span>
          <HoverText title={POWER_SUM_HINT}>
            <span
              className="font-medium shrink-0 w-14 text-right tabular-nums inline-block"
              style={{ color: ldenToColor(lden) }}
            >
              {lden.toFixed(1)}{'\u00A0'}dB
            </span>
          </HoverText>
          <span className="text-[10px] text-muted-foreground/40 shrink-0">
            {expanded ? '▲' : '▼'}
          </span>
        </div>
      </button>
      {expanded && <SegmentExpanded trace={trace} />}
    </div>
  )
}
