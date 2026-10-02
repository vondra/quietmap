// The streamed popup contract of `/api/popup`: every line is one PopupUpdate (the whole answer so
// far, replacing the previous line) or one PopupError. Levels are dB rounded to 0.1; `null` is
// silence (no modelled source reaches the point in that period).

export type Level = number | null

/** Day (07-19), evening (19-23), night (23-07) and the day-evening-night level. */
export interface PeriodLevels {
  ld: Level
  le: Level
  ln: Level
  lden: Level
}

/** One layer's levels at the point. */
export interface LayerLevels extends PeriodLevels {
  source_type: string
  /** Lden if everything the stop rule left out (so far) were as loud as its bound. */
  lden_upper: Level
  /** Sources computed in full, of the candidates the rings read so far. */
  evaluated: number
  candidates: number
}

/** A contributor's display record: the flat display fields of its layer, as the tile builder
 *  wrote them. Layers and fields keep being added, so every field is read defensively. */
export type ContributorMetadata = Record<string, unknown>

/** One contributor: sources sharing a name and class (a street, a railway line, a site). */
export interface Contributor {
  /** Stable across the streamed updates of one click. */
  id: string
  source_type: string
  /** Name, else reference, else class. */
  name: string
  /** Road class; null for the other layers. */
  subtype: string | null
  distance_m: number
  received_lden: Level
  received: PeriodLevels
  metadata: ContributorMetadata | null
  /** What the map draws of it, lines of [lat, lon] (one point for a point source): all of it within
   *  the reach in the final update, its loudest pieces before. */
  geometry?: [number, number][][]
}

/** The façade receiver a building click is answered at. */
export interface FacadeReceiver {
  /** The façade's outward direction, degrees clockwise from north. */
  bearing_deg: number
  /** Canonical index among the building's exposed receivers. */
  index: number
  /** [lat, lon] 0.1 m in front of the wall. */
  receiver: [number, number]
}

/** A click inside a building answers for its loudest façade receiver. The answer keeps gaining
 *  fields; the popup lists the ones it has no words for by name. */
export interface BuildingAnswer {
  id: string
  height_m: number
  /** The loudest façade receiver; null when no façade is exposed (the building is not assessed). */
  facade?: FacadeReceiver | null
  /** Exposed façade receivers compared. */
  facade_receivers?: number
  [field: string]: unknown
}

/** A piece of a flight's track: its two ends as [lat, lon, altitude above sea level in m]. */
export type TrackPiece = [[number, number, number], [number, number, number]]

/** One of the loudest flights at the point: an ADS-B flight of the aircraft layer. */
export interface TopFlight {
  /** ICAO 24-bit address, six lowercase hex digits. */
  icao: string
  /** ATC callsign; empty when the flight sent none. */
  callsign: string
  /** ICAO type designator ("A320"); empty when unknown. */
  type: string
  /** The flight's start, Unix seconds UTC. */
  start_unix: number
  /** The period the flight passed the point in. */
  period: 'day' | 'evening' | 'night'
  /** Sound exposure level at the point of the parts of the flight the popup computed. */
  sel_db: number
  /** The flight's peak level (LAmax) at the point: the loudest of its parts, each at its closest point;
   *  the list's order. */
  lmax_db: number
  /** Horizontal distance to where that peak is reached (the closest point of the loudest part). */
  closest_m: number
  /** Height of that point above the receiver. */
  altitude_m: number
  /** The parts of the flight the popup computed near the point, in the order computed: not along
   *  the flight, and not necessarily contiguous. */
  track: TrackPiece[]
}

/** The levels exceeded 5, 10, 50 and 90 % of the time, per period (null where silent). */
export interface PopupPercentiles {
  l5?: { day: number | null; evening: number | null; night: number | null }
  l10: { day: number | null; evening: number | null; night: number | null }
  l50: { day: number | null; evening: number | null; night: number | null }
  l90: { day: number | null; evening: number | null; night: number | null }
  /** The share of each period (%, 0.1 precision) the sources together stand above a quiet natural
   *  background: how much of the time human noise is heard. */
  audible_percent?: { day: number; evening: number; night: number }
}

/** How loud the place sounds: Zwicker's loudness (ISO 532-1) of the sound exceeded 5 % of each
 * period, N5 in sone (twice the sone, twice as loud). */
export interface PopupLoudness {
  n5_sone: { day: number | null; evening: number | null; night: number | null }
}

/** What the click read and computed so far. */
export interface PopupStats {
  rings: number
  files: number
  bytes: number
  read_ms: number
  candidate_ms: number
  evaluate_ms: number
  elapsed_ms: number
}

/** A listed piece's ray: the ground under it from the source (distance m, altitude m, G), the
 *  source and receiver altitudes, and its terms (dB, A-weighted over the source's day spectrum;
 *  pairs are homogeneous, favourable). */
export interface PieceTrace {
  profile: [number, number, number][]
  source_altitude_m: number
  receiver_altitude_m: number
  slant_m: number
  /** Share of favourable (downward refracting) propagation by day, evening and night. */
  p: [number, number, number]
  /** Ground and screening together. */
  boundary_db: [number, number]
  /** Screening alone, without the ground. */
  without_ground_db: [number, number]
  air_db: number
  path_difference_m: [number, number]
  /** The ray on the map: [lat, lon] of the piece's closest point, then of the receiver. */
  ray?: [[number, number], [number, number]]
}

/** The segments view's pieces on the map: each listed piece's ends and its ray in the colour of
 *  its row (the selected piece's marked), and the point the level is computed at. */
export interface SegmentFan {
  receiver: [number, number]
  pieces: { ends: [number, number][], ray: [[number, number], [number, number]], color: string, selected: boolean }[]
}

/** One computed piece of the segments view (asked with `segments=1`). */
export interface PopupPiece {
  source_type: string
  /** Its contributor group's id. */
  id: string
  /** [lat, lon] ends; one for a point source. */
  ends: [number, number][]
  distance_m: number
  /** A-weighted emission (per metre of a line). */
  emission: PeriodLevels
  received: PeriodLevels
  metadata: ContributorMetadata | null
  /** Buildings and walls the ray crosses: distance from the receiver (m), height (m), id. */
  crossings: [number, number, string][]
  trace: PieceTrace | null
}

export interface PopupUpdate {
  seq: number
  /** True until every ring within the reach is read: the numbers shown are valid, but still refined. */
  partial: boolean
  center: [number, number]
  elevation_m: number
  /** The receiver reflection bonus of the surroundings (dB: 0, 1.5 or 3). */
  reflection_db?: number
  building: BuildingAnswer | null
  total_lden: Level
  total: PeriodLevels
  sources: LayerLevels[]
  top_contributors: Contributor[]
  /** The loudest flights by Lmax, loudest first; empty when no aircraft are heard. */
  top_flights: TopFlight[]
  /** With the final update only. */
  percentiles?: PopupPercentiles | null
  loudness?: PopupLoudness | null
  /** The segments view's pieces, when asked for. */
  pieces?: PopupPiece[]
  stats: PopupStats
}

/** The last line of a failed click: the answer is incomplete and must not be shown as final. */
export interface PopupError {
  error: string
}
