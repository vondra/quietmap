// Filter quoted stays by exact nightly price and the union of the router's reachable polygons.
import type { Stay, ViewBox } from './stays.ts'

/** A ring is local (at most two hours' travel). Unwrap around its first vertex so the same
 *  polygon works across the antimeridian and in repeated map worlds. Edges count as inside. */
function insideRing(lng: number, lat: number, ring: GeoJSON.Position[]): boolean {
  if (ring.length < 3) return false
  const origin = ring[0][0]
  const unwrap = (x: number) => x + 360 * Math.round((origin - x) / 360)
  const x = unwrap(lng)
  let inside = false
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = [unwrap(ring[i][0]), ring[i][1]]
    const [xj, yj] = [unwrap(ring[j][0]), ring[j][1]]
    if ((x - xi) * (yj - yi) === (lat - yi) * (xj - xi)
      && x >= Math.min(xi, xj) && x <= Math.max(xi, xj) && lat >= Math.min(yi, yj) && lat <= Math.max(yi, yj)) return true
    if ((yi > lat) !== (yj > lat) && x < (xj - xi) * (lat - yi) / (yj - yi) + xi) inside = !inside
  }
  return inside
}

/** Any selected travel mode may reach the stay; islands count, holes do not. */
export function insideReach(stay: Pick<Stay, 'lat' | 'lng'>, area: GeoJSON.FeatureCollection): boolean {
  const polygonContains = (rings: GeoJSON.Position[][]) => rings.length > 0
    && insideRing(stay.lng, stay.lat, rings[0])
    && !rings.slice(1).some(ring => insideRing(stay.lng, stay.lat, ring))
  return area.features.some(({ geometry }) => geometry?.type === 'Polygon' ? polygonContains(geometry.coordinates)
    : geometry?.type === 'MultiPolygon' && geometry.coordinates.some(polygonContains))
}

/** Compare the quote before display rounding; an unknown price cannot meet a price ceiling. */
export function belowPrice(stay: Stay, maxPrice: number | null): boolean {
  return maxPrice == null || (stay.total != null && stay.currency === 'EUR' && stay.total / stay.nights <= maxPrice)
}

/** A pin counts in the world copy nearest the view, which may run past ±180°. */
export function stayInView(stay: Pick<Stay, 'lat' | 'lng'>, view: ViewBox): boolean {
  const lng = stay.lng + 360 * Math.round(((view.west + view.east) / 2 - stay.lng) / 360)
  return stay.lat >= view.south && stay.lat <= view.north && lng >= view.west && lng <= view.east
}
