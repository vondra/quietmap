import { useMemo, useRef, useState } from 'react'
import type { EdgePoint, ObstacleEdge, PathProfileTrace } from '../../types/noise'
import { HoverText } from '../ui/info-tip'
import { DIAGRAM_COLORS, formatDist } from './shared'

const VB_W = 600
const VB_H = 220
const PAD_L = 42
const PAD_R = 12
const PAD_T = 16
const PAD_B = 28
const PLOT_W = VB_W - PAD_L - PAD_R
const PLOT_H = VB_H - PAD_T - PAD_B

const MIN_BUILDING_PX = 4
// An exact crossing is a point, not a cell: draw it a fixed narrow bar so a
// 6 m shed and a 60 m block differ in HEIGHT, which is what screens sound.
// Wide enough to read as a building at popup scale — the earlier 5-unit bar
// looked like just another sample tick next to the ridge rings.
const MIN_BUILDING_W = 9

// Forest shades used only here (canopy + highlight). The shared
// DIAGRAM_COLORS.forest is a mid-green; we also need a lighter shade
// for the center tuft so it reads as layered vegetation.
const FOREST_CANOPY = '#2d6b2d'
const FOREST_HIGHLIGHT = '#3a843a'

function niceTickStep(range: number): number {
  // ×1 / ×0.5 / ×0.2 produced up to 10 ticks on paths in the 1–2 × 10^k
  // range (e.g. 18 m → 0, 2, 4 … 18) — too crowded for narrow popups.
  if (range <= 0) return 1
  const pow = Math.pow(10, Math.floor(Math.log10(range)))
  const norm = range / pow
  if (norm >= 5) return pow * 2
  if (norm >= 2) return pow * 1
  return pow * 0.5
}

// Qualitative ground-hardness bands. The ridge polyline is split into
// runs of consecutive segments sharing a bucket, each rendered with
// the bucket's dasharray — so the terrain line itself encodes ground
// type: dotted = soft, dashed = mixed, solid = hard.
function imdBucket(imd: number): { label: string; dash: string | undefined } {
  if (imd <= 33) return { label: 'soft', dash: '1 5' }
  if (imd <= 66) return { label: 'mixed', dash: '5 4' }
  return { label: 'hard', dash: undefined }
}

function imdLabel(imd: number): string {
  return imdBucket(imd).label
}

export interface PathProfileDiagramProps {
  trace: PathProfileTrace
  /** Engine-detected diffraction edge (single-edge model: at most the one
   * max-δ dominant edge). When undefined / empty no apex marker is drawn. */
  terrainEdges?: EdgePoint[]
  /** The exact vector crossing that beat the terrain edge. */
  obstacleEdge?: ObstacleEdge
}

