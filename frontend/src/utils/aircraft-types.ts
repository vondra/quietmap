/**
 * Aircraft typecode display helpers.
 *
 * The popup must be a FAITHFUL view of what's in the data. We compress long
 * strings to fit columns but never hide the underlying value — tooltips
 * expose the full string and explain how it's produced.
 *
 * Backend popup payload:
 *   • `top_aircraft` (per band) = noise-class label, e.g. "WING_B738",
 *     "HELICOPTER", "WING_FALLBACK". Source:
 *     `engine/noise-compute/src/emission/profiles_generated.rs`
 *     `CLASS_NAMES[noise_class_of(top_profile_idx)]`.
 *   • `top_flights[].profile` = `Profile.name` string built by the generator
 *     (`scripts/build-aircraft-profiles.py`) as `f"{typecode}/{acft.acft_id}"`.
 *     `typecode` = ICAO Doc 8643 4-letter; `acft_id` = EASA ANP database ID.
 *     Special case: `FALLBACK/737800` — typecode "FALLBACK" was hand-curated
 *     against ANP's 737-800 row as a placeholder; `recompute_fallback_npd`
 *     later overwrites the NPD values to a traffic-weighted energy-mean of
 *     all 123 profiles, but the display string keeps the historical
 *     placeholder.
 */
import { PROFILE_CLASS, CLASS_ANCHOR } from './profile-class.generated'

