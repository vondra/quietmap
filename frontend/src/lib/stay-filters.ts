// Filter quoted stays by exact nightly price and count them in the current map view.
import type { Stay, ViewBox } from './stays.ts'

/** Compare the quote before display rounding; an unknown price cannot meet a price ceiling. */
export function belowPrice(stay: Stay, maxPrice: number | null): boolean {
  return maxPrice == null || (stay.total != null && stay.currency === 'EUR' && stay.total / stay.nights <= maxPrice)
}

/** A pin counts in the world copy nearest the view, which may run past ±180°. */
export function stayInView(stay: Pick<Stay, 'lat' | 'lng'>, view: ViewBox): boolean {
  const lng = stay.lng + 360 * Math.round(((view.west + view.east) / 2 - stay.lng) / 360)
  return stay.lat >= view.south && stay.lat <= view.north && lng >= view.west && lng <= view.east
}
