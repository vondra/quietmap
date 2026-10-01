// The popup answers the browser tests stream: one update of the contract, a street, Prague airport's
// ground operations, and the loudest flights with their tracks near the hermetic world's point.
import type { Contributor, PopupUpdate, TopFlight, TrackPiece } from '../src/types/noise'
import { POINT } from './support'

/** Quieter than the street: the aircraft row ranks below it. */
const AIRCRAFT_DB = 55

/** One road contributor at `db`. */
export function roadContributor(db: number): Contributor {
  return {
    id: '00000000000000aa',
    source_type: 'road',
    name: 'Fixture street',
    subtype: 'tertiary',
    distance_m: 12,
    received_lden: db,
    received: { ld: db - 2, le: db - 3, ln: db - 8, lden: db },
    metadata: {
      name: 'Fixture street', ref: '', road_class: 'tertiary',
      aadt_light: 9000, aadt_medium: 300, aadt_heavy: 200, aadt_moto: 50, traffic_estimated: 8,
      cross_section_aadt: 9550, speed_posted_kmh: 50, speed_kmh: 50, speed_source: 'osm_posted',
      surface: 'asphalt', surface_corr_db: 0, lanes: 2, oneway: false, bridge: false, source_id: 1,
    },
  }
}

/** Prague airport's ground operations at `db`: a contributor of the aircraft layer. */
export function airportContributor(db: number): Contributor {
  return {
    id: '00000000000000bb',
    source_type: 'aircraft',
    name: 'LKPR ground operations',
    subtype: 'airport_traffic:LKPR',
    distance_m: 2400,
    received_lden: db,
    received: { ld: db - 1, le: db - 2, ln: db - 7, lden: db },
    metadata: {
      name: 'LKPR ground operations', subtype: 'airport_traffic:LKPR', airport: 'Letiště Václava Havla Praha',
      arrivals_per_day: 180.4, departures_per_day: 181.2, ground_vehicles_per_day: 36.5,
    },
  }
}

/** One streamed update of the popup contract. */
export function popupUpdate(
  seq: number,
  partial: boolean,
  lat: number,
  lng: number,
  db: number | null,
  building: PopupUpdate['building'] = null,
): PopupUpdate {
  const levels = { ld: db, le: db, ln: db, lden: db }
  return {
    seq,
    partial,
    center: [lat, lng],
    elevation_m: 350,
    building,
    total_lden: db,
    total: levels,
    sources: [{ source_type: 'road', ...levels, lden_upper: db, evaluated: 1, candidates: 1 }],
    top_contributors: db == null ? [] : [roadContributor(db)],
    top_flights: [],
    // The final answer carries the loudness line.
    loudness: partial || db == null ? null : { n5_sone: { day: 12, evening: 11, night: 7.4 } },
    stats: { rings: seq, files: 27, bytes: 1e6, read_ms: 3, candidate_ms: 4, evaluate_ms: 20, elapsed_ms: 30 },
  }
}

/** A track piece between two places given in degrees north and east of the point. */
function trackPiece([north0, east0]: [number, number], [north1, east1]: [number, number], altitude_m: number): TrackPiece {
  return [[POINT.lat + north0, POINT.lng + east0, altitude_m], [POINT.lat + north1, POINT.lng + east1, altitude_m]]
}

/** The loudest flights, loudest first: an A320 by day, and a helicopter with a long type name at
 *  night. Their tracks lie north of the point, where a phone's sheet leaves the map visible: the
 *  A320's three pieces run east-west about 150 px north at the tile zoom (the piece between the two
 *  others computed last), the helicopter's one north-south about 145 px west. */
export const FIXTURE_FLIGHTS: TopFlight[] = [
  {
    icao: '4b0a1c', callsign: 'CSA123', type: 'A320', start_unix: Date.UTC(2025, 8, 2, 14, 26, 40) / 1000,
    period: 'day', sel_db: 79.1, lmax_db: 70.2, closest_m: 444, altitude_m: 255,
    track: [
      trackPiece([0.0166, 0.005], [0.0166, 0.02], 610),
      trackPiece([0.0166, -0.02], [0.0166, -0.005], 600),
      trackPiece([0.0166, -0.005], [0.0166, 0.005], 605),
    ],
  },
  {
    icao: '49d3e1', callsign: 'HELI42', type: 'AS55', start_unix: Date.UTC(2025, 8, 1, 23, 58, 20) / 1000,
    period: 'night', sel_db: 77.4, lmax_db: 68.9, closest_m: 1310, altitude_m: 610,
    track: [trackPiece([0.01, -0.025], [0.025, -0.025], 960)],
  },
]

/** `update` with the aircraft layer audible at AIRCRAFT_DB and its loudest `flights`. */
export function withAircraft(update: PopupUpdate, flights: TopFlight[]): PopupUpdate {
  const db = AIRCRAFT_DB
  return {
    ...update,
    sources: [
      ...update.sources,
      { source_type: 'aircraft', ld: db - 1, le: db - 4, ln: db - 9, lden: db, lden_upper: db, evaluated: 40, candidates: 40 },
    ],
    top_flights: flights,
  }
}
