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

/** The kinds of the aircraft layer: Doc 29's engine installations, helicopters, and the airport's
 *  taxiing and take-off rolls. */
export type AircraftKind = 'airliners' | 'regional_business_jets' | 'propeller' | 'helicopters' | 'ground'

/** What flies over the point: per band of maximum level (dB, at or above) the flights of an
 *  average day and those at night (23-07), their mean height above the ground there and the type
 *  flying most of them (null without flights); the helicopters a day above the lowest band. Each
 *  flight of the year counts once, at its loudest moment, outdoors in the open. */
export interface AircraftEvents {
  above_db: number[]
  per_day: number[]
  night: number[]
  height_m: (number | null)[]
  type: (string | null)[]
  helicopters_per_day: number
}

/** One layer's levels at the point. */
export interface LayerLevels extends PeriodLevels {
  source_type: string
  /** The aircraft layer alone, its flights and its airports' ground operations: its Nden in sone
   *  (the final update's). */
  nden_sone?: number
  /** The aircraft layer's share of the place's loudness, a fraction (the final update's). */
  share?: number
  /** What the popup leaves out of a ground layer: its levels, and how many contributors they are. */
  unlisted?: PeriodLevels
  unlisted_sources?: number
  /** What the aircraft layer is made of: each kind's share of its Lden energy, those of 0.5 % or
   *  more (the final update's). */
  kinds?: Partial<Record<AircraftKind, number>>
  /** What flies over the point (the aircraft layer, from the first update on). */
  events?: AircraftEvents
  /** Lden if everything the stop rule left out (so far) were as loud as its bound. */
  lden_upper: Level
  /** Sources computed in full, of the candidates the rings read so far. */
  evaluated: number
  candidates: number
}

/** A contributor's display record: the flat display fields of its layer, as the tile builder
 *  wrote them. Layers and fields keep being added, so every field is read defensively. */
export type ContributorMetadata = Record<string, unknown>

/** How a contributor's sound reaches the point, summed over its rays (Lden): its level over the
 *  distance with nothing in the way (the façades apart), what the air takes (dB), the screening and
 *  the ground in calm air and bent down by the weather, the façades' reflection, the level here in
 *  either state, and the percent of the day, evening and night the weather bends it down. */
export interface SourcePath {
  free_lden: number | null
  air_db: number | null
  screening_db: [number | null, number | null]
  ground_db: [number | null, number | null]
  facades_db: number
  lden: [number | null, number | null]
  bent_percent: [number | null, number | null, number | null]
}

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
  /** It alone: its Nden in sone (the final update's), as the list ranks it. */
  nden_sone?: number
  /** Its share of the place's loudness, a fraction (the final update's): every moment's loudness
   *  shared by the sources' energy then, so the rows add up to the whole. */
  share?: number
  path?: SourcePath
  metadata: ContributorMetadata | null
  /** What the map draws of it, lines of [lat, lon] (one point for a point source): all of it within
   *  the reach in the final update, its loudest pieces before. */
  geometry?: [number, number][][]
  /** How it is heard (final update; none for a steady source): its passes per hour by day,
   *  evening and night, and whether at its distance they run together into a steady sound. */
  heard?: { per_hour: { day: number; evening: number; night: number }; steady: boolean }
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

/** A line of a flight's track: its points as [lat, lon]. */
export type TrackLine = [number, number][]

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
  /** The flight's line within 20 km of the point, along the flight (more than one where it leaves
   *  the area and comes back); the final answer's only. */
  track: TrackLine[]
}

/** The levels exceeded 5, 10, 50 and 90 % of the time, per period (null where silent). */
export interface PopupPercentiles {
  l5?: { day: number | null; evening: number | null; night: number | null }
  l10: { day: number | null; evening: number | null; night: number | null }
  l50: { day: number | null; evening: number | null; night: number | null }
  l90: { day: number | null; evening: number | null; night: number | null }
}

/** How loud the place sounds: Zwicker's loudness (ISO 532-1, sone: twice the sone, twice as loud)
 *  of every moment, averaged over each period as it sounds and over the whole day as Nden (the
 *  evening counted 5 dB and the night 10 dB louder, as Lden counts them, the periods by their
 *  hours). */
export interface PopupLoudness {
  mean_sone: { day: number | null; evening: number | null; night: number | null }
  nden_sone: number | null
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

/** The segments view's pieces on the map: each listed piece's ends and its nearest ray in the
 *  colour of its row (the selected piece's marked), every ray the selected piece was summed over
 *  in the colour of what reaches the receiver along it, and the point the level is computed at. */
export interface SegmentFan {
  receiver: [number, number]
  pieces: { ends: [number, number][], ray: [[number, number], [number, number]], color: string, selected: boolean }[]
  rays: { from: [number, number], color: string }[]
  /** What the map shows when the pieces first appear: the pieces that make the level. */
  overview: [number, number][]
  /** The opened piece's ends and rays, which the map frames as it opens. */
  opened: { index: number, points: [number, number][] } | null
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
  /** Its whole source's Lden: every piece of it, listed or not. */
  source_lden: number | null
  metadata: ContributorMetadata | null
  /** Buildings and walls the ray crosses: distance from the receiver (m), height (m), id. */
  crossings: [number, number, string][]
  trace: PieceTrace | null
  /** Every ray the piece was summed over: [lat, lon] it leaves from, the in-plane angle it stands
   *  for (rad; 0 for a point source, its one ray), the Lden it delivers (null: silent) and its
   *  terms. */
  rays?: [number, number, number, number | null, RayTerms?][]
}

/** One ray's terms (dB, losses positive): ground and screening in calm air and bent down,
 *  screening alone in calm air and bent down, the air's absorption; then its slant length (m). */
export type RayTerms = [number, number, number, number, number, number] | null

export interface PopupUpdate {
  seq: number
  /** True until every ring within the reach is read: the numbers shown are valid, but still refined. */
  partial: boolean
  center: [number, number]
  elevation_m: number
  /** The receiver reflection bonus of the surroundings (dB: 0, 1.5 or 3). */
  reflection_db?: number
  /** The place's weather (final update): per period the percent of the time the weather bends
   *  sound down along each of 16 bearings (the direction it travels, clockwise from north), and
   *  the air's absorption per octave band, 63 Hz to 8 kHz (dB/km). */
  weather?: { favourable_percent: number[][], alpha_db_per_km: number[] } | null
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
  /** Everything the list leaves out, together and steady: its Nden in sone (the final update's). */
  rest_nden_sone?: number | null
  /** Its share of the place's loudness, a fraction (the final update's). */
  rest_share?: number | null
  /** The segments view's pieces, when asked for. */
  pieces?: PopupPiece[]
  stats: PopupStats
}

/** The last line of a failed click: the answer is incomplete and must not be shown as final. */
export interface PopupError {
  error: string
}
