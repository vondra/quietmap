/** Search flight target: the popup opens only when the map really arrived there. */

export const SEARCH_FLIGHT_ZOOM = 14

// ~0.1 m: flyTo lands on the target within float error; an interrupted flight
// (a hash jump, a drag) is metres or more away.
const ARRIVAL_TOLERANCE_DEG = 1e-6

/**
 * MapLibre fires `moveend` for an interrupted flight too (`stop()` →
 * `_afterEase`, same event data), so the event alone cannot tell arrival from
 * a hash navigation during the 1.5 s flight — which then reopened the popup at
 * the stale search result over the link's own `d=` (review 2026-09-10).
 */
export function searchFlightArrived(
  view: { lat: number; lng: number; zoom: number },
  target: { lat: number; lon: number },
): boolean {
  return Math.abs(view.lat - target.lat) < ARRIVAL_TOLERANCE_DEG
    && Math.abs(view.lng - target.lon) < ARRIVAL_TOLERANCE_DEG
    && Math.abs(view.zoom - SEARCH_FLIGHT_ZOOM) < ARRIVAL_TOLERANCE_DEG
}
