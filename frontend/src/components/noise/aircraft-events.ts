// What flies over the point in words: per band of peak level the flights of an average day, those of
// them at night, their height and their commonest type. Pure TypeScript, so it has a dependency-free
// unit test.
import type { AircraftEvents } from '../../types/noise.ts'
import { aircraftTypeName } from '../../utils/aircraft-types.ts'
import { fmtCount } from '../../utils/formatters.ts'

export interface AircraftEventRow {
  /** "≥ 50 dB" */
  peak: string
  /** Flights a day: "412", "1.5", "0.05" */
  perDay: string
  /** Of them at night, a day: "21", or "–" without */
  night: string
  /** Kilometres above the ground: "0.6", or "–" */
  heightKm: string
  /** The type in words, else its designator; empty without */
  type: string
  /** The type with its designator, for its hover tip */
  typeTitle: string
}

/** The bands any flight reaches, the quietest first. */
export function aircraftEventRows(events: AircraftEvents): AircraftEventRow[] {
  return events.above_db.flatMap((above, band) => {
    const perDay = events.per_day[band] ?? 0
    if (perDay <= 0) return []
    const night = events.night[band] ?? 0
    const height = events.height_m[band]
    const designator = events.type[band] ?? ''
    const type = designator ? aircraftTypeName(designator) : ''
    return [{
      peak: `≥ ${above} dB`,
      perDay: fmtCount(perDay),
      night: night > 0 ? fmtCount(night) : '–',
      heightKm: height == null ? '–' : (height / 1000).toFixed(1),
      type,
      typeTitle: type === designator ? type : `${type} (${designator})`,
    }]
  })
}
