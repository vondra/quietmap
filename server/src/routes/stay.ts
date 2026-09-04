import type { FastifyInstance } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.js'

// Live proxy to Stay22's Direct Travel API (bookable hotels + vacation
// rentals with affiliate links). Their terms forbid cold-storing listing
// data, so results live only in a short in-memory cache (55 min, their
// recommended TTL). The keyless demo tier allows 5 requests/min — viewport
// queries are snapped to a coarse grid so panning reuses cache entries, and
// a global token bucket throttles upstream calls; when the budget is spent
// a bucket serves its stale copy rather than erroring.
const STAY22_URL = 'https://api.stay22.com/v2/accommodations'
// Affiliate id + API key come from the environment; without a key the route
// degrades to the keyless demo tier (aid=stay22, 5 req/min, no payouts).
const AID = process.env.STAY22_AID || 'stay22'
const API_KEY = process.env.STAY22_API_KEY || null
const CACHE_TTL_MS = 55 * 60 * 1000
// Expired entries may still be served when the upstream budget is spent or
// the upstream is down — but a quote older than this is worse than an empty
// map (prices drift, listings close).
const STALE_MAX_MS = 6 * 60 * 60 * 1000
const CACHE_MAX = 300
// Authenticated tier publishes no hard number — 30/min is conservative and
// an order below anything a map UI needs; keyless demo is 5/min, keep one
// in reserve. Fixed sliding window, no burst.
const UPSTREAM_PER_MIN = API_KEY ? 30 : 4
// Server-side snap normalizes stray clients; honest clients pre-snap to a
// zoom-tiered grid (0.01/0.05/0.5 — see StayLayer). All tiers are multiples
// of this finest step, so re-snapping their values is an identity.
const GRID_DEG = 0.01
// Refuse world-scale boxes; anything under this works — zoomed-out viewports
// get coarse H3 representatives, not a flat dump.
const MAX_SPAN_DEG = 12
// Upstream page size cap. A bucket may spend up to PAGES_MAX pages, so the
// H3 precision fit (pickPrecision) targets PAGE_SIZE * PAGES_MAX cells — if
// those drift apart, cluster=top silently truncates to a biased subset.
const PAGE_SIZE = 100
const PAGES_MAX = 3
// At street spans the paged flat list is complete, so density is the point
// (owner 2026-07-29: Václavák showed no hotels). Any wider and a flat list
// truncates to Stay22's own ranking — which clusters spatially — so wider
// boxes use one-per-H3-cell sampling instead. ~3×2 km keeps totals under
// the page budget even in dense city cores (Václavák box ≈ 264).
const FLAT_SPAN_DEG = 0.0301

interface SlimStay {
  id: string
  name: string
  lat: number
  lng: number
  thumbnail: string | null
  rating: { value: number | null; count: number | null; stars: number | null }
  capacity: { guests: number | null; bedrooms: number | null }
  freeCancellation: boolean
  price: { total: number; perNight: number } | null
  url: string
}
interface StayPayload {
  listings: SlimStay[]
  meta: { checkin: string; checkout: string; nights: number; currency: string; stale?: boolean; partial?: boolean }
}

const cache = new Map<string, { at: number; payload: StayPayload }>()
// Single-flight: concurrent requests for one bucket share one upstream call
// (and one token) instead of racing past the completed-entry cache.
const inflight = new Map<string, Promise<StayPayload>>()

// Sliding-window call log, not a token bucket: a bucket with refill lets a
// full burst plus refills reach 2× the cap inside one 60 s window, which
// would trip Stay22's own limiter.
const upstreamCalls: number[] = []
function takeToken(): boolean {
  const now = Date.now()
  while (upstreamCalls.length > 0 && upstreamCalls[0] <= now - 60_000) upstreamCalls.shift()
  if (upstreamCalls.length >= UPSTREAM_PER_MIN) return false
  upstreamCalls.push(now)
  return true
}

/** Test-only: multi-page scenarios would otherwise exhaust the real window. */
export function resetUpstreamWindowForTests(): void {
  upstreamCalls.length = 0
}

