// The rules of a visitor's places, the same in the browser (browser-places.ts) and on the server for a
// signed-in visitor (server/src/accounts/): the recent places are the history of the places opened,
// clicked or searched, at most 50, the one opened last first; one place is any point within 30 m; a
// name stays at the point it was first given, and of two names for one place the later is kept. Pure,
// so that the server runs this very file.

export interface RecentPlace {
  lat: number
  lng: number
  /** The reverse-geocoded place, when known. */
  place: string | null
  /** The whole day's loudness (sone) and Lden (dB), when answered. */
  sone: number | null
  lden: number | null
  /** When it was opened last (ms since 1970; 0 for a place kept from before the history kept times). */
  openedAt: number
}

/** A place's name as the visitor wrote it. */
export interface PlaceTitle {
  lat: number
  lng: number
  title: string
  /** When it was given (ms since 1970); 0 for a name kept from before names had times. */
  updatedAt: number
}

/** The most recent places kept. */
export const RECENT_PLACES_MAX = 50
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

/** The history's order: the place opened last first (a stable sort: places kept from before the
 *  history kept times keep their order). A visitor goes back to what they opened just before; an order
 *  by how often hid a place searched once under the places opened often (owner 2026-10-30). */
export function historyOrder(list: RecentPlace[]): RecentPlace[] {
  return [...list].sort((a, b) => b.openedAt - a.openedAt)
}

/** `list`, in the history's order, with `entry` replacing the place's entry or added; beyond the most
 *  kept the one ranked last goes, and no other entry stays within the place. */
function withEntry(list: RecentPlace[], entry: RecentPlace): RecentPlace[] {
  return historyOrder([entry, ...list.filter(other => !samePlace(other, entry))]).slice(0, RECENT_PLACES_MAX)
}

/** `list` with the place at `at` opened at `openedAt`: its entry opened again, where it was first
 *  opened (moved, it would take in a neighbour's entry within 30 m of the new point), or a new one
 *  named `place` (a search's result) until its own name comes. */
export function withOpened(list: RecentPlace[], at: { lat: number, lng: number }, openedAt: number, place: string | null = null): RecentPlace[] {
  const older = list.find(other => samePlace(other, at))
  return withEntry(list, older
    ? { ...older, place: older.place ?? place, openedAt }
    : { lat: at.lat, lng: at.lng, place, sone: null, lden: null, openedAt })
}

/** `list` with an answered place's numbers, and a name when its entry has none, where the entry stands;
 *  `list` itself when nothing changes or the place is not in it (forgotten while it was computed). */
export function withAnswer(
  list: RecentPlace[],
  answer: { lat: number, lng: number, place: string | null, sone: number | null, lden: number | null },
): RecentPlace[] {
  const k = list.findIndex(other => samePlace(other, answer))
  if (k < 0) return list
  const older = list[k]
  const entry = { ...older, place: older.place ?? answer.place, sone: answer.sone, lden: answer.lden }
  if (entry.place === older.place && entry.sone === older.sone && entry.lden === older.lden) return list
  return list.map((other, j) => (j === k ? entry : other))
}

/** `list` with `name` given to the same place's entry if it has none yet (a name looked up after
 *  the visitor clicked on comes back late); `list` itself when nothing changes. */
export function withName(list: RecentPlace[], at: { lat: number, lng: number }, name: string): RecentPlace[] {
  if (!list.some(other => other.place == null && samePlace(other, at))) return list
  return list.map(other => (other.place == null && samePlace(other, at) ? { ...other, place: name } : other))
}

/** `list` without the place. */
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

/** The name the visitor gave the place at `at`, with the point it was given at, if any. */
export function titleAt(titles: PlaceTitle[], at: { lat: number, lng: number }): PlaceTitle | null {
  const k = titleIndex(titles, at)
  return k < 0 ? null : titles[k]
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
