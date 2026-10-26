// The rules of a visitor's places, the same in the browser (browser-places.ts) and on the server for a
// signed-in visitor (server/src/accounts/): the recent places are the last five opened, newest
// first, a place opened again keeping its tab where it is; one place is any point within 30 m; a name
// stays at the point it was first given, and of two names for one place the later is kept. Pure, so
// that the server runs this very file.

export interface RecentPlace {
  lat: number
  lng: number
  /** The reverse-geocoded place, when known. */
  place: string | null
  /** The whole day's loudness (sone) and Lden (dB), when answered. */
  sone: number | null
  lden: number | null
}

/** A place's name as the visitor wrote it. */
export interface PlaceTitle {
  lat: number
  lng: number
  title: string
  /** When it was given (ms since 1970); 0 for a name kept from before names had times. */
  updatedAt: number
}

/** The most recent places kept: one row of tabs on a phone. */
export const RECENT_PLACES_MAX = 5
/** The most names kept, the newest first. */
export const PLACE_TITLES_MAX = 200
/** The longest name. */
export const TITLE_MAX = 80
/** Two points closer than this (m) are one place. */
const SAME_PLACE_M = 30
const METRES_PER_DEGREE = 111_320

/** A geocoded name's first part (the place before its region and country). */
export function placeLabel(name: string | null): string | null {
  return name?.split(',')[0]?.trim() || null
}

/** The distance between two points (m), flat over a place, across the antimeridian too. */
function metresBetween(a: { lat: number, lng: number }, b: { lat: number, lng: number }): number {
  const east = ((b.lng - a.lng + 540) % 360) - 180
  return Math.hypot((b.lat - a.lat) * METRES_PER_DEGREE, east * METRES_PER_DEGREE * Math.cos((a.lat * Math.PI) / 180))
}

export function samePlace(a: { lat: number, lng: number }, b: { lat: number, lng: number }): boolean {
  return metresBetween(a, b) < SAME_PLACE_M
}

/** `list` with `place`: a new place first, the oldest beyond the most kept gone; a place already in
 *  it where its first entry stands, as tabs do, with `place`'s numbers and its older name until its
 *  own arrives, no other entry left within the place. */
export function withPlace(list: RecentPlace[], place: RecentPlace): RecentPlace[] {
  const k = list.findIndex(other => samePlace(other, place))
  if (k < 0) return [place, ...list].slice(0, RECENT_PLACES_MAX)
  const entry = place.place == null ? { ...place, place: list[k].place } : place
  return list.flatMap((other, j) => (j === k ? [entry] : samePlace(other, place) ? [] : [other]))
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

/** Which of `titles` names the place at `at`: the nearest within one place's 30 m (the first of
 *  equals), or -1. Two named spots a house apart stay two, and naming a point again keeps the name
 *  where it was given. */
function titleIndex(titles: PlaceTitle[], at: { lat: number, lng: number }): number {
  let nearest = -1
  let nearestM = SAME_PLACE_M
  titles.forEach((title, k) => {
    const metres = metresBetween(title, at)
    if (metres < nearestM) [nearest, nearestM] = [k, metres]
  })
  return nearest
}

/** The name the visitor gave the place at `at`, if any. */
export function titleAt(titles: PlaceTitle[], at: { lat: number, lng: number }): string | null {
  const k = titleIndex(titles, at)
  return k < 0 ? null : titles[k].title
}

/** `titles` with the place at `at` named `title` at `updatedAt`, first and at the point its name was
 *  first given, or without its name when `title` is null; a name given later than `updatedAt` stays
 *  (and `titles` itself is the answer). Uncapped. */
function named(titles: PlaceTitle[], at: { lat: number, lng: number }, title: string | null, updatedAt: number): PlaceTitle[] {
  const k = titleIndex(titles, at)
  if (k >= 0 && titles[k].updatedAt > updatedAt) return titles
  const others = titles.filter((_, j) => j !== k)
  if (title == null) return others
  const point = k < 0 ? at : titles[k]
  return [{ lat: point.lat, lng: point.lng, title, updatedAt }, ...others]
}

/** As `named`, at most the most kept, the newest first. */
export function withTitle(
  titles: PlaceTitle[],
  at: { lat: number, lng: number },
  title: string | null,
  updatedAt: number,
): PlaceTitle[] {
  const next = named(titles, at, title, updatedAt)
  return next === titles ? titles : next.slice(0, PLACE_TITLES_MAX)
}

/** An account's names with a browser's added: of two names for one place the later is kept, the
 *  account's on a tie (names kept from before names had times are all equally old); then the newest
 *  most kept, so that no name falls out before the later one for its place is known. */
export function mergedTitles(account: PlaceTitle[], browser: PlaceTitle[]): PlaceTitle[] {
  const oldestFirst = [...browser].sort((a, b) => a.updatedAt - b.updatedAt)
  const merged = oldestFirst.reduce((titles, title) => {
    const k = titleIndex(titles, title)
    return k >= 0 && titles[k].updatedAt >= title.updatedAt ? titles : named(titles, title, title.title, title.updatedAt)
  }, account)
  return [...merged].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, PLACE_TITLES_MAX)
}
