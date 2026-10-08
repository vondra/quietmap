// GET /api/stay: the places to stay with a room in a map view for the visitor's dates and guests,
// each at its cheapest supplier's price: Stay22's accommodation search, kept in memory only (its
// terms allow no stored copy of its listings) and rate-limited per client. Without a Stay22 account
// it answers 503, and the noise map stays up.
import type { FastifyInstance, FastifyReply } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.ts'
import { coordinate } from './popup.ts'

const STAY22_URL = 'https://api.stay22.com/v2/accommodations'
/** The currency every price is asked and answered in. */
const CURRENCY = 'EUR'
/** Stay22's largest page: it refuses a larger one. */
const PAGE_SIZE = 100
/** Pages read of one list, so a list holds at most 300 places. */
const PAGES = 3
/** H3's cell area at resolution 0 as its finer resolutions give it: each is a seventh of the one
 *  before, resolution 3 averaging 12,393 km². */
const H3_R0_KM2 = 12_393.4 * 7 ** 3
/** The widest box searched: zoom 7, where the layer starts, on a screen 2,900 pixels wide. A wider
 *  view is answered for its middle. */
const MAX_SPAN_DEG = 16
/** Stay22's recommended lifetime of an answer, as its API documentation gave it (dev1, 2026-07). */
const CACHE_TTL_MS = 55 * 60_000
const CACHE_MAX = 300
const UPSTREAM_TIMEOUT_MS = 10_000

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

export interface StayAnswer {
  listings: Listing[]
  nights: number
  currency: string
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
  results?: Stay22Result[]
  meta?: { total?: unknown }
}

/** Stay22 refused the search itself (a check-in before its today, the UTC day, …): its words go to
 *  the visitor. */
class Stay22Refusal extends Error {}

/** Stay22 takes 150 searches a minute from a key (measured 2026-10-08: the 151st within a minute is
 *  answered 429, with the seconds left of the minute in Retry-After); none is sent before then. */
class Stay22Busy extends Error {
  readonly until: number
  constructor(retryAfter: string | null) {
    super('Stay22 is busy')
    this.until = Date.now() + 1000 * (Number(retryAfter) || 60)
  }
}

const DATE = /^\d{4}-\d{2}-\d{2}$/
const ADULTS = /^[1-9][0-9]?$/
const STARS = /^[1-5]$/
const SCORE = /^([1-9]|10)$/

/** The middle `MAX_SPAN_DEG` of a span. */
function middle(low: number, high: number): [number, number] {
  const centre = (low + high) / 2
  const half = Math.min(high - low, MAX_SPAN_DEG) / 2
  return [centre - half, centre + half]
}

/** The view and the search a query names, or what is wrong with it. A check-in Stay22 would refuse
 *  (before its today, the UTC day) is refused here, at no cost of its searches; Stay22 judges the
 *  rest (an impossible date, too many guests) and its refusal is passed on. */
export function parseStayQuery(query: Record<string, unknown>): { view: Box; search: StaySearch } | string {
  const [south, west, north, east] = [query.swlat, query.swlng, query.nelat, query.nelng].map(coordinate)
  if (south == null || west == null || north == null || east == null
    || south < -90 || north > 90 || west < -180 || east > 180 || north <= south || east <= west) {
    return 'swlat, swlng, nelat and nelng must be a box within ±90 and ±180'
  }
  const [swlat, nelat] = middle(south, north)
  const [swlng, nelng] = middle(west, east)
  const { checkin, checkout, adults, type, minstars, minscore } = query
  if (typeof checkin !== 'string' || !DATE.test(checkin) || typeof checkout !== 'string' || !DATE.test(checkout)) {
    return 'checkin and checkout must be dates (YYYY-MM-DD)'
  }
  const today = new Date().toISOString().slice(0, 10)
  if (checkin < today || checkout <= checkin) return `checkin must be ${today} (UTC) or later, and checkout after it`
  if (adults !== undefined && (typeof adults !== 'string' || !ADULTS.test(adults))) return 'adults must be a number of guests'
  if (type !== undefined && type !== 'hotel' && type !== 'rental') return 'type must be hotel or rental'
  if (minstars !== undefined && (typeof minstars !== 'string' || !STARS.test(minstars))) return 'minstars must be 1 to 5'
  if (minscore !== undefined && (typeof minscore !== 'string' || !SCORE.test(minscore))) return 'minscore must be 1 to 10'
  return {
    view: { swlat, swlng, nelat, nelng },
    search: {
      checkin,
      checkout,
      ...(adults !== undefined ? { adults } : {}),
      types: type === undefined ? ['hotel', 'rental'] : [type],
      ...(minstars !== undefined ? { minstarrating: minstars } : {}),
      ...(minscore !== undefined ? { minguestrating: minscore } : {}),
    },
  }
}

/** The box a view is answered for: its edges moved out to a grid of 1, 2 or 5 times a power of
 *  ten, the coarsest at most a quarter of the view, so the views around a place at one zoom share
 *  an answer and a box is at most 2.25 times its view. Every such grid divides 90 and 180, so a box
 *  stays on the globe. */
