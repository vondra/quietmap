// The visitor's recent places: the last points opened, newest first, each with its place name and
// its numbers, kept in this browser only, so that a few known places stay at hand to compare and
// to reopen.

export interface RecentPlace {
  lat: number
  lng: number
  /** The reverse-geocoded place, when known. */
  place: string | null
  /** The whole day's loudness (sone) and Lden (dB), when answered. */
  sone: number | null
  lden: number | null
}

/** The most places kept: one row of tabs on a phone. */
export const RECENT_PLACES_MAX = 5
const STORAGE_KEY = 'quietmap.recent-places'
/** Two points closer than this (m) are one place. */
const SAME_PLACE_M = 30
const METRES_PER_DEGREE = 111_320

export function samePlace(a: { lat: number, lng: number }, b: { lat: number, lng: number }): boolean {
  const north = (b.lat - a.lat) * METRES_PER_DEGREE
  const east = (b.lng - a.lng) * METRES_PER_DEGREE * Math.cos((a.lat * Math.PI) / 180)
  return Math.hypot(north, east) < SAME_PLACE_M
}

/** `list` with `place` first, in place of the same place's older entry (whose name it keeps until
 *  its own arrives), at most the most kept. */
export function withPlace(list: RecentPlace[], place: RecentPlace): RecentPlace[] {
  const older = list.find(other => samePlace(other, place))
  const entry = place.place == null && older?.place != null ? { ...place, place: older.place } : place
  return [entry, ...list.filter(other => !samePlace(other, place))].slice(0, RECENT_PLACES_MAX)
}

/** `list` with `name` given to the same place's entry if it has none yet (a name looked up after
 *  the visitor clicked on comes back late); `list` itself when nothing changes. */
export function withName(list: RecentPlace[], at: { lat: number, lng: number }, name: string): RecentPlace[] {
  if (!list.some(other => other.place == null && samePlace(other, at))) return list
  return list.map(other => (other.place == null && samePlace(other, at) ? { ...other, place: name } : other))
}

export function withoutPlace(list: RecentPlace[], place: { lat: number, lng: number }): RecentPlace[] {
  return list.filter(other => !samePlace(other, place))
}

const numberOrNull = (value: unknown) => (typeof value === 'number' && Number.isFinite(value) ? value : null)

/** The stored places; none when the browser keeps no storage or holds something else. */
export function loadRecentPlaces(): RecentPlace[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]')
    if (!Array.isArray(parsed)) return []
    return parsed.flatMap((entry: Record<string, unknown>) => {
      const [lat, lng] = [numberOrNull(entry?.lat), numberOrNull(entry?.lng)]
      if (lat == null || lng == null) return []
      return [{
        lat,
        lng,
        place: typeof entry.place === 'string' ? entry.place : null,
        sone: numberOrNull(entry.sone),
        lden: numberOrNull(entry.lden),
      }]
    }).slice(0, RECENT_PLACES_MAX)
  } catch {
    return []
  }
}

export function saveRecentPlaces(list: RecentPlace[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(list))
  } catch {
    // A private window or blocked storage keeps the places for this visit only.
  }
}
