import { FastifyInstance } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.js'

interface SearchResult {
  display_name: string
  secondary: string
  lat: number
  lon: number
}

interface PhotonFeature {
  properties: {
    name?: string
    street?: string
    housenumber?: string
    city?: string
    district?: string
    locality?: string
    state?: string
    country?: string
  }
  geometry: {
    coordinates: [number, number]
  }
}

function formatPhotonResult(p: PhotonFeature['properties']): { display_name: string; secondary: string } {
  let primary = ''
  const secondaryParts: string[] = []

  if (p.street) {
    let num = p.housenumber
    if (num) {
      const slash = num.indexOf('/')
      if (slash !== -1) num = num.substring(slash + 1)
      primary = `${p.street} ${num}`
    } else {
      primary = p.street
    }
  } else if (p.name) {
    primary = p.name
  }

  const city = p.city
  const district = p.district || p.locality
  if (city) {
    if (district && district !== city) {
      secondaryParts.push(`${city} – ${district}`)
    } else {
      secondaryParts.push(city)
    }
  } else if (district) {
    secondaryParts.push(district)
  } else if (p.state) {
    secondaryParts.push(p.state)
  }

  if (p.country) secondaryParts.push(p.country)

  return {
    display_name: primary || p.name || '?',
    secondary: secondaryParts.join(', '),
  }
}

const cache = new Map<string, { data: SearchResult[]; expires: number }>()
const reverseCache = new Map<string, { place: string | null; expires: number }>()
const CACHE_TTL = 60 * 60 * 1000

/** Expiry sweep + hard cap shared by both response caches: once over the cap,
 *  sweep expired entries, then drop oldest down to ~80% of the cap. */
function capCache<V extends { expires: number }>(map: Map<string, V>, cap: number): void {
  if (map.size <= cap) return
  const now = Date.now()
  for (const [k, v] of map) { if (v.expires < now) map.delete(k) }
  if (map.size > cap) {
    const excess = map.size - Math.floor(cap * 0.8)
    let removed = 0
    for (const k of map.keys()) {
      if (removed >= excess) break
      map.delete(k)
      removed++
    }
  }
}

/** Shared Photon call — one UA + timeout for /api/search and /api/reverse. */
function fetchPhoton(url: URL): Promise<Response> {
  return fetch(url.toString(), {
    headers: { 'User-Agent': 'quietmap.org/1.0 (noise atlas; contact: info@quietmap.org)' },
    signal: AbortSignal.timeout(3000),
  })
}

/** Place label for the document title ("Dejvice, Praha") — district-level
 *  first so titles read like a neighbourhood, not a house number. */
function formatReversePlace(p: PhotonFeature['properties']): string | null {
  const primary = p.district || p.locality || p.city || p.name || p.state
  if (!primary) return p.country ?? null
  const secondary = [p.city, p.country].find(s => s && s !== primary)
  return secondary ? `${primary}, ${secondary}` : primary
}

export async function searchRoutes(app: FastifyInstance) {
  // Both geocode routes proxy the external Photon service — rate-limited per
  // client (owner directive 2026-07-15) to protect Photon etiquette and us.
  app.get<{ Querystring: { q?: string; lat?: string; lon?: string } }>('/api/search', {
    config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT },
  }, async (request, reply) => {
    const q = request.query.q?.trim()
    if (!q || q.length < 2) return reply.send([])

    const lat = request.query.lat || '50.08'
    const lon = request.query.lon || '14.42'
    const cacheKey = `${q.toLowerCase()}|${parseFloat(lat).toFixed(1)}|${parseFloat(lon).toFixed(1)}`

    const entry = cache.get(cacheKey)
    if (entry && entry.expires > Date.now()) return reply.send(entry.data)

    try {
      const url = new URL('https://photon.komoot.io/api/')
      url.searchParams.set('q', q)
      url.searchParams.set('lat', lat)
      url.searchParams.set('lon', lon)
      url.searchParams.set('lang', 'default')
      url.searchParams.set('limit', '5')

      const res = await fetchPhoton(url)

      if (!res.ok) return reply.send([])

      const data = await res.json() as { features: PhotonFeature[] }
      const results: SearchResult[] = data.features.map(f => {
        const { display_name, secondary } = formatPhotonResult(f.properties)
        return {
          display_name,
          secondary,
          lat: f.geometry.coordinates[1],
          lon: f.geometry.coordinates[0],
        }
      })

      cache.set(cacheKey, { data: results, expires: Date.now() + CACHE_TTL })
      capCache(cache, 1000)

      return reply.send(results)
    } catch {
      return reply.send([])
    }
  })

  // Reverse geocode for the tab/share title — place-first titles like
  // "Dejvice, Praha - 62 dB - quietmap.org" (owner spec 2026-07-10). Same Photon
  // instance and etiquette as /api/search; ~100 m server-side cache.
  app.get<{ Querystring: { lat?: string; lon?: string } }>('/api/reverse', {
    config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT },
  }, async (request, reply) => {
    const lat = parseFloat(request.query.lat ?? '')
    const lon = parseFloat(request.query.lon ?? '')
    if (!Number.isFinite(lat) || Math.abs(lat) > 90 || !Number.isFinite(lon) || Math.abs(lon) > 180) {
      return reply.send({ place: null })
    }
    const cacheKey = `${lat.toFixed(3)}|${lon.toFixed(3)}`
    const entry = reverseCache.get(cacheKey)
    if (entry && entry.expires > Date.now()) return reply.send({ place: entry.place })

    try {
      const url = new URL('https://photon.komoot.io/reverse')
      url.searchParams.set('lat', String(lat))
      url.searchParams.set('lon', String(lon))
      url.searchParams.set('lang', 'default')
      url.searchParams.set('limit', '1')

      const res = await fetchPhoton(url)
      if (!res.ok) return reply.send({ place: null })

      const data = await res.json() as { features: PhotonFeature[] }
      const place = data.features.length ? formatReversePlace(data.features[0].properties) : null
      reverseCache.set(cacheKey, { place, expires: Date.now() + CACHE_TTL })
      capCache(reverseCache, 2000)
      return reply.send({ place })
    } catch {
      return reply.send({ place: null })
    }
  })
}
