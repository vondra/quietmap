import type { ScreeningFanTrace } from '../../../types/noise'
import { HoverText } from '../../ui/info-tip'
import { InlineTable, fmtDbSigned } from './display'

export function ScreeningFanRow({ fan }: { fan: ScreeningFanTrace }) {
  const omitted = fan.intervals_omitted ?? 0
  const intervalCount = fan.intervals.length + omitted
  const intervalLabel = `${intervalCount} interval${intervalCount === 1 ? '' : 's'}`
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
        'Each listed obstacle is a representative edge; other edges may supply the 1 kHz band envelope.\n' +
        'The engine energy-averages max(A_ground, A_terrain + A_screen) over the interval shares.\n\n' +
        intervalLines.join('\n')
      }
    >
      {intervalLabel} · {Math.round(fan.blocked_fraction * 100)} % blocked
    </HoverText>,
  ]
  return <InlineTable rows={[row]} />
}
