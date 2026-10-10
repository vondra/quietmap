// A source's sound path, as the computation summed it over its rays (Lden): its sound power spread
// over the distance alone, then what air absorption, buildings and terrain, and the ground effect
// take or add, in calm air and downwind, to the level of each; and how often each holds. The level
// by period under it is the two mixed by those shares.
import type { SourcePath } from '../../../types/noise'
import { HoverText } from '../../ui/info-tip'
import { DetailTable } from '../shared'

const DISTANCE_ONLY = 'Its sound power spread over the distance, with nothing in between'
const AIR_ABSORPTION = 'Absorbed by the air (ISO 9613-1): more at high pitch and over long distances'
const BUILDINGS_AND_TERRAIN = 'Blocked by buildings, walls and hills in between (CNOSSOS-EU)'
const GROUND_EFFECT = 'Soft ground (fields, forest floor) absorbs, hard ground (asphalt, water) reflects'
export const REFLECTIONS = 'Reflected by the buildings around the point'
const CALM = 'Sound travels in straight lines'
const DOWNWIND = 'Wind toward the point, or a temperature inversion at night, bends sound\ndown over obstacles and carries it further (CNOSSOS-EU favourable conditions)'
const SHARE_OF_TIME = 'Day · evening · night (ERA5 weather, by direction)'

/** A level of the account in dB, negative ones too (one under 0 dB can rise above it). */
function level(db: number | null): string {
  return db == null ? '—' : db.toFixed(1)
}

/** A term in dB with its sign. */
export function signed(db: number | null): string {
  if (db == null) return '—'
  const tenth = Math.round(db * 10) / 10
  return `${tenth > 0 ? '+' : tenth < 0 ? '−' : ''}${Math.abs(tenth).toFixed(1)}`
}

export function PathTable({ path }: { path: SourcePath }) {
  if (path.free_lden == null) return null
  const shares = (downwind: boolean) => path.bent_percent
    .map(p => (p == null ? '—' : String(downwind ? p : 100 - p)))
    .join(' · ') + ' %'
  const both = (value: string) => [value, value]
  const rows = [
    [<HoverText title={DISTANCE_ONLY}>Distance only</HoverText>, ...both(level(path.free_lden))],
    [<HoverText title={AIR_ABSORPTION}>Air absorption</HoverText>, ...both(signed(path.air_db))],
    [<HoverText title={BUILDINGS_AND_TERRAIN}>Buildings and terrain</HoverText>, signed(path.screening_db[0]), signed(path.screening_db[1])],
    [<HoverText title={GROUND_EFFECT}>Ground effect</HoverText>, signed(path.ground_db[0]), signed(path.ground_db[1])],
    ...(path.facades_db ? [[<HoverText title={REFLECTIONS}>Reflections</HoverText>, ...both(signed(path.facades_db))]] : []),
    [<span className="text-foreground">Level</span>, ...path.lden.map(level)],
    [<HoverText title={SHARE_OF_TIME}>Share of time</HoverText>, shares(false), shares(true)],
  ]
  return (
    <DetailTable
      head={[
        'Sound path, dB Lden',
        <HoverText title={CALM}>Calm</HoverText>,
        <HoverText title={DOWNWIND}>Downwind</HoverText>,
      ]}
      rows={rows}
    />
  )
}
