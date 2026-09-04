/**
 * Centralized metric definitions for noise popup tooltips.
 *
 * One entry per term with `label`, `description`, optional `standard`.
 * Structured for future i18n layer — copy is English for now (matches rest
 * of the app per AGENTS.md "English everywhere" rule).
 */

type MetricDef = {
  label: string
  /** Technical description: formulas, standards citations, fine print.
   * Used by the Noise Segments tab (pro debug) via `MetricLabel mode='technical'`. */
  description: string
  /** Public-facing description: plain language, no formulas, no jargon.
   * Used by the Noise Sources tab (public) via `MetricLabel mode='public'`
   * (default). Falls back to `description` when omitted. */
  descriptionPublic?: string
  standard?: string
}

export const METRIC_DEFS: Record<string, MetricDef> = {
  lden: {
    label: "Lden",
    description:
      "Day-Evening-Night weighted noise level over a 24-hour period. Evening gets a +5 dB penalty and night a +10 dB penalty to reflect annoyance.",
    standard: "EU Environmental Noise Directive 2002/49/EC",
  },
  emission: {
    label: "Emission",
    description:
      "Sound power level (Lw) at the source, before any propagation. Roads and railways are LINE sources — per-metre L'w from CNOSSOS-EU vehicle/train counts × speed coefficients. Buildings, industrial sites and leisure areas are POINT sources — total Lw from an area-law (per-m² emission + 10·log10 footprint) discretised over the polygon. The building/leisure layer is a non-standard extension (CNOSSOS models no building source); it is grounded in EN ISO 12354-4 / VDI / DIN engineering data — see /about/methodology.",
    descriptionPublic:
      "How much noise the source makes at the source itself, before the sound travels out. Roads and railways are modelled as lines — louder with more and faster traffic. Buildings, industry and sports/leisure areas are modelled from their size — a bigger footprint is louder.",
    standard: "CNOSSOS-EU Part 2 (road/rail) · engineering area-law (buildings/leisure)",
  },
  aadt: {
    label: "Traffic",
    description:
      "Annual Average Daily Traffic — vehicles per 24 h averaged over the year. Uses matched external traffic datasets where available, otherwise local service-road estimates or CNOSSOS road-class defaults.",
    standard: "Matched traffic datasets / service-tree estimate / CNOSSOS defaults",
  },
  trains: {
    label: "Trains/day",
    description:
      "Daily train count separated by passenger and freight. Sourced from CZPTT timetables and E-PRTR freight reports where available, otherwise defaults for the rail type.",
  },
  speed: {
    label: "Speed",
    description:
      "Speed value actually used in the CNOSSOS emission calculation. Normally the posted OSM maxspeed; defaults to a per-class value if no posted limit; roundabouts cap at 30 km/h.",
  },
  surface: {
    label: "Surface",
    description:
      "Road surface type. Applied as a per-frequency rolling-noise correction in CNOSSOS (asphalt = 0 dB reference, gravel ~+4 dB).",
    standard: "CNOSSOS-EU Annex II",
  },
  baseline: {
    label: "Baseline",
    description:
      "Sum of geometric divergence (distance losses), atmospheric absorption, and ground effect — the attenuation you get over flat uniform terrain.",
    standard: "ISO 9613-2 §7",
  },
  atmospheric: {
    label: "Atmospheric absorption",
    description:
      "α[i] × d_slant / 1000 per band. Standard atmosphere (15 °C, 70 % RH). Scalar shown is A-weighted ΔL_A = full − no_atmospheric Lden.",
    descriptionPublic:
      "Atmospheric absorption (ISO 9613-2 §7.2). Air absorbs sound over distance — most noticeably at high frequencies.",
    standard: "ISO 9613-2 §7.2",
  },
  ground: {
    label: "Ground effect",
    description:
      "CF[i] × G per band, where G = 1 − IMD/100 (hard ↔ soft). Signed — over soft ground CF[i] < 0 at 63/125 Hz, so A_gr can BOOST LF energy. Scalar shown is A-weighted ΔL_A (full − no_ground Lden).",
    descriptionPublic:
      "Ground effect (ISO 9613-2 §7.3 / CNOSSOS §2.5.15). Interaction of direct and ground-reflected rays; soft ground (fields, grass) absorbs more than hard ground (asphalt) and can slightly boost low bass.",
    standard: "ISO 9613-2 §7.3.1 + CNOSSOS-EU §2.5.15",
  },
  terrain: {
    label: "Terrain",
    description:
      "Terrain diffraction via Maekawa/Fresnel (ISO 9613-2 §7.3/7.4), using the dominant edge above line of sight. CNOSSOS §2.5.6(c) Rayleigh δ* gate zeroes bands where δ ≤ λ/4 − δ*. Combined with building/barrier screening in a single Fresnel pass (SPEC §4.6–4.7, anti-double-count).",
    descriptionPublic:
      "Terrain diffraction (ISO 9613-2 §7.4 / CNOSSOS §2.5.6). Hills or embankments between source and receiver bend sound over the top — taller and closer to the path means more reduction.",
    standard: "ISO 9613-2 §7.3/7.4 + CNOSSOS-EU §2.5.6(c)",
  },
  screening: {
    label: "Screening",
    description:
      "Increment of the combined terrain + building + barrier diffraction over pure terrain (A_terrain + A_screen ≡ A_combined, SPEC §4.7 — not a second independent Fresnel pass). The engine intersects exact Overture building footprints and explicit noise barriers with the path. One winning edge may be a bare-earth hill — UI labels it 'terrain' then.",
    descriptionPublic:
      "Building / barrier screening (ISO 9613-2 §7.4 + CNOSSOS §2.5.6). Buildings and noise barriers on the path block the direct line of sight; engine combines with terrain in a single diffraction model.",
    standard: "ISO 9613-2 §7.3 + CNOSSOS-EU §2.5.6(c)",
  },
  vegetation: {
    label: "Vegetation",
    description:
      "Attenuation from dense forest along the sound path, integrated trapezoidally over the WorldCover forest raster. Capped at ~200 m effective depth per ISO 9613-2 Table A.1. Scalar × 0.5 Central-Europe calibration for the binary-forest raster.",
    descriptionPublic:
      "Foliage attenuation (ISO 9613-2 Annex A.2.2). Dense forest along the path absorbs sound; scattered or thin trees contribute little.",
    standard: "ISO 9613-2 §A.2.2",
  },
  per_band: {
    label: "Per-band levels",
    description:
      "Received level in each octave band from 63 Hz to 8 kHz (A-weighted). Useful for spectral comparison.",
  },
  segments: {
    label: "Segments",
    description:
      "Number of OSM microsegments the engine saw in the relevant radius, and their total length. One contributor aggregates all segments that share the same name/ref/class.",
  },
  aircraft: {
    label: "Aircraft",
    description:
      "Aircraft popup is split into airborne and ground ops. Airborne uses Doc 29 empirical NPD tables from observed ADS-B flight events. Ground ops uses airport movement line sources with terrain, screening, and vegetation propagation so runway/taxi/apron activity can be read separately from overflights.",
    standard: "ECAC Doc 29 4th Edition",
  },
  distance: {
    label: "Distance",
    description:
      "Horizontal distance from the receiver point to the nearest microsegment of this source.",
  },
} as const

export type MetricTerm = keyof typeof METRIC_DEFS
