// Map coordinates shared by the search field and the server's search, popup and place routes.
export interface SearchResult {
  display_name: string
  secondary: string
  lat: number
  lon: number
}

/** The popup's Web Mercator latitude limit, rounded inwards. */
export const MAX_LATITUDE = 85.05
const DECIMAL = /^[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?$/

/** A query's decimal number, or null for anything else (`Number` alone reads '' as 0). */
export function coordinate(text: unknown): number | null {
  if (typeof text !== 'string' || !DECIMAL.test(text)) return null
  const value = Number(text)
  return Number.isFinite(value) ? value : null
}

/** Keep in-range longitudes exact: a rounding error can change the popup's sampling seed. */
export function wrappedLongitude(lon: number): number {
  return lon >= -180 && lon < 180 ? lon : ((((lon + 180) % 360) + 360) % 360) - 180
}

/** A pasted latitude, longitude has one exact result, or none outside the map. Only text goes to the geocoder. */
export function coordinateSuggestions(query: string): SearchResult[] | null {
  const parts = query.split(',')
  if (parts.length !== 2) return null
  const decimals = parts.map(part => part.trim().replace(/^\u2212/, '-'))
  if (!decimals.every(part => DECIMAL.test(part))) return null
  const [lat, lon] = decimals.map(Number)
  if (Math.abs(lat) > MAX_LATITUDE || Math.abs(lon) > 180) return []
  return [{ lat, lon, display_name: `${lat}, ${lon}`, secondary: '' }]
}
