// Stay22's accommodation search: the places to stay with a room in a box for a stay, each at its
// cheapest supplier's price, each kind in two lists of up to three pages, every call paid from the
// budget its caller passes.

const STAY22_URL = 'https://api.stay22.com/v2/accommodations'
/** The currency every price is asked and answered in. */
export const CURRENCY = 'EUR'
/** Stay22's largest page: it refuses a larger one. */
const PAGE_SIZE = 100
/** Pages read of one list, so a list holds at most 300 places. */
const PAGES = 3
/** H3's cell area at resolution 0 as its finer resolutions give it: each is a seventh of the one
 *  before, resolution 3 averaging 12,393 km². */
const H3_R0_KM2 = 12_393.4 * 7 ** 3
const UPSTREAM_TIMEOUT_MS = 10_000
/** The longest wait a Retry-After is believed for: Stay22 counts a key's calls by the minute. */
const MAX_WAIT_S = 60

export interface Box {
  swlat: number
  swlng: number
  nelat: number
  nelng: number
}

/** What the visitor searches: the stay, and which places (Stay22's names and values). */
export interface StaySearch {
  checkin: string
  checkout: string
  adults?: string
  /** Hotels, the rest ("rental": apartments, guest houses, hostels…), or both. */
  types: ('hotel' | 'rental')[]
  minstarrating?: string
  minguestrating?: string
}

/** A place to stay as the map and the popup show it. */
export interface Listing {
  id: string
  name: string
  lat: number
  lng: number
  /** The cheapest supplier's price of the whole stay; null when no supplier quoted one. */
  total: number | null
  stars: number | null
  /** The guests' score out of 10, and the reviews it averages. */
  score: number | null
  reviews: number | null
  guests: number | null
  bedrooms: number | null
  freeCancellation: boolean
  thumbnail: string | null
  /** Stay22's link to the offer, which carries the affiliate id. */
  url: string
}

/** The calls a search may make: `take` counts one and answers 0, or answers the milliseconds until
 *  one is free (counting nothing); `pause` stops every call for the seconds Stay22 asked. */
export interface Budget {
  take(): number
  pause(seconds: number): void
}

interface Stay22Result {
  id?: unknown
  name?: unknown
  url?: unknown
  location?: { coordinates?: { lat?: unknown; lng?: unknown } }
  suppliers?: Record<string, { price?: { total?: unknown } } | undefined>
  rating?: { value?: unknown; hotelStars?: unknown; count?: unknown }
  capacity?: { guests?: unknown; bedrooms?: unknown }
  policies?: { freeCancellation?: unknown }
  media?: { thumbnail?: unknown }
}

interface Stay22Page {
  results: Stay22Result[]
  meta?: { total?: unknown }
}

/** Stay22 refused the search itself (an impossible date, too many guests): its words go to the
 *  visitor. */
export class Stay22Refusal extends Error {}

/** No call goes to Stay22 for `seconds`: it answered 429, a minute's calls are spent, or too many
 *  searches run at once. */
export class Stay22Busy extends Error {
  readonly seconds: number
  constructor(seconds: number) {
    super('Stay22 is busy')
    this.seconds = seconds
  }
}

/** A Retry-After's seconds, believed up to a minute: a missing, negative or absurd one (999999999
 *  would stop every search for 31 years) waits the minute. */
export function retryAfterSeconds(header: string | null): number {
  const seconds = Math.ceil(Number(header))
  return seconds >= 1 ? Math.min(seconds, MAX_WAIT_S) : MAX_WAIT_S
}

/** The H3 resolution of the one-place-per-cell list: the finest whose cells over the box are no
 *  more than a list holds, so no cell's place is cut. */
export function clusterPrecision(box: Box): number {
  const km = 111.32
  const areaKm2 = (box.nelat - box.swlat) * km * (box.nelng - box.swlng) * km
    * Math.cos(((box.swlat + box.nelat) / 2) * (Math.PI / 180))
  const cells = PAGE_SIZE * PAGES
  return Math.min(15, Math.max(0, Math.floor(Math.log((cells * H3_R0_KM2) / areaKm2) / Math.log(7))))
}

// The page shows links and images as href and src: https only, as a javascript: link would run.
const https = (value: unknown) => (typeof value === 'string' && value.startsWith('https://') ? value : null)
// The page computes with these: a string such as "8.9" would break it.
const finite = (value: unknown) => (typeof value === 'number' && Number.isFinite(value) ? value : null)

