/**
 * The frozen panel of criteria v2 (`panel.manifest_fields`): eligibility, receiver, membership,
 * convention variants, corrected measurements and the per-metric uncertainty of every station.
 */
import type { IndicatorComparison } from './comparison.ts'
import {
  conventionVariants, interpolate, METRIC_PERIODS, provenanceClass, roadCohort, siteClass, siteClassCohorts, stated, tableMetric, tile,
  type Criteria, type Variant,
} from './criteria.ts'
import type { StationRow } from './report.ts'

export type MetricEntry = {
  indicator: string
  layer: string | null
  /** 'end': END windows; 'end_variant_06_18_22': published on French END periods, corrected to END. */
  period_basis: 'end' | 'end_variant_06_18_22'
  O_by_variant: Partial<Record<Variant, number>>
  u_components: Record<'instrument' | 'window' | 'year_station' | 'period' | 'height' | 'position', number>
  u_convention_minus3: number
  u_i_by_variant: Partial<Record<Variant, number>>
  /** network drift² + period u² + height² of the cohort band U_c. */
  band_square: number
}
export type ManifestStation = {
  station_id: string; physical_station_id: string; set: string; year: number | null; site_class: string; site_class_basis: string
  cohort: string | null; eligible: boolean; exclusion_reason: string | null
  receiver_lat: number | null; receiver_lng: number | null; receiver_moved_m: number; height_m_used: number | null; height_pending: boolean
  variants: Variant[]; metrics: Record<string, MetricEntry>
  dominant_road_osm_id: number | null; dominant_source_id: number | null; provenance_class: string | null; road_class: string | null
  dominant_distance_m: number | null; z9_holdout_square: boolean; site_cluster: string; k_years: number | null; k_assumed: boolean
}
export type Manifest = { criteria_version: string; baseline_run: string; catalogue: unknown; stations: Record<string, ManifestStation> }

/** A station's metric comparison the model can supply: END windows exactly, or the French END variant. */
export function metricComparison(row: StationRow, metric: string): { comparison: IndicatorComparison; basis: MetricEntry['period_basis'] } | null {
  const candidates = row.comparisons.filter(comparison => comparison.period === METRIC_PERIODS[metric] && comparison.unit === 'dB'
    && comparison.measured != null && comparison.band == null && (comparison.kind === 'weighted' ? comparison.period === 'lden' : comparison.kind === 'window'))
    .sort((a, b) => Number(a.layer != null) - Number(b.layer != null))
  const comparison = candidates[0]
  if (!comparison) return null
  if (comparison.period_mapping === 'exact') return { comparison, basis: 'end' }
  const french = metric === 'lden' ? '06-18/18-22/22-06' : metric === 'ln' ? '22-06' : metric === 'ld' ? '06-18' : '18-22'
  if (comparison.windows === french && comparison.layer !== 'aircraft') return { comparison, basis: 'end_variant_06_18_22' }
  return null
}

/** The model value a metric is scored with: the model's own END value for the French variant. */
export function modelValue(row: StationRow, entry: Pick<MetricEntry, 'layer' | 'period_basis'>, metric: string): number | null {
  const comparison = row.comparisons.find(item => item.period === METRIC_PERIODS[metric] && item.layer === entry.layer && item.unit === 'dB' && item.band == null)
  if (entry.period_basis === 'end') return comparison?.model ?? null
  const levels = entry.layer ? row.model?.layers[entry.layer] : row.model?.total
  if (!levels) return null
  return metric === 'lden' ? levels.lden : levels.periods[METRIC_PERIODS[metric] as 'day' | 'evening' | 'night']
}

function spread(row: StationRow, metric: string): number {
  const own = metric === 'ln' ? row.model?.total.periods.night : row.model?.total.lden
  const values = [own ?? null, ...(row.position_samples ?? []).filter(sample => !sample.dropped).map(sample => metric === 'ln' ? sample.ln : sample.lden)]
    .filter((value): value is number => value != null)
  if (values.length < 2) return 0
  const average = values.reduce((sum, value) => sum + value, 0) / values.length
  return Math.sqrt(values.reduce((sum, value) => sum + (value - average) ** 2, 0) / (values.length - 1))
}

