// The popup answers the browser tests stream: one update of the contract, a street and a piece of
// it, Prague airport's ground operations, and the loudest flights with their tracks near the
// hermetic world's point.
import type { Contributor, PopupPiece, PopupUpdate, TopFlight, TrackLine } from '../src/types/noise'
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
    // The final answer carries each row's share of the loudness: the street three quarters, the
    // aircraft row (when there is one) the rest.
    top_contributors: db == null ? [] : [{ ...roadContributor(db), ...(partial ? {} : { share: 0.75 }) }],
    rest_share: partial ? undefined : 0,
    top_flights: [],
    // The final answer carries the loudness and the time levels.
    loudness: partial || db == null ? null : { mean_sone: { day: 12, evening: 11, night: 7.4 }, nden_sone: 15 },
    percentiles: partial || db == null ? null : {
      l5: { day: db + 4, evening: db + 3, night: db - 3 },
      l10: { day: db + 3, evening: db + 2, night: db - 4 },
      l50: { day: db - 2, evening: db - 3, night: db - 9 },
      l90: { day: db - 6, evening: db - 7, night: db - 15 },
    },
    stats: { rings: seq, files: 27, bytes: 1e6, read_ms: 3, candidate_ms: 4, evaluate_ms: 20, elapsed_ms: 30 },
  }
}

/** A computed piece of `contributor` delivering `lden`, a 45 m line `distance_m` east of the point:
 *  four rays, the southernmost behind a building that takes 20 dB in calm air and 8.5 downwind. */
export function computedPiece(contributor: Contributor, lden: number, distance_m: number): PopupPiece {
  const east = distance_m / (111_320 * Math.cos((POINT.lat * Math.PI) / 180))
  const nearest: [number, number] = [POINT.lat, POINT.lng + east]
  return {
    source_type: contributor.source_type,
    id: contributor.id,
    ends: [[POINT.lat - 0.0002, POINT.lng + east], [POINT.lat + 0.0002, POINT.lng + east]],
    distance_m,
    emission: { ld: 80, le: 78, ln: 72, lden: 82 },
    received: { ld: lden - 2, le: lden - 3, ln: lden - 8, lden },
    path: {
      free_lden: lden + 1.1, air_db: -0.1, screening_db: [-1.6, -0.6], ground_db: [0.2, 0.3],
      facades_db: 0, lden: [lden - 0.4, lden + 0.6], bent_percent: [40, 55, 65],
    },
    metadata: contributor.metadata,
    crossings: [[distance_m / 2, 9, '011400ad000668eb']],
    // Terms as losses: ground and screening calm and downwind, screening alone calm and downwind,
    // air; slant m.
    rays: [
      [POINT.lat - 0.00015, POINT.lng + east, 0.3, lden - 30.5, [21.5, 8.2, 20, 8.5, 0.1, 22]],
      [POINT.lat - 0.00005, POINT.lng + east, 0.6, lden - 4, [-0.4, -0.5, 0, 0, 0.1, 13]],
      [POINT.lat + 0.00005, POINT.lng + east, 0.6, lden - 4, [-0.4, -0.5, 0, 0, 0.1, 13]],
      [POINT.lat + 0.00015, POINT.lng + east, 0.3, lden - 7, [-0.3, -0.4, 0, 0, 0.1, 22]],
    ],
    trace: {
      profile: [[0, 350, 0], [distance_m, 350, 0]],
      source_altitude_m: 350,
      receiver_altitude_m: 354,
      slant_m: distance_m + 0.4,
      p: [0.55, 0.8, 0.9],
      boundary_db: [-0.4, -0.5],
      without_ground_db: [0, 0],
      air_db: 0.1,
      path_difference_m: [0, 0],
      ray: [nearest, [POINT.lat, POINT.lng]],
    },
  }
}

/** The answer of `piece`'s own run (`piece=`): that piece alone, every ray with the flat ground
 *  under it, hard then soft, and the southernmost ray with the two walls of the building it
 *  crosses. */
export function pieceRun(update: PopupUpdate, piece: PopupPiece): PopupUpdate {
  const rays = piece.rays.map(([lat, lon, angle, lden, terms], k) => [lat, lon, angle, lden, terms, {
    ground: [[0, 350, 0], [terms[5] / 2, 350, 0], [terms[5], 350, 1]],
    source_altitude_m: 350,
    walls: k === 0 ? [[terms[5] / 2 - 3, 9, true], [terms[5] / 2 + 3, 9, true]] : [],
  }] as PopupPiece['rays'][number])
  return { ...update, pieces: [{ ...piece, rays }] }
}

/** A track line between two places given in degrees north and east of the point. */
function trackLine([north0, east0]: [number, number], [north1, east1]: [number, number]): TrackLine {
  return [[POINT.lat + north0, POINT.lng + east0], [POINT.lat + north1, POINT.lng + east1]]
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
      trackLine([0.0166, 0.005], [0.0166, 0.02]),
      trackLine([0.0166, -0.02], [0.0166, -0.005]),
      trackLine([0.0166, -0.005], [0.0166, 0.005]),
    ],
  },
  {
    icao: '49d3e1', callsign: 'HELI42', type: 'AS55', start_unix: Date.UTC(2025, 8, 1, 23, 58, 20) / 1000,
    period: 'night', sel_db: 77.4, lmax_db: 68.9, closest_m: 1310, altitude_m: 610,
    track: [trackLine([0.01, -0.025], [0.025, -0.025])],
  },
]

/** `update` with the aircraft layer audible at AIRCRAFT_DB and its loudest `flights`. */
export function withAircraft(update: PopupUpdate, flights: TopFlight[]): PopupUpdate {
  const db = AIRCRAFT_DB
  return {
    ...update,
    sources: [
      ...update.sources,
      {
        source_type: 'aircraft', ld: db - 1, le: db - 4, ln: db - 9, lden: db, lden_upper: db, evaluated: 40, candidates: 40,
        ...(update.partial ? {} : { share: 0.25 }),
      },
    ],
    top_flights: flights,
  }
}
