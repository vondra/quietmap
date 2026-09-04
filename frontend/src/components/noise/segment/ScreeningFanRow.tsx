import type { ScreeningFanTrace } from '../../../types/noise'
import { HoverText } from '../../ui/info-tip'
import { InlineTable, fmtDbSigned } from './display'
import {
  FAN_SLICE_CSS,
  FAN_SLICE_LABEL,
  classifyFanSlice,
  type FanSliceKind,
} from '../fanGeometry'

const KIND_ORDER: FanSliceKind[] = ['clear', 'building', 'terrain', 'mixed']

export function ScreeningFanRow({ fan }: { fan: ScreeningFanTrace }) {
  const omitted = fan.intervals_omitted ?? 0
  const intervalCount = fan.intervals.length + omitted
  // Short count ("5 int.") — the row must fit one line next to the label;
  // the full interval list lives in the value tooltip.
  const intervalLabel = `${intervalCount} int.`
  const intervalLines = fan.intervals.map(interval => {
    const state = interval.blocked ? 'blocked' : 'clear'
    const obstacle = interval.obstacle
      ? ` · ${interval.obstacle.kind} ${interval.obstacle.height_m.toFixed(1)} m`
      : ''
    const terrain = interval.terrain_db > 0 ? ` · terrain ${fmtDbSigned(-interval.terrain_db)}` : ''
    const cp = interval.contains_cp ? ' · characteristic point' : ''
    return `${interval.from_deg.toFixed(1)}–${interval.to_deg.toFixed(1)}° · ${state}${obstacle}${terrain} · ΔL ${fmtDbSigned(-interval.screen_db)}${cp}`
  })
  if (omitted > 0) {
    const share = ((fan.omitted_fraction ?? 0) * 100).toFixed(1)
    intervalLines.push(`… ${omitted} smaller interval${omitted === 1 ? '' : 's'} (${share} % of the fan) omitted`)
  }

  // Slice causes actually present — drives the map-fan legend chips below.
  const presentKinds = KIND_ORDER.filter(kind =>
    fan.intervals.some(interval => classifyFanSlice(interval) === kind),
  )

  const row: [React.ReactNode, React.ReactNode] = [
    <HoverText
      title={
        'Angular screening quadrature for this line segment. Angles are\n' +
        'offsets from its characteristic-point ray (0°).'
      }
    >
      Screening fan
    </HoverText>,
    <HoverText
      title={
        `Arc quadrature · span ${fan.span_deg.toFixed(1)}° · ${(fan.blocked_fraction * 100).toFixed(1)} % blocked.\n` +
        'ΔL is each interval\'s A_screen at 1 kHz; terrain is its own ray\'s A_terrain where non-zero.\n' +
        'The engine energy-averages max(A_ground, A_terrain + A_screen) over the interval shares.\n\n' +
        intervalLines.join('\n')
      }
    >
      {intervalLabel} · {Math.round(fan.blocked_fraction * 100)} % blocked
    </HoverText>,
  ]
  return (
    <>
      <InlineTable rows={[row]} />
      {presentKinds.length > 0 && (
        // Indented: the key belongs to the fan row above, so the next data
        // row (Foliage) doesn't read as part of it. The layout div stays
        // outside HoverText (block content can't live in its inline span).
        <div className="mt-1 pl-3 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[10px] text-muted-foreground">
          <HoverText
            title={
              'Slice color on the map = slice cause at 1 kHz:\n' +
              'building = blocked slice (kind + height in the tooltip above),\n' +
              'terrain = A_terrain ≥ 1 dB on that slice’s own ray.\n' +
              'Forest is NOT sliced — the engine resolves it once on the\n' +
              'characteristic ray, so it has no per-slice color.'
            }
          >
            <span className="font-medium text-muted-foreground/80">Map:</span>
          </HoverText>
          {presentKinds.map(kind => (
            <span key={kind} className="inline-flex items-center gap-1.5 whitespace-nowrap">
              <span
                aria-hidden="true"
                className="inline-block h-2.5 w-2.5 rounded-[3px] shrink-0"
                style={{
                  backgroundColor: FAN_SLICE_CSS[kind],
                  boxShadow: 'inset 0 0 0 1px rgba(0,0,0,0.3)',
                }}
              />
              {FAN_SLICE_LABEL[kind]}
            </span>
          ))}
        </div>
      )}
    </>
  )
}
