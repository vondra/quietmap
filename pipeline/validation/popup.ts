/**
 * Read one popup answer as outdoor model levels: per-layer periods and shares, the receiver
 * actually computed, indoor-to-facade restoration, and the dominant road's traffic provenance.
 */
import { DATASETS } from '../lib/enrichment-datasets.ts'
import { energySumDb, ldenFromPeriods, type PeriodLevels } from './lib.ts'

type WireSource = { source_type: string; lden: number | null; ld: number | null; le: number | null; ln: number | null }
type WireContributor = {
  source_type: string
  osm_id: number | null
  name: string
  subtype: string
  distance_m: number
  received_lden: number
  metadata?: Record<string, unknown>
}
export type PopupAnswer = {
  center: [number, number]
  receiver?: { lat: number; lng: number; height_m: number }
  total_lden: number | null
  sources: WireSource[]
  top_contributors: WireContributor[]
  envelope_class?: string
  envelope_delta_db?: number
  facade_lden?: number
  unavailable_layers?: string[]
}

export type Contributor = { source_type: string; osm_id: number | null; name: string; subtype: string; distance_m: number; received_lden: number }
export type LayerModel = { lden: number | null; periods: PeriodLevels; share_lden: number }
/** Traffic provenance in the W1 criteria vocabulary (criteria.json cohorts[].membership). */
export type RoadProvenance = 'measured_count' | 'service_tree' | 'continuity_fill' | 'proxy' | 'class_default' | 'other'
export type DominantRoad = {
  osm_id: number | null
  name: string
  road_class: string | null
  distance_m: number
  aadt: { light: number; medium: number; heavy: number; moto: number; total: number }
  traffic_estimated: number | null
  /** Light, medium or heavy flow is an estimate: a counted total split by class shares, or a prior. */
  vehicle_split_estimated: boolean
  dominant_source_id: number | null
  provenance_tier: string | null
  /** Name of the traffic dataset behind the dominant road; null for a class prior. */
  dataset_name: string | null
  /** Year of the traffic dataset (the input year of the criteria's year gap); null for a prior. */
  dataset_year: number | null
  speed_source: string | null
  traffic_provenance: RoadProvenance
}
export type StationModel = {
  receiver: { lat: number; lng: number; height_m: number | null; click_to_receiver_m: number }
  inside_footprint: boolean
  envelope_class: string | null
  envelope_delta_db: number | null
  total: { lden: number | null; periods: PeriodLevels }
  layers: Record<string, LayerModel>
  dominant_layer: string | null
  /** Every listed contributor (the popup lists the 30 loudest), loudest first. */
  contributors: Contributor[]
  /** The loudest listed contributor of every layer (the popup lists the 30 loudest overall). */
  loudest_by_layer: Record<string, Contributor>
  dominant_road: DominantRoad | null
  unavailable_layers: string[]
}

const datasetId = (key: string): number => {
  const dataset = DATASETS.find(entry => entry.key === key)
  if (!dataset) throw new Error(`dataset ${key} missing from the registry`)
  return dataset.id
}
const SERVICE_TREE_SOURCE_ID = datasetId('service-tree-heuristic')
const CONTINUITY_SOURCE_ID = datasetId('road-continuity-heuristic')
const MEASURED_TIERS = new Set(['city-measured', 'national-measured', 'continental-measured', 'global-measured'])
/** `traffic_estimated` bits of light (1), medium (2) and heavy (4) vehicles; moto is 8. */
const LIGHT_MEDIUM_HEAVY_ESTIMATED_BITS = 1 | 2 | 4

/** Flat-earth metres between two nearby points (receiver displacement is at most 100 m). */
function metresBetween(lat1: number, lng1: number, lat2: number, lng2: number): number {
  const north = (lat2 - lat1) * 111_320
  const east = (lng2 - lng1) * 111_320 * Math.cos(((lat1 + lat2) / 2) * Math.PI / 180)
  return Math.hypot(north, east)
}

/**
 * The dataset behind the road's flow decides: a measured tier observed this road's traffic,
 * even when the class split of a counted total is estimated (every class bit set).
 */
export function roadTrafficProvenance(metadata: Record<string, unknown>): RoadProvenance {
  const sourceId = typeof metadata.dominant_source_id === 'number' ? metadata.dominant_source_id : null
  const tier = (metadata.provenance as { tier?: string } | null | undefined)?.tier ?? null
  if (sourceId === SERVICE_TREE_SOURCE_ID) return 'service_tree'
  if (sourceId === CONTINUITY_SOURCE_ID) return 'continuity_fill'
  if (tier && MEASURED_TIERS.has(tier)) return 'measured_count'
  if (tier === 'national-proxy') return 'proxy'
  if (!tier || sourceId === 0 || tier === 'baseline') return 'class_default'
  return 'other'
}

