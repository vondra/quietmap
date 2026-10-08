// GET /api/stay: the places to stay with a room in a map view for the visitor's dates and guests
// (Stay22's search, ../stay22.ts), kept in memory for Stay22's lifetime of an answer and never past
// it (its terms allow no stored copy of its listings), on Stay22's calls a minute shared between
// visitors. Without a Stay22 account it answers 503, and the noise map stays up.
import type { FastifyInstance, FastifyReply } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT, rateLimitClientKey } from '../rate-limit.ts'
import { CURRENCY, Stay22Busy, Stay22Refusal, searchStays, type Box, type Budget, type Listing, type StaySearch } from '../stay22.ts'
import { coordinate } from './popup.ts'

/** The widest box searched: zoom 7, where the layer starts, on a screen 2,900 pixels wide. A wider
 *  view is answered for its middle. */
const MAX_SPAN_DEG = 16
/** Stay22's lifetime of an answer, as its API documentation gave it (2026-07). */
const LIFETIME_MS = 55 * 60_000
const CACHE_MAX = 300
const MINUTE_MS = 60_000
/** Stay22's calls a minute for a key (measured 2026-10-08: the 151st within a minute is answered
 *  429). */
const KEY_CALLS_PER_MINUTE = 150
/** One visitor's share of them: a third, so that one visitor's panning leaves two others room. */
const VISITOR_CALLS_PER_MINUTE = 50
/** Searches at once: the minute's calls sustain about one (4 to 12 calls of 0.5 to 2.5 s each). */
const SEARCHES_AT_ONCE = 4

/** A view's answer: its places, and when they expire (Stay22's lifetime from when they were asked). */
interface StayAnswer {
  listings: Listing[]
  nights: number
  expires: number
  /** Why some lists are missing, when only part of the search failed; such an answer is not kept. */
  failure: unknown
}

const DATE = /^\d{4}-\d{2}-\d{2}$/
const ADULTS = /^[1-9][0-9]?$/
const STARS = /^[1-5]$/
const SCORE = /^([1-9]|10)$/

/** A calendar date, YYYY-MM-DD: Date.parse reads 2026-11-31 as 1 December. */
function isDate(text: unknown): text is string {
  if (typeof text !== 'string' || !DATE.test(text)) return false
  const time = Date.parse(text)
  return Number.isFinite(time) && new Date(time).toISOString().startsWith(text)
}

/** The middle `MAX_SPAN_DEG` of a span. */
function middle(low: number, high: number): [number, number] {
  const centre = (low + high) / 2
  const half = Math.min(high - low, MAX_SPAN_DEG) / 2
  return [centre - half, centre + half]
}

/** The view and the search a query names, or what is wrong with it. A check-in Stay22 would refuse
 *  (before its today, the UTC day) is refused here, at no cost of its calls; Stay22 judges the rest
 *  and its refusal is passed on. */