/** Top ~50 ICAO typecodes → human aircraft model. Mapped subset for tooltip. */
const TYPECODE_MODEL: Record<string, string> = {
  // Boeing 737 family
  B737: 'Boeing 737-700', B738: 'Boeing 737-800', B739: 'Boeing 737-900',
  B734: 'Boeing 737-400', B735: 'Boeing 737-500', B733: 'Boeing 737-300',
  B736: 'Boeing 737-600',
  B38M: 'Boeing 737 MAX 8', B39M: 'Boeing 737 MAX 9', B37M: 'Boeing 737 MAX 7',
  // Airbus A320 family
  A319: 'Airbus A319', A320: 'Airbus A320', A321: 'Airbus A321',
  A19N: 'Airbus A319neo', A20N: 'Airbus A320neo', A21N: 'Airbus A321neo',
  // Airbus A220
  BCS1: 'Airbus A220-100', BCS3: 'Airbus A220-300',
  // Boeing 757 / 767 / 777 / 787
  B752: 'Boeing 757-200', B753: 'Boeing 757-300',
  B763: 'Boeing 767-300', B764: 'Boeing 767-400',
  B772: 'Boeing 777-200', B773: 'Boeing 777-300',
  B77W: 'Boeing 777-300ER', B77L: 'Boeing 777-200LR', B77F: 'Boeing 777F',
  B788: 'Boeing 787-8', B789: 'Boeing 787-9', B78X: 'Boeing 787-10',
  // Boeing 747
  B741: 'Boeing 747-100', B742: 'Boeing 747-200', B744: 'Boeing 747-400', B748: 'Boeing 747-8',
  // Airbus A330 / A340 / A350 / A380
  A332: 'Airbus A330-200', A333: 'Airbus A330-300',
  A338: 'Airbus A330-800neo', A339: 'Airbus A330-900neo',
  A342: 'Airbus A340-200', A343: 'Airbus A340-300', A346: 'Airbus A340-600',
  A359: 'Airbus A350-900', A35K: 'Airbus A350-1000',
  A306: 'Airbus A300-600', A310: 'Airbus A310', A388: 'Airbus A380-800',
  // McDonnell Douglas / others
  MD11: 'McDonnell Douglas MD-11', DC10: 'McDonnell Douglas DC-10',
  L101: 'Lockheed L-1011 TriStar', IL76: 'Ilyushin Il-76',
  // Embraer regional + business
  E170: 'Embraer 170', E190: 'Embraer 190', E195: 'Embraer 195',
  E290: 'Embraer E190-E2', E295: 'Embraer E195-E2',
  E75L: 'Embraer 175 (long wing)', E75S: 'Embraer 175 (short wing)',
  E135: 'Embraer ERJ-135', E145: 'Embraer ERJ-145',
  E55P: 'Embraer Phenom 300', E545: 'Embraer Legacy 450', E550: 'Embraer Legacy 500',
  E35L: 'Embraer Legacy 600/650',
  // Bombardier CRJ
  CRJ2: 'Bombardier CRJ-200', CRJ7: 'Bombardier CRJ-700', CRJ9: 'Bombardier CRJ-900',
  // Bombardier Challenger / Global / Learjet / Gulfstream
  CL30: 'Bombardier Challenger 300/350', CL35: 'Bombardier Challenger 350',
  CL60: 'Bombardier Challenger 600', CL64: 'Bombardier Challenger 650',
  GLEX: 'Bombardier Global Express', GLF5: 'Gulfstream G500', GLF6: 'Gulfstream G650',
  GL5T: 'Bombardier Global 5000', GL7T: 'Bombardier Global 7500',
  GLF4: 'Gulfstream G450', GLF7: 'Gulfstream G700',
  LJ45: 'Learjet 45', LJ60: 'Learjet 60',
  // Cessna piston singles
  C150: 'Cessna 150', C152: 'Cessna 152', C170: 'Cessna 170',
  C172: 'Cessna 172 Skyhawk', C177: 'Cessna 177 Cardinal',
  C180: 'Cessna 180', C182: 'Cessna 182 Skylane', C185: 'Cessna 185',
  // Cessna twin / turboprop / jets
  C208: 'Cessna 208 Caravan', C25A: 'Cessna Citation CJ1', C25B: 'Cessna Citation CJ2',
  C25C: 'Cessna Citation CJ4', C510: 'Cessna Citation Mustang',
  C525: 'Cessna CitationJet', C550: 'Cessna Citation II', C560: 'Cessna Citation V',
  C56X: 'Cessna Citation Excel/XLS', C68A: 'Cessna Citation Latitude',
  C700: 'Cessna Citation Longitude', C750: 'Cessna Citation X',
  C310: 'Cessna 310', C402: 'Cessna 402',
  // Piper
  P28A: 'Piper PA-28 Cherokee', P28B: 'Piper PA-28 Warrior', P28R: 'Piper PA-28R Arrow',
  P32R: 'Piper PA-32R Saratoga',
  PA24: 'Piper PA-24 Comanche', PA31: 'Piper PA-31 Navajo',
  PA46: 'Piper PA-46 Malibu',
  // Beechcraft
  BE20: 'Beech King Air 200', BE35: 'Beech V35 Bonanza', BE33: 'Beech 33 Bonanza',
  BE40: 'Beechjet 400', BE58: 'Beech Baron 58', BE95: 'Beech 95 Travel Air',
  BE9L: 'Beech 90 King Air',
  B190: 'Beech 1900', B350: 'Beech King Air 350',
  // Pilatus / TBM / SAAB
  PC12: 'Pilatus PC-12', TBM7: 'Daher TBM-700/850/900',
  SF34: 'Saab 340', SF50: 'Cirrus SF50 Vision Jet',
  // ATR / Dash
  AT72: 'ATR 72', AT75: 'ATR 72-500/600', AT76: 'ATR 72-600',
  DH8A: 'DHC-8-100 Dash 8', DH8C: 'DHC-8-300', DH8D: 'DHC-8-400',
  // Falcon / Hawker / 717
  F2TH: 'Dassault Falcon 2000', F900: 'Dassault Falcon 900',
  H25B: 'Hawker 800XP', B712: 'Boeing 717-200',
  // Cirrus / homebuilt / sport
  S22T: 'Cirrus SR22T', SR20: 'Cirrus SR20', SR22: 'Cirrus SR22',
  RV6: "Vans RV-6", RV9: "Vans RV-9", RV10: "Vans RV-10",
  RV12: "Vans RV-12", RV4: "Vans RV-4",
  // Helicopters
  EC35: 'Eurocopter EC135', EC30: 'Eurocopter EC130', EC75: 'Eurocopter EC175',
  EC55: 'Eurocopter EC155', EC45: 'Eurocopter EC145',
  AS50: 'Aérospatiale AS350 Écureuil', AS55: 'Aérospatiale AS355 Écureuil 2',
  AS65: 'Aérospatiale AS565 Panther',
  H125: 'Airbus H125', H130: 'Airbus H130', H135: 'Airbus H135',
  H145: 'Airbus H145', H155: 'Airbus H155', H160: 'Airbus H160',
  H175: 'Airbus H175', H215: 'Airbus H215', H225: 'Airbus H225',
  A109: 'Leonardo A109', A119: 'Leonardo A119', A139: 'Leonardo AW139',
  A169: 'Leonardo AW169', A189: 'Leonardo AW189',
  R22: 'Robinson R22', R44: 'Robinson R44', R66: 'Robinson R66',
  B407: 'Bell 407', B412: 'Bell 412', B429: 'Bell 429', B505: 'Bell 505',
  S76: 'Sikorsky S-76', S92: 'Sikorsky S-92',
}

/**
 * Class label → anchor typecode. WING_B738 → "B738", HELICOPTER → "HELI",
 * WING_FALLBACK → "Average NPD" (the synthetic energy-mean class).
 */
