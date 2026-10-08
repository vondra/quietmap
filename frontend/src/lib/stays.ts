// Places to stay: the stay a visitor searches (dates, guests, which places), the server's places
// for a map view (`/api/stay`), the pins kept while the map moves and until their prices expire, and
// their prices as the map and the popup print them. Pure TypeScript, so it has dependency-free unit
// tests.

/** Which places: all, hotels, or the rest (apartments, guest houses, hostels…). */
export type StayKind = 'all' | 'hotel' | 'rental'

export interface StaySearch {
  kind: StayKind
  /** YYYY-MM-DD. */
  checkin: string
  checkout: string
  adults: number
  minStars: number | null
  /** The lowest guests' score, out of 10. */
  minScore: number | null
}

/** A place to stay with a room for the search it was found by (the server's listing). */
export interface Stay {
  id: string
  name: string
  lat: number
  lng: number
  /** The cheapest supplier's price of the whole stay; null when no supplier quoted one. */
  total: number | null
  stars: number | null
  score: number | null
  reviews: number | null
  guests: number | null
  bedrooms: number | null
  freeCancellation: boolean
  thumbnail: string | null
  url: string
  /** The stay the price is for. */
  nights: number
  currency: string
  /** When the place and its price go, by the page's clock: Stay22's lifetime of the answer. */
  expires: number
}

/** A map view: its box and its zoom. */
export interface ViewBox {
  west: number
  south: number
  east: number
  north: number
  zoom: number
}

/** The map layer of the places' dots, beneath which the heatmap and the data layers draw. */
export const STAY_DOT_LAYER = 'stays-dot'
/** Below this zoom the view is wider than the server answers (16° on a 2,900-pixel screen). */
export const STAY_MIN_ZOOM = 7
/** The most pins kept; the oldest go first. */
export const MAX_PINS = 1500
const DAY_MS = 86_400_000

const pad = (value: number) => String(value).padStart(2, '0')

/** The date `days` after a YYYY-MM-DD date. */
export function addDays(date: string, days: number): string {
  return new Date(Date.parse(date) + days * DAY_MS).toISOString().slice(0, 10)
}

export function nightsBetween(checkin: string, checkout: string): number {
  return Math.round((Date.parse(checkout) - Date.parse(checkin)) / DAY_MS)
}

/** The first check-in Stay22 takes: from its today on, the UTC day, which late in a day west of UTC
 *  is the visitor's tomorrow. */
export function firstCheckin(now = new Date()): string {
  const local = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
  const utc = now.toISOString().slice(0, 10)
  return local > utc ? local : utc
}

const mercatorY = (lat: number) => Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360))
const latitudeAt = (y: number) => (360 / Math.PI) * Math.atan(Math.exp(y)) - 90

/** The box a map shows as if it looked straight down (a tilted map's bounds run to its horizon): its
 *  canvas turned by the bearing, around the centre taken in the world copy -180..180. */
export function viewFootprint(center: { lng: number; lat: number }, zoom: number, bearing: number, width: number, height: number): ViewBox {
  const turn = (bearing * Math.PI) / 180
  const radiansPerPixel = (2 * Math.PI) / (512 * 2 ** zoom)
  const halfX = ((Math.abs(width * Math.cos(turn)) + Math.abs(height * Math.sin(turn))) / 2) * radiansPerPixel
  const halfY = ((Math.abs(width * Math.sin(turn)) + Math.abs(height * Math.cos(turn))) / 2) * radiansPerPixel
  const lng = center.lng - 360 * Math.round(center.lng / 360)
  const y = mercatorY(center.lat)
  return {
    west: lng - (halfX * 180) / Math.PI,
    east: lng + (halfX * 180) / Math.PI,
    south: latitudeAt(y - halfY),
    north: latitudeAt(y + halfY),
    zoom,
  }
}

/** The search a visitor starts from, as booking sites start: from today for two nights, two
 *  guests, every place. */
export function defaultStaySearch(now = new Date()): StaySearch {
  const checkin = firstCheckin(now)
  return { kind: 'all', checkin, checkout: addDays(checkin, 2), adults: 2, minStars: null, minScore: null }
}

