// Plain-language definitions of the metric labels in the popup, one entry per term.

type MetricDef = {
  label: string
  description: string
}

export const METRIC_DEFS = {
  emission: {
    label: "Emission",
    description:
      "How much noise the source makes at the source itself, before the sound travels out: its sound power by day. Industry, buildings, sports grounds and ship cells are modelled from their kind and size.",
  },
  aadt: {
    label: "Traffic",
    description:
      "Annual Average Daily Traffic — vehicles per 24 h averaged over the year, split into light / medium / heavy / motorcycle classes. Values are prepared when the map is built from matched traffic counts, estimates or class priors; each class shows whether its value was counted or estimated.",
  },
  trains: {
    label: "Trains/day",
    description:
      "Daily train count separated by passenger and freight, from timetables and freight statistics where available, otherwise estimated for the line.",
  },
  speed: {
    label: "Speed",
    description:
      "Speed used in the CNOSSOS emission calculation. Normally the posted OSM maxspeed; a default for the road class or rail type if no limit is posted; roundabouts cap at 30 km/h.",
  },
  surface: {
    label: "Surface",
    description:
      "Road surface type. Applied as a per-frequency rolling-noise correction in CNOSSOS (asphalt = 0 dB reference, cobbles and paving stones about +4 dB).",
  },
} as const satisfies Record<string, MetricDef>

export type MetricTerm = keyof typeof METRIC_DEFS