// A stable near-future stay (4 weeks out, 2 nights) so prices are real and
// comparable across pins; the dates sit in the cache key, so entries roll
// over naturally at midnight.
const CHECKIN_OFFSET_DAYS = 28
const NIGHTS = 2
function defaultDates(): { checkin: string; checkout: string; nights: number } {
  const day = (offset: number) => new Date(Date.now() + offset * 86_400_000).toISOString().slice(0, 10)
  return { checkin: day(CHECKIN_OFFSET_DAYS), checkout: day(CHECKIN_OFFSET_DAYS + NIGHTS), nights: NIGHTS }
}

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/
// Shape AND calendar validity: V8 normalizes 2026-09-31 to Oct 1, which
// would let two spellings of one stay alias different upstream requests
// under one cache key — a canonical round-trip rejects those.
const canonicalDate = (s: string): boolean =>
  DATE_RE.test(s) && new Date(Date.parse(s)).toISOString().slice(0, 10) === s
/** Client-picked stay window; anything unparseable or unreasonable falls back
 *  to the defaults rather than erroring — the filter is best-effort. */
export function pickDates(checkin?: string, checkout?: string): { checkin: string; checkout: string; nights: number } {
  if (checkin && checkout && canonicalDate(checkin) && canonicalDate(checkout)) {
    const nights = Math.round((Date.parse(checkout) - Date.parse(checkin)) / 86_400_000)
    const leadDays = (Date.parse(checkin) - Date.now()) / 86_400_000
    if (nights >= 1 && nights <= 30 && leadDays >= -1 && leadDays <= 540) return { checkin, checkout, nights }
  }
  return defaultDates()
}

const intIn = (v: string | undefined, lo: number, hi: number): number | null => {
  const n = Number(v)
  return Number.isInteger(n) && n >= lo && n <= hi ? n : null
}

// The epsilon keeps grid-boundary values in place — bare floor(50.05/0.05)
// lands on 1000.999…, snapping a whole cell too far and doubling the box.
// Mirrored in frontend/src/components/StayLayer.tsx so client URLs land on
// the same buckets — keep the two in sync.
export const snap = (v: number, up: boolean) => {
  const q = v / GRID_DEG
  return ((up ? Math.ceil(q - 1e-9) : Math.floor(q + 1e-9)) * GRID_DEG).toFixed(2)
}

// Average H3 hex areas (km²) for resolutions r3..r10. r3 covers the largest
// allowed box (12° at the equator, ~1.8M km², ~143 r3 cells ≤ the budget).
const H3_AREA_KM2: [number, number][] = [[3, 12392.7], [4, 1770.3], [5, 252.9], [6, 36.13], [7, 5.161], [8, 0.7373], [9, 0.1053], [10, 0.01505]]

// Finest H3 resolution whose cell count over the bbox still fits the page
// budget — then `cluster=top` returns EVERY cell's best-rated stay and
// coverage is uniform; a finer grid would truncate to a biased subset.
export function pickPrecision(swlat: number, swlng: number, nelat: number, nelng: number): number {
  const midLat = ((swlat + nelat) / 2) * (Math.PI / 180)
  const areaKm2 = (nelat - swlat) * 111 * (nelng - swlng) * 111 * Math.cos(midLat)
  let res = H3_AREA_KM2[0][0]
  for (const [r, hex] of H3_AREA_KM2) if (areaKm2 / hex <= PAGE_SIZE * PAGES_MAX) res = r
  return res
}

// Upstream strings that reach the client as href/src must be https — a
// poisoned `javascript:` link would execute on click (React escapes text,
// not URL schemes).
const httpsOnly = (v: unknown): string | null =>
  typeof v === 'string' && v.startsWith('https://') ? v : null

// The client renders and computes with these — a stray string from upstream
// (e.g. "8.9") would throw in the card's toFixed.
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)