/** A Stay22 result as the page shows it; null without an id, a name, a place or an https link. */
export function slimListing(result: Stay22Result): Listing | null {
  const lat = finite(result.location?.coordinates?.lat)
  const lng = finite(result.location?.coordinates?.lng)
  const url = https(result.url)
  const id = typeof result.id === 'string' || typeof result.id === 'number' ? String(result.id) : null
  if (id == null || lat == null || lng == null || url == null || typeof result.name !== 'string' || !result.name) {
    return null
  }
  const totals = Object.values(result.suppliers ?? {})
    .map(supplier => finite(supplier?.price?.total))
    .filter((total): total is number => total != null)
  return {
    id,
    name: result.name,
    lat,
    lng,
    total: totals.length ? Math.min(...totals) : null,
    stars: finite(result.rating?.hotelStars),
    score: finite(result.rating?.value),
    reviews: finite(result.rating?.count),
    guests: finite(result.capacity?.guests),
    bedrooms: finite(result.capacity?.bedrooms),
    freeCancellation: result.policies?.freeCancellation === true,
    thumbnail: https(result.media?.thumbnail),
    url,
  }
}

async function stay22Page(params: URLSearchParams, apiKey: string, budget: Budget, signal: AbortSignal): Promise<Stay22Page> {
  const wait = budget.take()
  if (wait > 0) throw new Stay22Busy(Math.ceil(wait / 1000))
  const response = await fetch(`${STAY22_URL}?${params}`, {
    headers: { 'X-API-KEY': apiKey },
    signal: AbortSignal.any([signal, AbortSignal.timeout(UPSTREAM_TIMEOUT_MS)]),
  })
  const body = await response.json().catch(() => null) as (Stay22Page & { message?: unknown }) | null
  if (response.status === 400) {
    throw new Stay22Refusal(`Stay22: ${typeof body?.message === 'string' ? body.message : 'the search was refused'}`)
  }
  if (response.status === 429) {
    const seconds = retryAfterSeconds(response.headers.get('retry-after'))
    budget.pause(seconds)
    throw new Stay22Busy(seconds)
  }
  // A success without results is a failure, never "no rooms".
  if (!response.ok || !Array.isArray(body?.results)) throw new Error(`Stay22 answered HTTP ${response.status} without results`)
  return body
}

/** One list, every page of it: the first tells how many places there are. */
async function stay22List(params: URLSearchParams, apiKey: string, budget: Budget, signal: AbortSignal): Promise<Stay22Page[]> {
  const page = (number: number) => {
    const paged = new URLSearchParams(params)
    paged.set('page', String(number))
    return stay22Page(paged, apiKey, budget, signal)
  }
  const first = await page(1)
  const pages = Math.min(PAGES, Math.ceil((finite(first.meta?.total) ?? 0) / PAGE_SIZE))
  const rest = await Promise.all(Array.from({ length: Math.max(0, pages - 1) }, (_, at) => page(at + 2)))
  return [first, ...rest]
}

/**
 * The places of a box (measured 2026-10-08). Each kind is searched apart: asked together,
 * apartments (nine in ten of the places) crowded the hotels out, and central Prague showed 4 places
 * with a room that night where hotels alone had 47. Each kind is two lists, as neither suffices
 * alone: Stay22's own order prices the places it ranks first and so crowds where it ranks most
 * (Paris the next night: 148 hotels, in 10 of 16 parts of the city), one place per H3 cell reaches
 * every part (91 hotels, in all 16) but drops a cell whose place has no room. A list that fails costs
 * only its own places, with the failure answered beside the rest; a busy Stay22 stops every list.
 */
export async function searchStays(
  box: Box, search: StaySearch, { aid, apiKey }: { aid: string; apiKey: string }, budget: Budget,
): Promise<{ listings: Listing[]; failure: unknown }> {
  const base = new URLSearchParams({
    swlat: String(box.swlat),
    swlng: String(box.swlng),
    nelat: String(box.nelat),
    nelng: String(box.nelng),
    checkin: search.checkin,
    checkout: search.checkout,
    currency: CURRENCY,
    pageSize: String(PAGE_SIZE),
    aid,
  })
  for (const name of ['adults', 'minstarrating', 'minguestrating'] as const) {
    const value = search[name]
    if (value !== undefined) base.set(name, value)
  }
  const lists = search.types.flatMap(type => {
    const ranked = new URLSearchParams(base)
    ranked.set('type', type)
    const perCell = new URLSearchParams(ranked)
    perCell.set('cluster', 'top')
    perCell.set('precision', String(clusterPrecision(box)))
    return [ranked, perCell]
  })
  const busy = new AbortController()
  const settled = await Promise.allSettled(lists.map(params => stay22List(params, apiKey, budget, busy.signal)
    .catch((error: unknown) => {
      if (error instanceof Stay22Busy) busy.abort()
      throw error
    })))
  const failures = settled.flatMap(list => (list.status === 'rejected' ? [list.reason as unknown] : []))
  // The busy list says why its neighbours were stopped.
  const failure = failures.find(error => error instanceof Stay22Busy) ?? failures[0] ?? null
  if (failures.length === settled.length) throw failure
  const listings = new Map<string, Listing>()
  for (const list of settled) {
    if (list.status === 'rejected') continue
    for (const page of list.value) {
      for (const result of page.results) {
        const listing = slimListing(result)
        if (listing && !listings.has(listing.id)) listings.set(listing.id, listing)
      }
    }
  }
  return { listings: [...listings.values()], failure }
}
