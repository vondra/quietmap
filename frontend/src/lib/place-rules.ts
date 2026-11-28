// The rules of a visitor's places, the same in the browser (browser-places.ts) and on the server for a
// signed-in visitor (server/src/accounts/): the recent places are the history of the places opened,
// clicked or searched, at most 50, most recent first. Nearby clicks share a row. A name retains both
// the exact point where it was given and the row that opens it, even after another row is removed.
import { samePoint, wrappedLongitude, type Point } from './coordinates.ts'

/** A completed popup's levels and the exact point requested. */
export interface PointLevels extends Point {
  sone: number | null
  lden: number | null
}

export interface RecentPlace {
  lat: number
  lng: number
  /** The reverse-geocoded place, when known. */
  place: string | null
  levels: PointLevels | null
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
  /** First click of the history row. Older names are paired once with their nearest existing row. */
  anchor?: Point
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

export function readTitleAnchor(value: unknown, at: Point): Point | undefined {
  if (!value || typeof value !== 'object') return undefined
  const { lat, lng } = value as Partial<Point>
  if (typeof lat !== 'number' || !Number.isFinite(lat) || Math.abs(lat) > 90 ||
      typeof lng !== 'number' || !Number.isFinite(lng)) return undefined
  const anchor = { lat, lng: wrappedLongitude(lng) }
  return samePlace(at, anchor) ? anchor : undefined
}

/** Equal distances use coordinates so opening a row or syncing another device cannot move a name. */
export function nearestPlace<T extends Point>(places: T[], at: Point, accepts?: (place: T) => boolean): T | null {
  let best: T | null = null
  let distance = SAME_PLACE_M
  for (const place of places) {
    if (Math.abs(place.lat - at.lat) * METRES_PER_DEGREE > distance) continue
    const metres = metresBetween(at, place)
    if (metres < distance || (best && metres === distance &&
        (place.lat < best.lat || (place.lat === best.lat && place.lng < best.lng)))) {
      if (accepts && !accepts(place)) continue
      if (metres === 0) return place
      best = place
      distance = metres
    }
  }
  return best
}

/** A legacy entry has no record of where its numbers were computed. */
export function readPointLevels(value: unknown): PointLevels | null {
  if (!value || typeof value !== 'object') return null
  const { lat, lng, sone, lden } = value as Partial<PointLevels>
  const finite = (n: unknown): n is number => typeof n === 'number' && Number.isFinite(n)
  if (!finite(lat) || Math.abs(lat) > 90 || !finite(lng)) return null
  return { lat, lng: wrappedLongitude(lng), sone: finite(sone) ? sone : null, lden: finite(lden) ? lden : null }
}

export function levelsAt(entry: RecentPlace | undefined, at: Point): PointLevels | null {
  return entry?.levels && samePoint(entry.levels, at) ? entry.levels : null
}

/** The destination of a history row. A name can be given across the street from its first click. */
export function historyPoint(history: RecentPlace[], titles: PlaceTitle[], entry: RecentPlace): Point & { title?: string } {
  return nearestPlace(titles, entry, title => samePoint(anchorOf(title, history), entry)) ?? entry
}

export function entryOpeningAt(history: RecentPlace[], titles: PlaceTitle[], at: Point): RecentPlace | undefined {
  const anchor = openingAnchor(titles, history, at)
  const entry = history.find(row => samePoint(row, anchor))
  return entry && samePoint(historyPoint(history, titles, entry), at) ? entry : undefined
}

/** The history's order: the place opened last first (a stable sort: places kept from before the
 *  history kept times keep their order). A visitor goes back to what they opened just before; an order
 *  by how often hid a place searched once under the places opened often (owner 2026-10-30). */
export function historyOrder(list: RecentPlace[]): RecentPlace[] {
  return [...list].sort((a, b) => b.openedAt - a.openedAt)
}

/** `list` with the place at `at` opened at `openedAt`: its entry opened again, where it was first
 *  opened (moved, it would take in a neighbour's entry within 30 m of the new point), or a new one
 *  named `place` (a search's result) until its own name comes. */
export function withOpened(list: RecentPlace[], at: Point, openedAt: number, place: string | null = null, titles: PlaceTitle[] = []): RecentPlace[] {
  at = openingAnchor(titles, list, at)
  const older = list.find(row => samePoint(row, at))
  const opened = older
    ? { ...older, place: older.place ?? place, openedAt }
    : { lat: at.lat, lng: at.lng, place, levels: null, openedAt }
  return historyOrder([opened, ...list.filter(row => row !== older)]).slice(0, RECENT_PLACES_MAX)
}

/** Keep a completed answer only in the row that reopens its receiver. */
export function withAnswer(
  list: RecentPlace[],
  answer: PointLevels & { place: string | null },
  titles: PlaceTitle[],
): RecentPlace[] {
  const entry = entryOpeningAt(list, titles, answer)
  if (!entry) return list
  const place = entry.place ?? answer.place
  const previous = levelsAt(entry, answer)
  if (place === entry.place && previous && previous.sone === answer.sone && previous.lden === answer.lden) return list
  const { lat, lng, sone, lden } = answer
  return list.map(other => other === entry ? { ...entry, place, levels: { lat, lng: wrappedLongitude(lng), sone, lden } } : other)
}

/** `list` with `name` given to the same place's entry if it has none yet (a name looked up after
 *  the visitor clicked on comes back late); `list` itself when nothing changes. */
export function withName(list: RecentPlace[], at: Point, name: string, titles: PlaceTitle[] = []): RecentPlace[] {
  const anchor = openingAnchor(titles, list, at)
  const entry = list.find(row => samePoint(row, anchor))
  if (!entry || entry.place != null) return list
  return list.map(other => other === entry ? { ...entry, place: name } : other)
}

/** `list` without the place. */
export function withoutPlace(list: RecentPlace[], place: { lat: number, lng: number }): RecentPlace[] {
  return list.filter(other => !samePoint(other, place))
}

/** A nearby name applies within the same recent place. A saved name outlives its history row. */
export function titleAt(titles: PlaceTitle[], at: Point, history: RecentPlace[] = []): PlaceTitle | null {
  if (!titles.length) return null
  const owner = openingAnchor(titles, history, at)
  return nearestPlace(titles, at, title => samePoint(title, at) || !history.length || samePoint(anchorOf(title, history), owner))
}

function anchorOf(title: PlaceTitle, history: RecentPlace[]): Point {
  return title.anchor ?? nearestPlace(history, title) ?? title
}

const pointOrder = (a: Point, b: Point) => a.lat - b.lat || a.lng - b.lng
const pointKey = (p: Point) => `${p.lat},${wrappedLongitude(p.lng)}`

/** Closest eligible pairs first, with each point on each device participating at most once. */
function closestPairs<L extends Point, R extends Point>(left: L[], right: R[], eligible: (a: L, b: R) => boolean): Map<L, R> {
  const candidates = []
  for (const a of left) for (const b of right) {
    const distance = metresBetween(a, b)
    if (distance < SAME_PLACE_M && eligible(a, b)) candidates.push({ a, b, distance })
  }
  candidates.sort((x, y) => x.distance - y.distance || pointOrder(x.a, y.a) || pointOrder(x.b, y.b))
  const pairs = new Map<L, R>(), taken = new Set<R>()
  for (const { a, b } of candidates) {
    if (pairs.has(a) || taken.has(b)) continue
    pairs.set(a, b)
    taken.add(b)
  }
  return pairs
}

/** Preserve known account rows, then pair the remaining names with the receiving device's first clicks. */
function pairTitleRows(titles: PlaceTitle[], history: RecentPlace[], fixed: Set<PlaceTitle>): PlaceTitle[] {
  const destinations = new Map(titles.map(title => [pointKey(title), title]))
  const used = new Set<string>(), owners = new Map<PlaceTitle, Point>()
  const available = (title: PlaceTitle, owner: Point) => {
    const key = pointKey(owner)
    return !used.has(key) && (!destinations.has(key) || destinations.get(key) === title)
  }
  const assign = (title: PlaceTitle, point: Point) => {
    owners.set(title, { lat: point.lat, lng: point.lng })
    used.add(pointKey(point))
  }
  for (const title of titles) {
    if (fixed.has(title) && title.anchor && available(title, title.anchor)) assign(title, title.anchor)
  }
  const remaining = titles.filter(title => !owners.has(title)).sort(pointOrder)
  const origins = remaining.map(title => ({ ...(title.anchor ?? title), title }))
  const pairs = closestPairs(origins, history, (origin, row) => available(origin.title, row) && samePlace(origin.title, row))
  for (const [origin, row] of pairs) assign(origin.title, row)
  for (const title of remaining) {
    if (!owners.has(title)) assign(title, title.anchor && available(title, title.anchor) ? title.anchor : title)
  }
  return titles.map(title => ({ ...title, anchor: owners.get(title)! }))
}

/** Remember a legacy name's owner before opening or forgetting any more places. */
export function bindPlaceTitles(titles: PlaceTitle[], history: RecentPlace[]): PlaceTitle[] {
  return titles.every(title => title.anchor) ? titles : pairTitleRows(titles, history, new Set(titles))
}

function openingAnchor(titles: PlaceTitle[], history: RecentPlace[], at: Point): Point {
  if (!titles.length) return nearestPlace(history, at) ?? at
  const exact = titles.find(title => samePoint(title, at))
  if (exact) return anchorOf(exact, history)
  const row = nearestPlace(history, at, entry => samePlace(historyPoint(history, titles, entry), at))
  if (row) return row
  const named = nearestPlace(titles, at, title => samePlace(anchorOf(title, history), at))
  return named ? anchorOf(named, history) : at
}

export function titlePosition(titles: PlaceTitle[], history: RecentPlace[], at: Point): { at: Point, anchor: Point } {
  const saved = titleAt(titles, at, history)
  const point = saved ?? at
  const owner = saved ? anchorOf(saved, history) : openingAnchor(titles, history, at)
  return { at: { lat: point.lat, lng: point.lng }, anchor: { lat: owner.lat, lng: owner.lng } }
}

/** Update the name at its saved coordinate, keeping a newer edit that already reached the account. */
export function withTitleAt(titles: PlaceTitle[], at: Point, title: string | null, updatedAt: number, anchor?: Point): PlaceTitle[] {
  const ownerTitle = anchor && titles.find(other => samePoint(other.anchor ?? other, anchor))
  const old = titles.find(other => samePoint(other, at)) ?? (ownerTitle && samePlace(ownerTitle, at) ? ownerTitle : undefined)
  if (old ? old.updatedAt > updatedAt : title == null) return titles
  const others = titles.filter(other => other !== old)
  if (title == null) return others
  const owner = old?.anchor ?? (ownerTitle ? at : anchor)
  return [{ lat: old?.lat ?? at.lat, lng: old?.lng ?? at.lng, title, updatedAt,
    ...(owner && { anchor: { lat: owner.lat, lng: owner.lng } }) }, ...others].slice(0, PLACE_TITLES_MAX)
}

/** An account's names with a browser's added: of two names for one place the later is kept, the
 *  account's on a tie (names kept from before names had times are all equally old); then the newest
 *  most kept, so that no name falls out before the later one for its place is known. */
export function mergedTitles(account: PlaceTitle[], browser: PlaceTitle[], history: RecentPlace[] = []): PlaceTitle[] {
  // A near match must fit the first click on both devices. A neighbour may share only one of them.
  const matched = closestPairs(account, browser, (a, b) => samePoint(a, b) || !a.anchor || !b.anchor ||
    (samePlace(a, b.anchor) && samePlace(b, a.anchor)))
  const existing = account.map(title => {
    const incoming = matched.get(title)
    return incoming && incoming.updatedAt > title.updatedAt ? { ...title, title: incoming.title, updatedAt: incoming.updatedAt } : title
  })
  const imported = new Set(matched.values())
  const merged = [...existing, ...browser.filter(title => !imported.has(title))]
    .sort((a, b) => b.updatedAt - a.updatedAt).slice(0, PLACE_TITLES_MAX)
  return history.length || merged.some(title => title.anchor) ? pairTitleRows(merged, history, new Set(existing)) : merged
}