function dominantRoad(contributors: WireContributor[]): DominantRoad | null {
  const road = contributors
    .filter(contributor => contributor.source_type === 'road' && contributor.metadata?.kind === 'road')
    .sort((a, b) => b.received_lden - a.received_lden)[0]
  if (!road) return null
  const metadata = road.metadata!
  const count = (field: string) => typeof metadata[field] === 'number' ? metadata[field] as number : 0
  const roadClass = typeof metadata.road_class === 'string' ? metadata.road_class : null
  const provenance = metadata.provenance as { tier?: string; year?: number; name?: string } | null | undefined
  return {
    osm_id: road.osm_id,
    name: road.name,
    road_class: roadClass,
    distance_m: road.distance_m,
    aadt: {
      light: count('aadt_light'), medium: count('aadt_medium'), heavy: count('aadt_heavy'), moto: count('aadt_moto'),
      total: count('aadt_light') + count('aadt_medium') + count('aadt_heavy') + count('aadt_moto'),
    },
    traffic_estimated: typeof metadata.traffic_estimated === 'number' ? metadata.traffic_estimated : null,
    vehicle_split_estimated: typeof metadata.traffic_estimated !== 'number'
      || (metadata.traffic_estimated & LIGHT_MEDIUM_HEAVY_ESTIMATED_BITS) !== 0,
    dominant_source_id: typeof metadata.dominant_source_id === 'number' ? metadata.dominant_source_id : null,
    provenance_tier: provenance?.tier ?? null,
    dataset_name: provenance?.name ?? null,
    dataset_year: typeof provenance?.year === 'number' ? provenance.year : null,
    speed_source: typeof metadata.speed_source === 'string' ? metadata.speed_source : null,
    traffic_provenance: roadTrafficProvenance(metadata),
  }
}

/**
 * Inside an enclosed footprint every popup level is the indoor estimate, facade − Δ, floored
 * at 0 dB (`envelope.rs`); adding Δ back restores the outdoor facade level. A floored value
 * cannot be restored and becomes null.
 */
function outdoor(level: number | null, delta: number | null): number | null {
  if (level == null || delta == null) return level
  return level > 0 ? level + delta : null
}

export function readPopupAnswer(answer: PopupAnswer, clicked: { lat: number; lng: number }): StationModel {
  const delta = answer.envelope_class ? answer.envelope_delta_db ?? null : null
  if (answer.envelope_class && delta == null) throw new Error('indoor answer without envelope_delta_db')
  const layers: Record<string, LayerModel> = {}
  for (const source of answer.sources) {
    layers[source.source_type] = {
      lden: outdoor(source.lden, delta),
      periods: { day: outdoor(source.ld, delta), evening: outdoor(source.le, delta), night: outdoor(source.ln, delta) },
      share_lden: 0,
    }
  }
  const totalLden = answer.facade_lden ?? outdoor(answer.total_lden, delta)
  const totalEnergy = Object.values(layers).reduce((sum, layer) => sum + (layer.lden == null ? 0 : 10 ** (layer.lden / 10)), 0)
  for (const layer of Object.values(layers)) layer.share_lden = totalEnergy > 0 && layer.lden != null ? 10 ** (layer.lden / 10) / totalEnergy : 0
  const periods: PeriodLevels = {
    day: energySumDb(Object.values(layers).map(layer => layer.periods.day)),
    evening: energySumDb(Object.values(layers).map(layer => layer.periods.evening)),
    night: energySumDb(Object.values(layers).map(layer => layer.periods.night)),
  }
  const receiver = answer.receiver ?? { lat: clicked.lat, lng: clicked.lng, height_m: null }
  const dominant = Object.entries(layers).sort(([, a], [, b]) => b.share_lden - a.share_lden)[0]
  const contributors: Contributor[] = [...answer.top_contributors]
    .sort((a, b) => b.received_lden - a.received_lden)
    .map(contributor => ({
      source_type: contributor.source_type,
      osm_id: contributor.osm_id,
      name: contributor.name,
      subtype: contributor.subtype,
      distance_m: contributor.distance_m,
      received_lden: outdoor(contributor.received_lden, delta) ?? 0,
    }))
  const loudestByLayer: Record<string, Contributor> = {}
  for (const contributor of contributors) loudestByLayer[contributor.source_type] ??= contributor
  return {
    receiver: {
      lat: receiver.lat,
      lng: receiver.lng,
      height_m: receiver.height_m,
      click_to_receiver_m: +metresBetween(clicked.lat, clicked.lng, receiver.lat, receiver.lng).toFixed(1),
    },
    inside_footprint: Boolean(answer.envelope_class),
    envelope_class: answer.envelope_class ?? null,
    envelope_delta_db: delta,
    total: { lden: totalLden ?? ldenFromPeriods(periods.day, periods.evening, periods.night), periods },
    layers,
    dominant_layer: dominant && dominant[1].share_lden > 0 ? dominant[0] : null,
    contributors,
    loudest_by_layer: loudestByLayer,
    dominant_road: dominantRoad(answer.top_contributors),
    unavailable_layers: answer.unavailable_layers ?? [],
  }
}
