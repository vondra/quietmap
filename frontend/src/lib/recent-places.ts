// The visitor's recent places: the last points opened, newest first, each with its place name and
// its numbers, kept in this browser only, so that a few known places stay at hand to compare and
// to reopen; and the names the visitor gave places, kept apart so a place keeps its name after it
// leaves the recent few.

export interface RecentPlace {
  lat: number
  lng: number
  /** The reverse-geocoded place, when known. */
  place: string | null
  /** The whole day's loudness (sone) and Lden (dB), when answered. */
  sone: number | null
  lden: number | null
}

/** A geocoded name's first part (the place before its region and country). */
export function placeLabel(name: string | null): string | null {
  return name?.split(',')[0]?.trim() || null
}

/** The most places kept: one row of tabs on a phone. */
export const RECENT_PLACES_MAX = 5
const STORAGE_KEY = 'quietmap.recent-places'
/** Two points closer than this (m) are one place. */
const SAME_PLACE_M = 30
const METRES_PER_DEGREE = 111_320

/** The distance between two points (m), flat over a place. */
function metresBetween(a: { lat: number, lng: number }, b: { lat: number, lng: number }): number {
  const north = (b.lat - a.lat) * METRES_PER_DEGREE
  const east = (b.lng - a.lng) * METRES_PER_DEGREE * Math.cos((a.lat * Math.PI) / 180)
  return Math.hypot(north, east)
}

export function samePlace(a: { lat: number, lng: number }, b: { lat: number, lng: number }): boolean {
  return metresBetween(a, b) < SAME_PLACE_M
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

/** A place's name as the visitor wrote it. */
export interface PlaceTitle {
  lat: number
  lng: number
  title: string
}

/** The most names kept, the newest first. */
export const PLACE_TITLES_MAX = 200
/** The names' storage key: another tab's change of it reaches this one as a `storage` event. */
export const PLACE_TITLES_KEY = 'quietmap.place-titles'

/** Which of `titles` names the place at `at`: the nearest within one place's 30 m, or -1. Two
 *  named spots a house apart stay two, and naming a point again keeps the name where it was given. */
function titleIndex(titles: PlaceTitle[], at: { lat: number, lng: number }): number {
  let nearest = -1
  for (let k = 0; k < titles.length; k++) {
    if (samePlace(titles[k], at) && (nearest < 0 || metresBetween(titles[k], at) < metresBetween(titles[nearest], at))) {
      nearest = k
    }
  }
  return nearest
}

/** The name the visitor gave the place at `at`, if any. */
export function titleAt(titles: PlaceTitle[], at: { lat: number, lng: number }): string | null {
  const k = titleIndex(titles, at)
  return k < 0 ? null : titles[k].title
}

/** `titles` with the place at `at` named `title` (first, at the point its name was first given),
 *  or without its name when `title` is null, at most the most kept. */
export function withTitle(titles: PlaceTitle[], at: { lat: number, lng: number }, title: string | null): PlaceTitle[] {
  const k = titleIndex(titles, at)
  const others = titles.filter((_, j) => j !== k)
  if (title == null) return others
  const point = k < 0 ? at : titles[k]
  return [{ lat: point.lat, lng: point.lng, title }, ...others].slice(0, PLACE_TITLES_MAX)
}

/** The stored names (none when storage holds something else), or null when the browser keeps no
 *  storage. */
export function loadPlaceTitles(): PlaceTitle[] | null {
  let stored: string | null
  try {
    stored = localStorage.getItem(PLACE_TITLES_KEY)
  } catch {
    return null
  }
  try {
    const parsed: unknown = JSON.parse(stored ?? '[]')
    if (!Array.isArray(parsed)) return []
    return parsed.flatMap((entry: Record<string, unknown>) => {
      const [lat, lng] = [numberOrNull(entry?.lat), numberOrNull(entry?.lng)]
      const title = typeof entry?.title === 'string' ? entry.title.trim() : ''
      return lat == null || lng == null || !title ? [] : [{ lat, lng, title }]
    }).slice(0, PLACE_TITLES_MAX)
  } catch {
    return []
  }
}

export function savePlaceTitles(titles: PlaceTitle[]): void {
  try {
    localStorage.setItem(PLACE_TITLES_KEY, JSON.stringify(titles))
  } catch {
    // A private window or blocked storage keeps the names for this visit only.
  }
}
