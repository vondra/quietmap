// GET /api/search (address suggestions) and /api/reverse (the place name of the tab title): a
// proxy to the Photon geocoder, filtered to what a visitor looks for, cached in memory and
// rate-limited per client.
import type { FastifyInstance } from 'fastify'
import { EXPENSIVE_ROUTE_RATE_LIMIT } from '../rate-limit.ts'

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
    osm_key?: string
    osm_value?: string
  }
  geometry: {
    coordinates: [number, number]
  }
}

/** What a visitor of a noise map looks for: places, streets and addresses, nature, somewhere to
 *  stay, landmarks and stations. Everything else (bus stops, guideposts, motorway exits, bars,
 *  shops, artworks) crowded out the place itself: "sněžka" gave a bus stop and three guideposts. */
const SEARCHABLE: Record<string, true | readonly string[]> = {
  place: true,
  boundary: ['administrative', 'national_park', 'protected_area'],
  building: true,
  natural: true,
  waterway: ['river', 'canal'],
  landuse: true,
  leisure: true,
  highway: ['motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'unclassified', 'residential',
    'living_street', 'pedestrian', 'service', 'road'],
  tourism: ['hotel', 'motel', 'guest_house', 'hostel', 'chalet', 'apartment', 'alpine_hut',
    'wilderness_hut', 'camp_site', 'caravan_site', 'viewpoint', 'attraction', 'museum', 'zoo',
    'theme_park'],
  historic: ['castle', 'ruins', 'fort', 'manor', 'monastery', 'archaeological_site'],
  railway: ['station', 'halt'],
  aeroway: ['aerodrome'],
  amenity: ['school', 'kindergarten', 'university', 'college', 'hospital'],
}

function searchable(p: PhotonFeature['properties']): boolean {
  if (p.housenumber) return true
  const allowed = p.osm_key ? SEARCHABLE[p.osm_key] : undefined
  return allowed === true || (allowed !== undefined && allowed.includes(p.osm_value ?? ''))
}

/** Results the visitor cannot tell apart: the same label anywhere (Nantes the city and Nantes
 *  the metropolis), or the same name within 5 km (Paris the city and Paris the boundary; a
 *  village and its cadastral area). */
const SAME_PLACE_M = 5_000

function samePlace(a: SearchResult, b: SearchResult): boolean {
  if (a.display_name !== b.display_name) return false
  if (a.secondary === b.secondary) return true
  const dy = (a.lat - b.lat) * 111_320
  const dx = (a.lon - b.lon) * 111_320 * Math.cos((a.lat * Math.PI) / 180)
  return Math.hypot(dx, dy) < SAME_PLACE_M
}

/** Results shown, out of the more Photon is asked for so that filtering leaves enough. */
const SHOWN = 5
const ASKED = 20

function formatPhotonResult(p: PhotonFeature['properties']): { display_name: string; secondary: string } {
  let primary = ''
  const secondaryParts: string[] = []

  // An address reads as its street and number; anything named reads as its name (Nantes station
  // read as its street, "Voies 10-11").
  if (p.street && p.housenumber) {
    const slash = p.housenumber.indexOf('/')
    primary = `${p.street} ${slash === -1 ? p.housenumber : p.housenumber.substring(slash + 1)}`
  } else {
    primary = p.name || p.street || ''
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

/** Shared Photon call — one UA + timeout for /api/search and /api/reverse. The public Photon
 *  answered in 5.2-6.6 s on 2026-10-07, so a 3 s limit returned no place at all. */
function fetchPhoton(url: URL): Promise<Response> {
  return fetch(url.toString(), {
    headers: { 'User-Agent': 'quietmap.org/1.0 (noise atlas; contact: info@quietmap.org)' },
    signal: AbortSignal.timeout(10_000),
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

export async function searchRoutes(app: FastifyInstance, { photonUrl }: { photonUrl: string }) {
  // Both geocode routes proxy the Photon geocoder (QM_PHOTON_URL, the public one unless set) —
  // rate-limited per client (owner directive 2026-07-15) to protect Photon etiquette and us.
  // The map's view biases the order, as strongly as the view is close: zoomed out over Prague,
  // "london" is London; without the zoom Photon took a street's radius and listed Prague's bars.
  app.get<{ Querystring: { q?: string; lat?: string; lon?: string; zoom?: string } }>('/api/search', {
    config: { rateLimit: EXPENSIVE_ROUTE_RATE_LIMIT },
  }, async (request, reply) => {
    const q = request.query.q?.trim()
    if (!q || q.length < 2) return reply.send([])

    const lat = parseFloat(request.query.lat ?? '')
    const lon = parseFloat(request.query.lon ?? '')
    const zoom = Math.round(parseFloat(request.query.zoom ?? ''))
    const view = Number.isFinite(lat) && Math.abs(lat) <= 90 && Number.isFinite(lon) &&
      Math.abs(lon) <= 180 && Number.isFinite(zoom)
      ? { lat: lat.toFixed(1), lon: lon.toFixed(1), zoom: String(Math.min(18, Math.max(0, zoom))) }
      : null
    const cacheKey = `${q.toLowerCase()}|${view ? `${view.lat}|${view.lon}|${view.zoom}` : ''}`

    const entry = cache.get(cacheKey)
    if (entry && entry.expires > Date.now()) return reply.send(entry.data)

    try {
      const url = new URL('/api/', photonUrl)
      url.searchParams.set('q', q)
      if (view) {
        url.searchParams.set('lat', view.lat)
        url.searchParams.set('lon', view.lon)
        url.searchParams.set('zoom', view.zoom)
      }
      url.searchParams.set('lang', 'default')
      url.searchParams.set('limit', String(ASKED))

      const res = await fetchPhoton(url)

      if (!res.ok) return reply.send([])

      const data = await res.json() as { features: PhotonFeature[] }
      const results: SearchResult[] = []
      for (const f of data.features) {
        if (!searchable(f.properties)) continue
        const { display_name, secondary } = formatPhotonResult(f.properties)
        const result = { display_name, secondary, lat: f.geometry.coordinates[1], lon: f.geometry.coordinates[0] }
        if (results.some(shown => samePlace(shown, result))) continue
        results.push(result)
        if (results.length === SHOWN) break
      }

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
      const url = new URL('/reverse', photonUrl)
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