export function snapBox(view: Box): Box {
  const quarter = Math.max(view.nelat - view.swlat, view.nelng - view.swlng, 0.001) / 4
  const decade = 10 ** Math.floor(Math.log10(quarter))
  const grid = decade * [5, 2, 1].find(step => step * decade <= quarter * (1 + 1e-9))!
  // The epsilon keeps an edge on the grid in place: floor(50.05 / 0.05) is 1000.99…
  const down = (value: number) => +(Math.floor(value / grid + 1e-9) * grid).toFixed(6)
  const up = (value: number) => +(Math.ceil(value / grid - 1e-9) * grid).toFixed(6)
  return { swlat: down(view.swlat), swlng: down(view.swlng), nelat: up(view.nelat), nelng: up(view.nelng) }
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

async function stay22Page(params: URLSearchParams, apiKey: string, signal: AbortSignal): Promise<Stay22Page> {
  const response = await fetch(`${STAY22_URL}?${params}`, {
    headers: { 'X-API-KEY': apiKey },
    signal: AbortSignal.any([signal, AbortSignal.timeout(UPSTREAM_TIMEOUT_MS)]),
  })
  const body = await response.json().catch(() => null) as (Stay22Page & { message?: unknown }) | null
  if (response.status === 400) {
    throw new Stay22Refusal(`Stay22: ${typeof body?.message === 'string' ? body.message : 'the search was refused'}`)
  }
  if (response.status === 429) throw new Stay22Busy(response.headers.get('retry-after'))
  if (!response.ok || body == null) throw new Error(`Stay22 answered HTTP ${response.status}`)
  return body
}

/** One list, every page of it: the first tells how many places there are. */
async function stay22List(params: URLSearchParams, apiKey: string, signal: AbortSignal): Promise<Stay22Page[]> {
  const page = (number: number) => {
    const paged = new URLSearchParams(params)
    paged.set('page', String(number))
    return stay22Page(paged, apiKey, signal)
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
 * every part (91 hotels, in all 16) but drops a cell whose place has no room.
 */
async function searchStays(box: Box, search: StaySearch, { aid, apiKey }: { aid: string; apiKey: string }): Promise<StayAnswer> {
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
  // A search that failed sends no more pages: each would spend one of the minute's 150.
  const failed = new AbortController()
  const pages = (await Promise.all(lists.map(params => stay22List(params, apiKey, failed.signal)))
    .catch((error: unknown) => {
      failed.abort()
      throw error
    })).flat()
  const listings = new Map<string, Listing>()
  for (const page of pages) {
    for (const result of page.results ?? []) {
      const listing = slimListing(result)
      if (listing && !listings.has(listing.id)) listings.set(listing.id, listing)
    }
  }
  const nights = Math.round((Date.parse(search.checkout) - Date.parse(search.checkin)) / 86_400_000)
  return { listings: [...listings.values()], nights, currency: CURRENCY }
}

export async function stayRoutes(app: FastifyInstance, { stay22 }: { stay22: { aid: string; apiKey: string } | null }) {
  if (!stay22) app.log.warn('STAY22_AID and STAY22_API_KEY are not set: /api/stay answers 503')
  const answers = new Map<string, { at: number; answer: StayAnswer }>()
  // Views of one box asked at once share one search.
  const searching = new Map<string, Promise<StayAnswer>>()
  let busyUntil = 0
  const busy = (reply: FastifyReply) => {
    const seconds = Math.max(1, Math.ceil((busyUntil - Date.now()) / 1000))
    return reply.code(503).header('Retry-After', String(seconds))
      .send({ error: `Stay22 takes no more searches this minute; places to stay again in ${seconds} s.` })
  }

  app.get<{ Querystring: Record<string, unknown> }>('/api/stay', {
    config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT },
  }, async (request, reply) => {
    if (!stay22) return reply.code(503).send({ error: 'Places to stay are not set up on this server.' })
    const query = parseStayQuery(request.query)
    if (typeof query === 'string') return reply.code(400).send({ error: query })
    const box = snapBox(query.view)
    const key = JSON.stringify([box, query.search])
    const kept = answers.get(key)
    if (kept && Date.now() - kept.at < CACHE_TTL_MS) return kept.answer
    if (Date.now() < busyUntil) return busy(reply)
    let search = searching.get(key)
    if (!search) {
      search = searchStays(box, query.search, stay22)
        .then(answer => {
          answers.delete(key)
          answers.set(key, { at: Date.now(), answer })
          if (answers.size > CACHE_MAX) answers.delete(answers.keys().next().value!)
          return answer
        })
        .finally(() => searching.delete(key))
      searching.set(key, search)
    }
    try {
      return await search
    } catch (error) {
      if (error instanceof Stay22Refusal) return reply.code(400).send({ error: error.message })
      if (error instanceof Stay22Busy) {
        busyUntil = Math.max(busyUntil, error.until)
        return busy(reply)
      }
      request.log.warn({ err: error }, 'Stay22 search failed')
      return reply.code(502).send({ error: 'Stay22 did not answer; no places to stay for now.' })
    }
  })
}