export function PathProfileDiagram({
  trace,
  terrainEdges,
  obstacleEdge,
}: PathProfileDiagramProps) {
  const n = trace.t.length
  const dist = Math.max(trace.dist_m, 1)
  const hasPath = n > 0 && trace.dist_m > 0

  const { elevMin, elevMax, xOf, yOf, xAxisTicks } =
    useMemo(() => {
      const elevs = trace.elevation_m
      // Fold, don't spread: elevation_m can be 10k+ samples at airport points,
      // and Math.max(...bigArray) overflows the call stack.
      const maxBld = Number(obstacleEdge?.height_m) || 0
      let rawMin = trace.src_alt_m - maxBld
      let rawMax = Math.max(trace.src_alt_m, trace.rcv_alt_m)
      for (const v of elevs) {
        if (v < rawMin) rawMin = v
        if (v > rawMax) rawMax = v
      }
      const elevPad = Math.max((rawMax - rawMin) * 0.1, 2)
      const eMin = rawMin - elevPad
      const eMax = rawMax + maxBld + elevPad

      const range = Math.max(eMax - eMin, 1)
      // Native scale: X-axis baseline coincides with the lowest sample.
      // Short buildings get MIN_BUILDING_PX inside the building-rect
      // loop so they stay visible when the vertical pixels-per-metre
      // ratio is small.
      const scale = PLOT_H / range

      const xOf = (t: number) => PAD_L + Math.max(0, Math.min(1, t)) * PLOT_W
      const yOf = (elevM: number) => PAD_T + (eMax - elevM) * scale

      const step = niceTickStep(dist)
      const ticks: number[] = []
      for (let d = 0; d <= dist + step * 0.5; d += step) {
        ticks.push(d)
        if (ticks.length > 12) break
      }

      return { elevMin: eMin, elevMax: eMax, xOf, yOf, xAxisTicks: ticks }
    }, [trace, dist, obstacleEdge])

  // Runs of consecutive segments sharing a ground-hardness bucket share
  // one polyline, so the dash pattern accumulates along the combined
  // length even when individual inter-sample segments are only a couple
  // of pixels wide. Segment bucket = avg IMD of its endpoints.
  const ridgeGroups = useMemo(() => {
    if (n < 2) return [] as Array<{ points: string; dash: string | undefined }>
    const bucketDashAt = (seg: number) =>
      imdBucket((trace.imd_u8[seg] + trace.imd_u8[seg + 1]) / 2).dash
    const pt = (i: number) =>
      `${xOf(trace.t[i]).toFixed(2)},${yOf(trace.elevation_m[i]).toFixed(2)}`
    const groups: Array<{ points: string; dash: string | undefined }> = []
    let runDash = bucketDashAt(0)
    let runPoints: string[] = [pt(0), pt(1)]
    for (let seg = 1; seg + 1 < n; seg++) {
      const segDash = bucketDashAt(seg)
      if (segDash === runDash) {
        runPoints.push(pt(seg + 1))
      } else {
        groups.push({ points: runPoints.join(' '), dash: runDash })
        runPoints = [pt(seg), pt(seg + 1)]
        runDash = segDash
      }
    }
    groups.push({ points: runPoints.join(' '), dash: runDash })
    return groups
  }, [trace, n, xOf, yOf])

  const forestTufts = useMemo(() => {
    const out: { x: number; baseY: number }[] = []
    const stride = Math.max(1, Math.floor(n / 25))
    for (let i = 0; i < n; i += stride) {
      if (trace.forest_u8[i] > 0) {
        out.push({ x: xOf(trace.t[i]), baseY: yOf(trace.elevation_m[i]) })
      }
    }
    return out
  }, [trace, n, xOf, yOf])

  // Raster-sample ring indices, capped at ~200 so an airport path (n in the
  // thousands) doesn't emit thousands of <circle> nodes and freeze the popup on
  // expand. Skips i=0 (source) / i=n-1 (receiver) — they get their own markers.
  const ringSamples = useMemo(() => {
    const stride = Math.max(1, Math.ceil((n - 2) / 200))
    const out: number[] = []
    for (let i = 1; i <= n - 2; i += stride) out.push(i)
    return out
  }, [n])

  // One rectangle for the exact obstacle crossing — not a raster sample.
  const buildingRects = useMemo(() => {
    if (!obstacleEdge || obstacleEdge.height_m <= 0) return []
    const groundAt = (t: number) => {
      if (n === 0) return 0
      let lo = 0
      while (lo < n - 1 && trace.t[lo + 1] < t) lo++
      const hi = Math.min(lo + 1, n - 1)
      const span = trace.t[hi] - trace.t[lo]
      const f = span > 0 ? (t - trace.t[lo]) / span : 0
      return trace.elevation_m[lo] + f * (trace.elevation_m[hi] - trace.elevation_m[lo])
    }
    const base = groundAt(obstacleEdge.t)
    const baseY = yOf(base)
    const hPx = Math.max(baseY - yOf(base + obstacleEdge.height_m), MIN_BUILDING_PX)
    const x = xOf(obstacleEdge.t)
    return [
      {
        x: x - MIN_BUILDING_W / 2,
        y: baseY - hPx,
        w: MIN_BUILDING_W,
        h: hPx,
        label: `${obstacleEdge.height_m.toFixed(0)} m`,
      },
    ]
  }, [trace, n, xOf, yOf, obstacleEdge])

  const svgRef = useRef<SVGSVGElement | null>(null)
  const [hoverIdx, setHoverIdx] = useState<number | null>(null)
  const scrubAt = (clientX: number, clientY: number) => {
    const svg = svgRef.current
    if (!svg) return
    const pt = svg.createSVGPoint()
    pt.x = clientX
    pt.y = clientY
    const ctm = svg.getScreenCTM()
    if (!ctm) return
    const p = pt.matrixTransform(ctm.inverse())
    const t = (p.x - PAD_L) / PLOT_W
    let bestI = 0
    let bestDist = Infinity
    for (let i = 0; i < n; i++) {
      const d = Math.abs(trace.t[i] - t)
      if (d < bestDist) {
        bestDist = d
        bestI = i
      }
    }
    setHoverIdx(bestI)
  }
  const handlePointerMove = (evt: React.PointerEvent<SVGSVGElement>) => {
    scrubAt(evt.clientX, evt.clientY)
  }
  const handlePointerDown = (evt: React.PointerEvent<SVGSVGElement>) => {
    // Touch: capture so moves outside the SVG still feed the scrub bar.
    evt.currentTarget.setPointerCapture?.(evt.pointerId)
    scrubAt(evt.clientX, evt.clientY)
  }
  const handlePointerUp = (evt: React.PointerEvent<SVGSVGElement>) => {
    evt.currentTarget.releasePointerCapture?.(evt.pointerId)
  }
  const handleLeave = () => setHoverIdx(null)

  const srcX = xOf(0)
  const rcvX = xOf(1)
  const srcY = yOf(trace.src_alt_m)
  // `trace.rcv_alt_m` already includes the receiver listening height
  // (engine returns ground + height_m via Receiver::altitude_m).
  const rcvY = yOf(trace.rcv_alt_m)

  // Apex rendering: the single-edge model yields at most ONE diffraction
  // edge — the engine's max-δ dominant edge (δ* anchor) — drawn as a filled
  // dot with an E₁ label.
  const apexMarkers = useMemo(() => {
    if (!terrainEdges || terrainEdges.length === 0) return []
    const e = terrainEdges[0]
    return [{ i: 0, isDominant: true, label: 'E₁', x: xOf(e.t), y: yOf(e.elevation_m), hideLabel: false }]
  }, [terrainEdges, xOf, yOf])

  // All hooks are above this point, so the early return can't change hook order.
  if (!hasPath) {
    return (
      <div className="py-3 text-[10px] italic text-muted-foreground/60">
        No path data — source and receiver coincide.
      </div>
    )
  }

  const groundTooltip =
    'Ground hardness from Copernicus Imperviousness Density\n' +
    '(0 = natural soil, 100 = fully sealed).\n' +
    'CNOSSOS ground factor G = 1 − IMD / 100.\n\n' +
    'Buckets: soft ≤ 33, mixed 34–66, hard ≥ 67.'

  // Scrub columns — fixed `w-[Nch]` widths (max of label or worst-case
  // value) so hover doesn't reflow the row. `justify-between` spreads
  // the leftover slack as uniform gaps.
  const readoutColumns: Array<{
    w: string
    label: string
    labelTooltip?: string
    tabular: boolean
    value: string
  }> = [
    {
      w: 'w-[8ch]',
      label: 'Distance',
      tabular: true,
      value: hoverIdx != null ? `${Math.round(trace.t[hoverIdx] * dist)} m` : '—',
    },
    {
      w: 'w-[6ch]',
      label: 'Elev',
      tabular: true,
      value: hoverIdx != null ? `${trace.elevation_m[hoverIdx].toFixed(0)} m` : '—',
    },
    {
      w: 'w-[8ch]',
      label: 'Building',
      tabular: true,
      value: obstacleEdge ? `${obstacleEdge.height_m.toFixed(0)} m` : '—',
    },
    {
      w: 'w-[6ch]',
      label: 'Forest',
      tabular: false,
      // Canopy density % (continuous tiles, geodata-v2 2a); the legacy
      // binary raster reads 100 where forested, so this stays truthful.
      value: hoverIdx != null ? (trace.forest_u8[hoverIdx] > 0 ? `${trace.forest_u8[hoverIdx]} %` : 'no') : '—',
    },
    {
      w: 'w-[8ch]',
      label: 'Ground',
      labelTooltip: groundTooltip,
      tabular: true,
      value: hoverIdx != null
        ? `${imdLabel(trace.imd_u8[hoverIdx])} ${trace.imd_u8[hoverIdx]}`
        : '—',
    },
  ]

  return (
    <div className="relative mt-1">
      <div className="mb-0.5 flex justify-between text-[11px] leading-tight text-muted-foreground/70">
        {readoutColumns.map(col => (
          <div key={col.label} className={`flex flex-col shrink-0 ${col.w}`}>
            {col.labelTooltip ? (
              <HoverText title={col.labelTooltip}><span>{col.label}</span></HoverText>
            ) : (
              <span>{col.label}</span>
            )}
            <span className={col.tabular ? 'tabular-nums' : undefined}>{col.value}</span>
          </div>
        ))}
      </div>
      <svg
        ref={svgRef}
        viewBox={`0 0 ${VB_W} ${VB_H}`}
        preserveAspectRatio="xMidYMid meet"
        className="w-full touch-none block [&_line]:[vector-effect:non-scaling-stroke] [&_path]:[vector-effect:non-scaling-stroke] [&_polyline]:[vector-effect:non-scaling-stroke]"
        style={{ height: 'auto', maxHeight: 260 }}
        onPointerMove={handlePointerMove}
        onPointerDown={handlePointerDown}
        onPointerUp={handlePointerUp}
        onPointerLeave={handleLeave}
      >
        <rect x={PAD_L} y={PAD_T} width={PLOT_W} height={PLOT_H} fill="var(--color-background, #fafafa)" />

        {/* Terrain ridge — one polyline per same-bucket run; dash pattern
            encodes ground hardness (solid=hard, dashed=mixed, dotted=soft). */}
        {ridgeGroups.map((g, i) => (
          <polyline
            key={`rg-${i}`}
            points={g.points}
            fill="none"
            stroke={DIAGRAM_COLORS.terrain}
            strokeWidth={1.75}
            strokeLinejoin="round"
            strokeLinecap="round"
            strokeDasharray={g.dash}
          />
        ))}

        {forestTufts.map((t, i) => (
          <g key={`tu-${i}`}>
            <polygon
              points={`${t.x - 6},${t.baseY - 14} ${t.x - 12},${t.baseY} ${t.x},${t.baseY}`}
              fill={FOREST_CANOPY}
            />
            <polygon
              points={`${t.x + 6},${t.baseY - 14} ${t.x},${t.baseY} ${t.x + 12},${t.baseY}`}
              fill={FOREST_CANOPY}
            />
            <polygon
              points={`${t.x},${t.baseY - 22} ${t.x - 8},${t.baseY} ${t.x + 8},${t.baseY}`}
              fill={FOREST_HIGHLIGHT}
            />
          </g>
        ))}

        {buildingRects.map((r, i) => (
          <g key={`b-${i}`}>
            <title>{`building ${r.label}`}</title>
            <rect x={r.x} y={r.y} width={r.w} height={r.h} fill="rgba(70,70,70,0.9)" />
            {r.h >= 8 && (
              <line
                x1={r.x}
                y1={r.y + r.h / 2}
                x2={r.x + r.w}
                y2={r.y + r.h / 2}
                stroke="rgba(255,255,255,0.5)"
                strokeWidth={0.75}
              />
            )}
          </g>
        ))}

        {/* Line of sight (source → receiver) */}
        <line
          x1={srcX}
          y1={srcY}
          x2={rcvX}
          y2={rcvY}
          stroke="currentColor"
          strokeOpacity={0.45}
          strokeDasharray="4 3"
          strokeWidth={1.25}
        />

        {/* One ring per raster sample actually read by the engine along
            the path. Skip i=0 (source) and i=n-1 (receiver) — they get
            their own filled blue/red markers below. Drawn before the
            apex so the green apex dot paints on top when they coincide. */}
        {ringSamples.map(i => (
          <circle
            key={`sp-${i}`}
            cx={xOf(trace.t[i])}
            cy={yOf(trace.elevation_m[i])}
            r={4}
            fill="#fafafa"
            stroke="#4a3a1d"
            strokeWidth={1.5}
          />
        ))}

        {apexMarkers.map(m => (
          <g key={`apex-${m.i}`}>
            <circle
              cx={m.x}
              cy={m.y}
              r={4}
              fill={m.isDominant ? DIAGRAM_COLORS.apex : '#fafafa'}
              stroke={DIAGRAM_COLORS.apex}
              strokeWidth={1.75}
            />
            {!m.hideLabel && (
              <text
                x={m.x + 6}
                y={m.y - 5}
                fontSize={23}
                fill={DIAGRAM_COLORS.apex}
                fontWeight={m.isDominant ? 600 : 400}
              >
                {m.label}
              </text>
            )}
          </g>
        ))}

        <circle cx={srcX} cy={srcY} r={4} fill={DIAGRAM_COLORS.source} />
        <line x1={srcX} y1={srcY} x2={srcX} y2={yOf(trace.elevation_m[0])} stroke={DIAGRAM_COLORS.source} strokeWidth={1.5} />

        <circle cx={rcvX} cy={rcvY} r={4} fill={DIAGRAM_COLORS.receiver} />
        <line x1={rcvX} y1={rcvY} x2={rcvX} y2={yOf(trace.elevation_m[n - 1])} stroke={DIAGRAM_COLORS.receiver} strokeWidth={1.5} />

        {hoverIdx != null && (
          <line
            x1={xOf(trace.t[hoverIdx])}
            y1={PAD_T}
            x2={xOf(trace.t[hoverIdx])}
            y2={PAD_T + PLOT_H}
            stroke="currentColor"
            strokeOpacity={0.35}
            strokeWidth={1.25}
          />
        )}

        <line x1={PAD_L} y1={PAD_T + PLOT_H} x2={PAD_L + PLOT_W} y2={PAD_T + PLOT_H} stroke="currentColor" strokeOpacity={0.5} strokeWidth={1.25} />
        {xAxisTicks.map(d => {
          const tx = PAD_L + (d / dist) * PLOT_W
          return (
            <g key={`xt-${d}`}>
              <line x1={tx} y1={PAD_T + PLOT_H} x2={tx} y2={PAD_T + PLOT_H + 4} stroke="currentColor" strokeOpacity={0.5} />
              <text x={tx} y={PAD_T + PLOT_H + 22} textAnchor="middle" fontSize={23} fill="currentColor" opacity={0.75}>
                {formatDist(Math.round(d))}
              </text>
            </g>
          )
        })}

        <text x={6} y={PAD_T + 14} fontSize={23} fill="currentColor" opacity={0.75}>
          {elevMax.toFixed(0)} m
        </text>
        <text x={6} y={PAD_T + PLOT_H - 2} fontSize={23} fill="currentColor" opacity={0.75}>
          {elevMin.toFixed(0)} m
        </text>

      </svg>

    </div>
  )
}
