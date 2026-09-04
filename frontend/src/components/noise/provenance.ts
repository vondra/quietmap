import type {
  DatasetProvenance,
  ProvenanceTier,
  RailTrainSource,
  RoadTrafficSource,
} from '../../types/noise.ts'

export type { RailTrainSource, RoadTrafficSource } from '../../types/noise.ts'

// Pure provenance wording shared by the contributor and segment views. Keep
// this module free of React/DOM imports so the strings can be tested with
// Node's built-in test runner without adding a frontend test framework.

const AUTHORITATIVE_TIERS: ReadonlySet<ProvenanceTier> = new Set([
  'city-measured',
  'national-measured',
  'continental-measured',
  'global-measured',
] as const)

type TierClass = 'authoritative' | 'estimate' | 'baseline' | 'unknown'

function classifyTier(p: DatasetProvenance): TierClass {
  if (p.tier != null && AUTHORITATIVE_TIERS.has(p.tier)) return 'authoritative'
  if (p.tier === 'national-proxy' || p.tier === 'heuristic') return 'estimate'
  if (p.tier === 'baseline') return 'baseline'
  // A missing tier can occur briefly during a rolling frontend/backend deploy.
  // Inconsistent legacy buckets must also fail conservatively: unknown input
  // is neither proof of a measurement nor proof of a proxy estimate.
  return 'unknown'
}

export function formatProv(p: DatasetProvenance | null | undefined): string {
  if (!p) return ''
  const parts: string[] = [p.name]
  if (p.year != null) parts.push(`(${p.year})`)
  if (p.license) parts.push(`· ${p.license}`)
  return parts.join(' ')
}

/** Compact road-traffic source block with an optional URL and method hint. */
export function roadSourceDescription(
  trafficSource: RoadTrafficSource,
  provenance: DatasetProvenance | null | undefined,
  roadClass: string,
): string {
  if (trafficSource === 'matched_external') {
    if (!provenance) {
      return (
        'Source: external road-traffic input\n' +
        '  (dataset metadata and measurement status unavailable)'
      )
    }
    const url = provenance.url ? `\n  ${provenance.url}` : ''
    const tierClass = classifyTier(provenance)
    // A high-authority tier is not proof of a direct count: some city
    // datasets contain working-day or interpolated values under a
    // *-measured provenance rank. Until the wire carries the registry's
    // counted/derived method, describe it neutrally as an external input.
    const method = tierClass === 'authoritative'
      ? 'external AADT input per OSM way'
      : tierClass === 'estimate'
        ? 'estimated AADT, not a direct measurement'
        : tierClass === 'baseline'
          ? 'model-derived AADT baseline, not an observed count'
          : 'external AADT input; measurement status unavailable'
    return `Source: ${formatProv(provenance)}${url}\n  (${method})`
  }
  if (trafficSource === 'estimated_service_tree') {
    // The "heuristic" tier covers two datasets: the service-tree (local roads,
    // trips accumulated from buildings) and the continuity-fill (major roads,
    // a measured neighbour's AADT carried along the same road). Name whichever
    // it actually is via the dataset; both are estimates, not measurements.
    const name = provenance ? formatProv(provenance) : 'Service-tree heuristic'
    return (
      `Source: ${name} — ${roadClass} class\n` +
      `  (estimated AADT, not a direct measurement)`
    )
  }
  if (trafficSource === 'default_by_class' && provenance) {
    const url = provenance.url ? `\n  ${provenance.url}` : ''
    // `trafficSource` is authoritative for AADT. In particular, the taper can
    // stamp baseline provenance for a speed-only adjustment while leaving raw
    // AADT empty, in which case the engine still uses the class default.
    const method = provenance.tier === 'baseline'
      ? 'class-default traffic count; listed source may apply to speed only'
      : 'class-default traffic count; source metadata does not establish AADT'
    return (
      `Source: ${formatProv(provenance)}${url} — ${roadClass} class\n` +
      `  (${method})`
    )
  }
  return (
    `Source: CNOSSOS Annex II default — ${roadClass} class\n` +
    `  (no enrichment data in this area)`
  )
}

/** Compact rail train-count source block for one category. */
export function railTrainSourceLine(
  source: RailTrainSource,
  provenance: DatasetProvenance | null | undefined,
  railType: string,
): string {
  if (source === 'arrow') {
    if (!provenance) {
      return (
        'External train-count input\n' +
        '  (dataset metadata and measurement status unavailable)'
      )
    }
    const url = provenance.url ? `\n  ${provenance.url}` : ''
    const tierClass = classifyTier(provenance)
    // Most authoritative rail inputs are schedules (GTFS/timetables), not
    // observed pass-by counters. The provenance rank alone cannot distinguish
    // those methods, so keep the wording neutral until method is wired.
    const method = tierClass === 'authoritative'
      ? 'external trains/day input per OSM way'
      : tierClass === 'estimate'
        ? 'estimated trains/day, not a direct measurement'
        : tierClass === 'baseline'
          ? 'model-derived trains/day baseline, not an observed count'
          : 'external train-count input; measurement status unavailable'
    return `${formatProv(provenance)}${url}\n  (${method})`
  }
  return (
    `CNOSSOS Annex IV default — ${railType}\n` +
    `  (no enrichment data)`
  )
}