export function classToAnchorTypecode(className: string | null | undefined): string {
  if (!className) return '?'
  if (className === 'HELICOPTER') return 'HELI'
  if (className === 'WING_FALLBACK' || className === 'FALLBACK') return 'Average NPD'
  const m = className.match(/^(?:WING|FUSE|PROP)_(.+)$/)
  return m ? m[1] : className
}

/**
 * Profile name → typecode. The string format `<typecode>/<acft_id>` is set
 * by `build_profiles` in `scripts/build-aircraft-profiles.py`. Returns the
 * full input unchanged if no slash is present.
 */
export function parseProfileName(profileName: string | null | undefined): string {
  if (!profileName) return '?'
  const idx = profileName.indexOf('/')
  return idx < 0 ? profileName : profileName.slice(0, idx)
}

/** ICAO typecode → human model name. Falls back to typecode itself for unknown entries. */
export function modelName(typecode: string): string {
  return TYPECODE_MODEL[typecode] ?? typecode
}

/**
 * Display name for a rep-typecode cell. Maps the synthetic
 * `"FALLBACK"` (the WING_FALLBACK class' anchor typecode, used for
 * traffic that didn't match any specific NPD profile) to a
 * user-readable label. Pass any other typecode through unchanged.
 *
 * Use this anywhere `rep_typecode` is rendered to the user;
 * "FALLBACK" is an internal implementation detail.
 */
export function displayTypecode(typecode: string): string {
  return typecode === 'FALLBACK' ? 'Average NPD' : typecode
}

/**
 * Tooltip for an aircraft cell. Same 3-line format in both Bands and
 * Top flights tables.
 *
 *   line 1 = aircraft model name (from typecode)
 *   line 2 = noise class this typecode falls into (Voronoi assignment)
 *   line 3 = NPD curve provenance — either:
 *              "from this aircraft's own profile"   (typecode = class anchor)
 *              "from <ANCHOR> (Voronoi-assigned)"   (typecode != anchor)
 *              "energy-mean of 123 profiles"        (FALLBACK / WING_FALLBACK)
 *
 * Caller passes typecode (the cell's display value) and optionally className
 * (for Bands, where the band already exposes the class label directly).
 */
export function aircraftTooltip(typecode: string, className?: string | null): string {
  const cls = className ?? PROFILE_CLASS[typecode] ?? null
  const anchor = cls ? CLASS_ANCHOR[cls] : null
  const lines = [modelName(typecode)]
  if (cls) lines.push(`Class: ${cls}`)
  if (anchor) {
    if (anchor.isEnergyMean) {
      lines.push('NPD curve: traffic-weighted energy-mean of 123 profiles')
    } else if (anchor.typecode === typecode) {
      lines.push("NPD curve: this aircraft's own profile")
    } else {
      lines.push(`NPD curve: ${anchor.typecode} (${modelName(anchor.typecode)}) — Voronoi-assigned`)
    }
  }
  return lines.join('\n')
}

/**
 * Top-flight cell tooltip — `aircraftTooltip` (type/class/NPD) plus the
 * per-flight identity block (callsign + ICAO hex + trace-globe click hint
 * when the row is clickable). Missing `icaoHex` is the synthetic signal:
 * the identity block degrades to a "Synthetic id" line so the table
 * doesn't show empty hex / phantom "click to open trace" hints. Pass
 * `synthetic: true` explicitly when the caller has its own synth flag
 * but ALSO sends a non-empty hex (rare — happens at the DetailPopup
 * site where the engine surface keeps both fields independently).
 * Used by both `TopFlightsTable` (DetailPopup) and
 * `CruiseLoudestFlights` (SegmentExpanded).
 */
export function aircraftFlightTooltip(opts: {
  typecode: string
  callsign?: string | null
  icaoHex?: string | null
  synthetic?: boolean
}): string {
  const base = aircraftTooltip(opts.typecode)
  // Synthetic when explicitly flagged, OR when no hex is available
  // (the only "click to open trace" target). Defensive: an explicit
  // `synthetic: false` plus a missing hex still falls through to the
  // non-synth branch but skips the broken "ICAO hex: ..." line.
  const isSynth = opts.synthetic === true || !opts.icaoHex
  if (isSynth) {
    return `${base}\n\nSynthetic id — anonymous-transponder trace or cruise-cell bucket aggregate; no single per-flight identity`
  }
  const csLine = opts.callsign ? `\nCallsign: ${opts.callsign}` : ''
  return `${base}${csLine}\n\nICAO hex: ${(opts.icaoHex ?? '').toUpperCase()}\nClick to open the flight trace (adsb.lol for GA/heli, adsbexchange for airliners)`
}