/** Criteria v2 panel: one frozen record per station from the baseline run. */
export function freezeManifest(rows: StationRow[], criteria: Criteria, baselineRun: string, catalogue: unknown): Manifest {
  const u = criteria.uncertainty
  const defaultYear = Number(stated(u.year_gap.k, /\|measurement year - (\d{4})\|/, 'the default input year'))
  const maxK = Number(stated(u.year_gap.interpolate, /k > (\d+)/, 'the largest year gap'))
  const sourceHeights = { road: Number(stated(u.height.pending_term, /road ([\d.]+) m/, 'the road source height')),
    railway: Number(stated(u.height.pending_term, /rail ([\d.]+) m/, 'the rail source height')) }
  const physicalId = (row: StationRow) => row.physical_station_id ?? `${row.set.replace(/-\d{4}$/, '')}/${row.station_id.split('/').slice(1).join('/')}`
  const inputYearOf = (row: StationRow) => row.model?.dominant_layer === 'road' ? row.model.dominant_road?.dataset_year ?? null : null
  const gapOf = (row: StationRow) => row.year == null ? null : Math.abs(row.year - (inputYearOf(row) ?? defaultYear))
  const ownExclusion = (row: StationRow): string | null => {
    const k = gapOf(row)
    return row.truth_kind !== 'measured' ? `truth_kind ${row.truth_kind} (cross-check)`
      : row.measurand !== 'sound_level' ? `measurand ${row.measurand}`
        : row.diagnostic_only ? `diagnostic: ${row.diagnostic_reason ?? ''}`
          : row.months_covered != null && row.months_covered < 9 ? `${row.months_covered} months covered`
            : row.year === 2020 || row.year === 2021 ? `measurement year ${row.year}`
              : k != null && k > maxK ? `year gap ${k} > ${maxK}`
                : row.error ? `popup failed: ${row.error}` : row.unscored ? row.unscored
                  : Object.keys(METRIC_PERIODS).some(metric => metricComparison(row, metric)) ? null : 'no metric the model can supply'
  }
  // One record per physical station among the otherwise eligible: the smallest |year − default|, then the latest.
  const chosen = new Map<string, StationRow>()
  for (const row of rows.filter(entry => ownExclusion(entry) == null)) {
    const current = chosen.get(physicalId(row))
    const rank = (candidate: StationRow) => [Math.abs((candidate.year ?? defaultYear) - defaultYear), -(candidate.year ?? 0)]
    if (!current || rank(row)[0] < rank(current)[0] || (rank(row)[0] === rank(current)[0] && rank(row)[1] < rank(current)[1])) chosen.set(physicalId(row), row)
  }
  const stations: Record<string, ManifestStation> = {}
  for (const row of rows) {
    const { site_class, basis } = siteClass(row)
    const road = row.model?.dominant_road ?? null
    const provenance = provenanceClass(criteria, road)
    const inputYear = inputYearOf(row)
    const k = gapOf(row)
    const receiver = row.receiver ?? { lat: row.lat, lng: row.lng, moved_m: 0 }
    const z15 = tile(receiver.lat, receiver.lng, 15)
    const exclusion = ownExclusion(row)
      ?? (chosen.get(physicalId(row)) !== row ? `another record of physical station ${physicalId(row)} is scored` : null)
    const classEntry = siteClassCohorts(criteria).find(entry => entry.classes.includes(site_class))
    const cohort = !classEntry ? null : classEntry.cohort === 'road cohorts' ? roadCohort(criteria, road) : classEntry.cohort
    const heightUsed = row.receiver_height_used_m
    const heightPending = row.mic_height_m == null || heightUsed == null || Math.abs(row.mic_height_m - heightUsed) > 1e-9
    const lineSource = row.model?.dominant_layer === 'road' ? { height: sourceHeights.road, distance: road?.distance_m ?? 0 }
      : row.model?.dominant_layer === 'railway' ? { height: sourceHeights.railway, distance: row.model.loudest_by_layer.railway?.distance_m ?? 0 } : null
    const slant = (height: number) => Math.hypot(lineSource!.distance, height - lineSource!.height)
    const heightTerm = !heightPending || !lineSource || lineSource.distance <= 0 ? 0
      : Math.abs(10 * Math.log10(slant(row.mic_height_m ?? 6) / slant(heightUsed ?? 4)))
    const variants = conventionVariants(row)
    const metrics: Record<string, MetricEntry> = {}
    for (const metric of Object.keys(METRIC_PERIODS)) {
      const found = metricComparison(row, metric)
      if (!found) continue
      const table = tableMetric(metric)
      const french = found.basis === 'end_variant_06_18_22' ? u.period.end_variant_06_18_22 : null
      const components = {
        instrument: row.instrument_class === '1' || row.instrument_class === 'class1' ? u.instrument.class1 : u.instrument.class2_or_unknown,
        window: u.window_months_to_db[table][String(Math.min(row.months_covered ?? 9, 12))] ?? 0,
        year_station: k == null ? 0 : interpolate(u.year_gap.station_rms_db[table], k),
        period: french ? french.u_db[table] : 0,
        height: heightTerm,
        position: spread(row, metric),
      }
      const base = Math.sqrt(Object.values(components).reduce((sum, value) => sum + value ** 2, 0))
      const convention = u.convention.u_when_minus3_applied_db
      const O = found.comparison.measured! + (french ? french.correction_to_end_equivalent_db[table] : 0)
      metrics[metric] = {
        indicator: found.comparison.indicator, layer: found.comparison.layer, period_basis: found.basis,
        O_by_variant: Object.fromEntries(variants.map(variant => [variant, +(O + (variant === 'minus3' ? criteria.constants.convention_alternative_db.value : 0)).toFixed(2)])),
        u_components: Object.fromEntries(Object.entries(components).map(([name, value]) => [name, +value.toFixed(3)])) as MetricEntry['u_components'],
        u_convention_minus3: convention,
        u_i_by_variant: Object.fromEntries(variants.map(variant => [variant, +(variant === 'minus3' ? Math.hypot(base, convention) : base).toFixed(3)])),
        band_square: (k == null ? 0 : interpolate(u.year_gap.network_mean_rms_db[table], k)) ** 2 + components.period ** 2 + components.height ** 2,
      }
    }
    stations[row.key] = {
      station_id: row.key, physical_station_id: physicalId(row), set: row.set, year: row.year, site_class, site_class_basis: basis,
      cohort, eligible: exclusion == null, exclusion_reason: exclusion,
      receiver_lat: row.receiver?.lat ?? null, receiver_lng: row.receiver?.lng ?? null, receiver_moved_m: receiver.moved_m,
      height_m_used: heightUsed, height_pending: heightPending, variants, metrics,
      dominant_road_osm_id: road?.osm_id ?? null, dominant_source_id: road?.dominant_source_id ?? null, provenance_class: provenance,
      road_class: road?.road_class ?? null, dominant_distance_m: road?.distance_m ?? null, z9_holdout_square: row.holdout_square,
      site_cluster: `${row.set}|z15 ${z15.x}/${z15.y}`, k_years: k, k_assumed: inputYear == null,
    }
  }
  return { criteria_version: criteria.criteria_version, baseline_run: baselineRun, catalogue, stations }
}