export function slim(result: any, nights: number): SlimStay | null {
  const lat = result?.location?.coordinates?.lat
  const lng = result?.location?.coordinates?.lng
  const url = httpsOnly(result?.url)
  if (typeof lat !== 'number' || typeof lng !== 'number' || !url || !result?.name) return null
  // The cheapest supplier drives the shown price; per-supplier detail is not
  // exposed — the card's single CTA is the aggregated roam link.
  const totals = Object.values(result.suppliers ?? {})
    .map((s: any) => num(s?.price?.total))
    .filter((t): t is number => t != null)
  const cheapest = totals.length > 0 ? Math.min(...totals) : null
  return {
    id: String(result.id),
    name: result.name,
    lat,
    lng,
    thumbnail: httpsOnly(result.media?.thumbnail),
    rating: {
      value: num(result.rating?.value),
      count: num(result.rating?.count),
      stars: num(result.rating?.hotelStars),
    },
    capacity: {
      guests: num(result.capacity?.guests),
      bedrooms: num(result.capacity?.bedrooms),
    },
    freeCancellation: result.policies?.freeCancellation === true,
    price: cheapest != null ? { total: cheapest, perNight: Math.round(cheapest / nights) } : null,
    url,
  }
}

export async function stayRoutes(app: FastifyInstance): Promise<void> {
  app.get('/api/stay', { config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT } }, async (request, reply) => {
    const q = request.query as Record<string, string | undefined>
    const swlat = parseFloat(q.swlat ?? '')
    const swlng = parseFloat(q.swlng ?? '')
    const nelat = parseFloat(q.nelat ?? '')
    const nelng = parseFloat(q.nelng ?? '')
    if (![swlat, swlng, nelat, nelng].every(Number.isFinite) || nelat <= swlat || nelng <= swlng) {
      return reply.code(400).send({ error: 'invalid bbox' })
    }
    if (nelat - swlat > MAX_SPAN_DEG || nelng - swlng > MAX_SPAN_DEG) {
      return reply.code(400).send({ error: 'bbox too large' })
    }
    const type = q.type === 'hotel' || q.type === 'rental' ? q.type : null

    // Optional owner-facing filters — validated, forwarded, and part of the
    // cache key. Stay22's `max` is per-night USD pre-conversion; the client
    // enters €, close enough for a filter.
    const dates = pickDates(q.checkin, q.checkout)
    const adults = intIn(q.adults, 1, 16)
    const maxPrice = intIn(q.max, 1, 100_000)
    const minStars = intIn(q.minstars, 1, 5)
    const minRating = intIn(q.minrating, 1, 10)
    const bbox = { swlat: snap(swlat, false), swlng: snap(swlng, false), nelat: snap(nelat, true), nelng: snap(nelng, true) }
    const key = [bbox.swlat, bbox.swlng, bbox.nelat, bbox.nelng, type ?? 'all',
      dates.checkin, dates.nights, adults, maxPrice, minStars, minRating].join('|')

    // Success responses only — an explicit max-age on a 429/502 would let
    // shared caches (Cloudflare) pin the error for 5 minutes. A partial
    // (window-truncated) set must not be pinned by ANY cache: the client
    // retries it on the next moveend.
    const sendOk = (payload: StayPayload) =>
      reply.header('Cache-Control', payload.meta.partial ? 'no-store' : 'public, max-age=300').send(payload)

    const hit = cache.get(key)
    if (hit && Date.now() - hit.at < CACHE_TTL_MS) return sendOk(hit.payload)
    const stale = hit && Date.now() - hit.at < STALE_MAX_MS
      ? { ...hit.payload, meta: { ...hit.payload.meta, stale: true } }
      : null

    let pending = inflight.get(key)
    if (!pending) {
      if (!takeToken()) {
        // Out of demo budget: a stale copy beats an error, an empty map beats a 500.
        if (stale) return sendOk(stale)
        return reply.code(429).header('Retry-After', '15').send({ error: 'rate limited' })
      }

      const params = new URLSearchParams({
        ...bbox,
        checkin: dates.checkin,
        checkout: dates.checkout,
        pageSize: String(PAGE_SIZE),
        currency: 'eur',
        aid: AID,
        campaign: '0db',
      })
      if (adults != null) params.set('adults', String(adults))
      if (maxPrice != null) params.set('max', String(maxPrice))
      if (minStars != null) params.set('minstarrating', String(minStars))
      if (minRating != null) params.set('minguestrating', String(minRating))
      const spanLat = parseFloat(bbox.nelat) - parseFloat(bbox.swlat)
      const spanLng = parseFloat(bbox.nelng) - parseFloat(bbox.swlng)
      // Street spans try the complete flat list first; anything wider goes
      // straight to one-best-per-H3-cell — a flat list there truncates to
      // Stay22's own ranking, which clusters wherever the city is densest
      // and leaves the rest of the screen empty.
      const flatEligible = spanLat <= FLAT_SPAN_DEG && spanLng <= FLAT_SPAN_DEG
      const precision = String(pickPrecision(parseFloat(bbox.swlat), parseFloat(bbox.swlng), parseFloat(bbox.nelat), parseFloat(bbox.nelng)))
      if (type) params.set('type', type)

      pending = (async () => {
        const BUDGET = PAGE_SIZE * PAGES_MAX
        // Page through one mode. The caller prepaid the first call's window
        // slot; every further call (page 2+, or a mode refetch) pays its own —
        // when the window is spent, a partial set beats waiting. A flat pass
        // aborts the moment page 1 reveals total > BUDGET: paging on would
        // burn the window the clustered refetch needs for ITS pages.
        const fetchSet = async (clustered: boolean, prepaid: boolean) => {
          const p = new URLSearchParams(params)
          if (clustered) {
            p.set('cluster', 'top')
            p.set('precision', precision)
          }
          const results: any[] = []
          let tokenStarved = false
          let aborted = false
          let total: number | null = null
          for (let page = 1; page <= PAGES_MAX; page++) {
            if (!(page === 1 && prepaid) && !takeToken()) { tokenStarved = true; break }
            p.set('page', String(page))
            const res = await fetch(`${STAY22_URL}?${p}`, {
              signal: AbortSignal.timeout(12_000),
              headers: API_KEY ? { 'X-API-KEY': API_KEY } : undefined,
            })
            if (!res.ok) throw new Error(`stay22 ${res.status}`)
            const data: any = await res.json()
            const batch = ((data?.results ?? []) as any[]).slice(0, PAGE_SIZE)
            results.push(...batch)
            total = num(data?.meta?.total)
            if (!clustered && total != null && total > BUDGET) { aborted = true; break }
            if (batch.length < PAGE_SIZE || (total != null && results.length >= total)) break
          }
          return { results, tokenStarved, aborted, total }
        }

        let set = await fetchSet(!flatEligible, true)
        let cacheable = !set.tokenStarved
        // A dense street box can exceed even the flat page budget — the cut
        // is then Stay22's spatially clustered ranking (owner 2026-07-29:
        // pins piled in one corner). Uniform per-cell sampling beats a
        // biased-but-larger list, so refetch clustered. With no meta.total,
        // a budget's worth of full batches is treated as truncation too.
        const truncated = set.aborted || (set.total != null
          ? set.total > set.results.length
          : set.results.length >= BUDGET)
        if (flatEligible && !set.tokenStarved && truncated) {
          const uniform = await fetchSet(true, false)
          if (uniform.results.length > 0 && !uniform.tokenStarved) {
            set = uniform
          } else {
            // The biased flat cut must not become the bucket's cached truth.
            cacheable = false
            if (uniform.results.length > 0) set = uniform
          }
        }

        const byId = new Map<string, SlimStay>()
        for (const r of set.results) {
          const s = slim(r, dates.nights)
          if (s && !byId.has(s.id)) byId.set(s.id, s)
        }
        const payload: StayPayload = {
          listings: [...byId.values()],
          meta: { ...dates, currency: 'EUR', ...(cacheable ? {} : { partial: true }) },
        }
        // A window-truncated or bias-suspect page set must not become the
        // bucket's truth for a whole TTL (nor evict a complete stale entry) —
        // serve it once and let the next request retry.
        if (cacheable) {
          cache.delete(key) // re-insert so a refreshed bucket is newest for eviction
          cache.set(key, { at: Date.now(), payload })
          while (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value!)
        }
        return payload
      })()
      inflight.set(key, pending)
      void pending.catch(() => {}).finally(() => inflight.delete(key))
    }

    try {
      return sendOk(await pending)
    } catch (err) {
      request.log.warn({ err }, 'stay22 fetch failed')
      if (stale) return sendOk(stale)
      return reply.code(502).send({ error: 'upstream unavailable' })
    }
  })
}
