/**
 * Period arithmetic for validation: map a station's native averaging windows onto the
 * engine's three END periods (day 07–19, evening 19–23, night 23–07, local time; SPEC).
 */

export type ModelPeriod = 'day' | 'evening' | 'night'
export type PeriodLevels = Record<ModelPeriod, number | null>

/** Local-hour window [start, end), wrapping midnight when end <= start (0–24 is the whole day). */
export type HourWindow = { start: number; end: number }

export const END_PERIOD_WINDOWS: Record<ModelPeriod, HourWindow> = {
  day: { start: 7, end: 19 },
  evening: { start: 19, end: 23 },
  night: { start: 23, end: 7 },
}

export function windowHours(window: HourWindow): number[] {
  const { start, end } = window
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || start > 23 || end < 0 || end > 24) {
    throw new Error(`hour window must use whole local hours 0..24 (got ${start}-${end})`)
  }
  const length = end > start ? end - start : end + 24 - start
  return Array.from({ length }, (_, offset) => (start + offset) % 24)
}

export function modelPeriodOfHour(hour: number): ModelPeriod {
  if (hour >= 7 && hour < 19) return 'day'
  if (hour >= 19 && hour < 23) return 'evening'
  return 'night'
}

/** Energetic sum of levels; null when every level is silent. */
export function energySumDb(levels: ReadonlyArray<number | null>): number | null {
  const energy = levels.reduce<number>((sum, level) => sum + (level == null ? 0 : 10 ** (level / 10)), 0)
  return energy > 0 ? 10 * Math.log10(energy) : null
}

/**
 * LAeq over a native window from the model's period levels. The model carries one level per
 * END period, so an hour takes its period's level; `exact` says the window is a union of whole
 * END periods, where no within-period profile is assumed.
 */
export function windowLevelFromModelPeriods(levels: PeriodLevels, window: HourWindow): { level: number | null; exact: boolean } {
  const hours = windowHours(window)
  const energy = hours.reduce((sum, hour) => {
    const level = levels[modelPeriodOfHour(hour)]
    return sum + (level == null ? 0 : 10 ** (level / 10))
  }, 0)
  const covered = new Set(hours)
  const exact = (Object.keys(END_PERIOD_WINDOWS) as ModelPeriod[]).every(period => {
    const periodHours = windowHours(END_PERIOD_WINDOWS[period])
    const inside = periodHours.filter(hour => covered.has(hour)).length
    return inside === 0 || inside === periodHours.length
  })
  return { level: energy > 0 ? 10 * Math.log10(energy / hours.length) : null, exact }
}

/** Day-evening-night level over the periods' own hours: evening + penalty (END Lden 5 dB), night + 10 dB. */
export function ldenFromPeriods(
  day: number | null, evening: number | null, night: number | null,
  hours: Record<ModelPeriod, number> = { day: 12, evening: 4, night: 8 }, eveningPenaltyDb = 5,
): number | null {
  if (hours.day + hours.evening + hours.night !== 24) throw new Error('day, evening and night must cover 24 hours')
  const energy = hours.day * (day == null ? 0 : 10 ** (day / 10))
    + hours.evening * (evening == null ? 0 : 10 ** ((evening + eveningPenaltyDb) / 10))
    + hours.night * (night == null ? 0 : 10 ** ((night + 10) / 10))
  return energy > 0 ? 10 * Math.log10(energy / 24) : null
}

/** A native Lden (or CNEL): the model's period levels re-averaged over the native windows. */
export function nativeLdenFromModelPeriods(
  levels: PeriodLevels, windows: Record<ModelPeriod, HourWindow>, eveningPenaltyDb = 5,
): { level: number | null; exact: boolean } {
  const periods = Object.keys(END_PERIOD_WINDOWS) as ModelPeriod[]
  const byPeriod = Object.fromEntries(periods.map(period => [period, windowLevelFromModelPeriods(levels, windows[period]).level]))
  const hours = Object.fromEntries(periods.map(period => [period, windowHours(windows[period]).length])) as Record<ModelPeriod, number>
  return {
    level: ldenFromPeriods(byPeriod.day, byPeriod.evening, byPeriod.night, hours, eveningPenaltyDb),
    exact: periods.every(period => windows[period].start === END_PERIOD_WINDOWS[period].start
      && windows[period].end === END_PERIOD_WINDOWS[period].end),
  }
}