export function parseStayQuery(query: Record<string, unknown>): { view: Box; search: StaySearch } | string {
  const [south, west, north, east] = [query.swlat, query.swlng, query.nelat, query.nelng].map(coordinate)
  if (south == null || west == null || north == null || east == null
    || south < -90 || north > 90 || west < -180 || east > 180 || north <= south || east <= west) {
    return 'swlat, swlng, nelat and nelng must be a box within ±90 and ±180'
  }
  const [swlat, nelat] = middle(south, north)
  const [swlng, nelng] = middle(west, east)
  const { checkin, checkout, adults, type, minstars, minscore } = query
  if (!isDate(checkin) || !isDate(checkout)) return 'checkin and checkout must be dates (YYYY-MM-DD)'
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
 *  ten, the coarsest at most a quarter of the view's longer side, so the views around a place at one
 *  zoom share an answer. Every such grid divides 90 and 180, so a box stays on the globe. */
export function snapBox(view: Box): Box {
  const quarter = Math.max(view.nelat - view.swlat, view.nelng - view.swlng, 0.001) / 4
  const decade = 10 ** Math.floor(Math.log10(quarter))
  const grid = decade * [5, 2, 1].find(step => step * decade <= quarter * (1 + 1e-9))!
  // The epsilon keeps an edge on the grid in place: floor(50.05 / 0.05) is 1000.99…
  const down = (value: number) => +(Math.floor(value / grid + 1e-9) * grid).toFixed(6)
  const up = (value: number) => +(Math.ceil(value / grid - 1e-9) * grid).toFixed(6)
  return { swlat: down(view.swlat), swlng: down(view.swlng), nelat: up(view.nelat), nelng: up(view.nelng) }
}

/** The status, words and wait a failure is answered with. */
function explain(failure: unknown): { status: number; error: string; retryAfter?: number } {
  if (failure instanceof Stay22Refusal) return { status: 400, error: failure.message }
  if (failure instanceof Stay22Busy) {
    return { status: 503, retryAfter: failure.seconds, error: `Stay22 is asked too often just now; places to stay again in ${failure.seconds} s.` }
  }
  return { status: 502, error: 'Stay22 did not answer; no places to stay for now.' }
}

export async function stayRoutes(app: FastifyInstance, { stay22 }: { stay22: { aid: string; apiKey: string } | null }) {
  if (!stay22) app.log.warn('STAY22_AID and STAY22_API_KEY are not set: /api/stay answers 503')
  const answers = new Map<string, StayAnswer>()
  // Views of one box asked at once share one search.
  const searching = new Map<string, Promise<StayAnswer>>()
  // The calls to Stay22 of the last minute, the key's and each visitor's, and the end of a pause
  // Stay22 asked for.
  const keyCalls: number[] = []
  const visitorCalls = new Map<string, number[]>()
  let pausedUntil = 0

  const budget = (visitor: string): Budget => ({
    take() {
      const now = Date.now()
      const mine = visitorCalls.get(visitor) ?? []
      for (const calls of [keyCalls, mine]) while (calls.length && calls[0] <= now - MINUTE_MS) calls.shift()
      const wait = Math.max(pausedUntil - now,
        keyCalls.length < KEY_CALLS_PER_MINUTE ? 0 : keyCalls[0] + MINUTE_MS - now,
        mine.length < VISITOR_CALLS_PER_MINUTE ? 0 : mine[0] + MINUTE_MS - now)
      if (wait > 0) return wait
      keyCalls.push(now)
      mine.push(now)
      visitorCalls.set(visitor, mine)
      if (visitorCalls.size > 10_000) {
        for (const [other, calls] of visitorCalls) if (calls[calls.length - 1] <= now - MINUTE_MS) visitorCalls.delete(other)
      }
      return 0
    },
    pause(seconds) {
      pausedUntil = Math.max(pausedUntil, Date.now() + seconds * 1000)
    },
  })

  // Stay22's terms keep no listing past its lifetime: an answer leaves memory when it expires.
  const keep = (key: string, answer: StayAnswer) => {
    answers.delete(key)
    answers.set(key, answer)
    if (answers.size > CACHE_MAX) answers.delete(answers.keys().next().value!)
    setTimeout(() => { if (answers.get(key) === answer) answers.delete(key) }, answer.expires - Date.now()).unref()
  }

  const fail = (reply: FastifyReply, failure: unknown) => {
    const { status, error, retryAfter } = explain(failure)
    if (status === 502) reply.log.warn({ err: failure }, 'Stay22 search failed')
    if (retryAfter) reply.header('Retry-After', String(retryAfter))
    return reply.code(status).send({ error })
  }

  app.get<{ Querystring: Record<string, unknown> }>('/api/stay', {
    config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT },
  }, async (request, reply) => {
    // The answer expires: no cache in front may keep it.
    reply.header('Cache-Control', 'no-store')
    if (!stay22) return reply.code(503).send({ error: 'Places to stay are not set up on this server.' })
    const query = parseStayQuery(request.query)
    if (typeof query === 'string') return reply.code(400).send({ error: query })
    const box = snapBox(query.view)
    const key = JSON.stringify([box, query.search])
    const kept = answers.get(key)
    let search = kept && kept.expires > Date.now() ? Promise.resolve(kept) : searching.get(key)
    if (!search) {
      if (searching.size >= SEARCHES_AT_ONCE) return fail(reply, new Stay22Busy(1))
      const asked = Date.now()
      const { checkin, checkout } = query.search
      search = searchStays(box, query.search, stay22, budget(rateLimitClientKey(request.ip)))
        .then(({ listings, failure }) => {
          const nights = Math.round((Date.parse(checkout) - Date.parse(checkin)) / 86_400_000)
          const answer = { listings, nights, expires: asked + LIFETIME_MS, failure }
          if (failure == null) keep(key, answer)
          return answer
        })
        .finally(() => searching.delete(key))
      searching.set(key, search)
    }
    let answer: StayAnswer
    try {
      answer = await search
    } catch (failure) {
      return fail(reply, failure)
    }
    const failure = answer.failure == null ? null : explain(answer.failure)
    if (failure?.retryAfter) reply.header('Retry-After', String(failure.retryAfter))
    return reply.send({
      listings: answer.listings,
      nights: answer.nights,
      currency: CURRENCY,
      // Seconds left, so the page's clock does not matter; a kept answer does not start anew.
      expiresIn: Math.max(0, Math.floor((answer.expires - Date.now()) / 1000)),
      failure: failure?.error ?? null,
    })
  })
}