/** The search moved to a new check-in, keeping the stay's length, as booking sites do. */
export function withCheckin(search: StaySearch, checkin: string): StaySearch {
  return { ...search, checkin, checkout: addDays(checkin, nightsBetween(search.checkin, search.checkout)) }
}

/** The search with a new check-out, at least a night after the check-in. */
export function withCheckout(search: StaySearch, checkout: string): StaySearch {
  return { ...search, checkout: checkout > search.checkin ? checkout : addDays(search.checkin, 1) }
}

/** The search still possible at `now`: a check-in Stay22 no longer takes (a tab open past midnight)
 *  moved to its first, the stay's length kept; else the search itself. */
export function freshSearch(search: StaySearch, now = new Date()): StaySearch {
  const first = firstCheckin(now)
  return search.checkin < first ? withCheckin(search, first) : search
}

/** The server's places for a map view (centred in the world copy -180..180): its box, or two where it
 *  crosses the antimeridian, a side each, as the server takes boxes within ±180°. */
export function stayRequests(view: Omit<ViewBox, 'zoom'>, search: StaySearch): string[] {
  const sides = view.west < -180 ? [[view.west + 360, 180], [-180, view.east]]
    : view.east > 180 ? [[view.west, 180], [-180, view.east - 360]]
      : [[view.west, view.east]]
  const degrees = (value: number) => String(+value.toFixed(6))
  return sides.map(([west, east]) => {
    const params = new URLSearchParams({
      swlat: degrees(view.south),
      swlng: degrees(west),
      nelat: degrees(view.north),
      nelng: degrees(east),
      checkin: search.checkin,
      checkout: search.checkout,
      adults: String(search.adults),
    })
    if (search.kind !== 'all') params.set('type', search.kind)
    if (search.minStars != null) params.set('minstars', String(search.minStars))
    if (search.minScore != null) params.set('minscore', String(search.minScore))
    return `/api/stay?${params}`
  })
}

const priceFormats = new Map<string, Intl.NumberFormat>()

/** A price in whole units of its currency: €1,234. */
export function formatPrice(amount: number, currency: string): string {
  let format = priceFormats.get(currency)
  if (!format) priceFormats.set(currency, format = new Intl.NumberFormat('en', { style: 'currency', currency, maximumFractionDigits: 0 }))
  return format.format(amount)
}

/** The price of a night, or null when no supplier quoted one. */
export function pricePerNight(stay: Stay): number | null {
  return stay.total == null ? null : Math.round(stay.total / stay.nights)
}

/** The pins after an answer at `now`: the earlier and the new, a place once with its newest price,
 *  none past its expiry, the oldest dropped beyond MAX_PINS. The server samples every view anew; the
 *  pins of a wider view stay when zooming in (the owner's report of 2026-07-29: a place seen in a
 *  street went on zooming). */
export function withStays(pins: Stay[], answer: Stay[], now = Date.now()): Stay[] {
  const byId = new Map(pins.map(stay => [stay.id, stay]))
  for (const stay of answer) {
    byId.delete(stay.id)
    byId.set(stay.id, stay)
  }
  return [...byId.values()].filter(stay => stay.expires > now).slice(-MAX_PINS)
}

/** The pins as the map draws them: a point each, with its price of a night and its reviews, by
 *  which the labels that collide give way, and the place itself, which a click opens. */
export function stayFeatures(pins: Stay[]): GeoJSON.FeatureCollection<GeoJSON.Point> {
  return {
    type: 'FeatureCollection',
    features: pins.map(stay => {
      const night = pricePerNight(stay)
      const price = night == null ? '' : formatPrice(night, stay.currency)
      return {
        type: 'Feature',
        geometry: { type: 'Point', coordinates: [stay.lng, stay.lat] },
        properties: { price, reviews: stay.reviews ?? 0, stay: JSON.stringify(stay) },
      }
    }),
  }
}

/** The place a map feature of the pins draws. */
export function stayOfFeature(feature: { properties: Record<string, unknown> }): Stay {
  return JSON.parse(String(feature.properties.stay)) as Stay
}
