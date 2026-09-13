import type { Contributor } from '../../../types/noise'
import { fmt, fmtFloat, fmtInt, fmtCompact, txtTable, type TableRow } from '../../../utils/formatters'
import { MetricLabel, DataPoint } from '../noise-tooltips'
import { formatProv, lineRow, railTrafficLabel, railTrafficDescription, roadSourceDescription, subtypeLabel } from '../shared'

// Road and rail source helpers are shared with the Noise segments tab
// (SegmentExpanded), so both views use identical attribution wording. The
// per-effect propagation tooltips differ by tab:
// ContributorRow uses `MetricLabel` default mode='public' (plain-language
// descriptions from `descriptionPublic`); SegmentExpanded uses inline
// HoverText with full technical detail (formulas, δ*, Rayleigh gate).

/** Pretty renderer for source-specific metadata (typed per discriminant). */
export function MetadataRows({ c }: { c: Contributor }) {
  const m = c.metadata
  if (!m) return null

  if (m.kind === 'road') {
    const nomTotal = m.aadt_light_nominal + m.aadt_medium_nominal + m.aadt_heavy_nominal + m.aadt_moto_nominal
    const effTotal = m.aadt_light_effective + m.aadt_medium_effective + m.aadt_heavy_effective + m.aadt_moto_effective
    const isDefault = m.traffic_source === 'default_by_class'
    const hasSpeedRange = m.speed_min_kmh < m.speed_max_kmh
    // Derestricted (maxspeed=none, e.g. German Autobahn): no number exists;
    // the engine models DERESTRICTED_SPEED_KMH and reports it in speed_kmh.
    // Keyed off the null posted value, not speed_source — a derestricted road
    // through a roundabout reports speed_source "roundabout_cap" but still
    // carries posted=null (engine sets null only for the 255 sentinel).
    let postedMaxspeed: string
    if (m.speed_posted_kmh == null) {
      postedMaxspeed = 'no limit'
    } else if (m.speed_posted_kmh > 0) {
      postedMaxspeed = `${m.speed_posted_kmh} km/h`
    } else {
      postedMaxspeed = '— (none)'
    }
    const speedText = txtTable([
      ['Source', m.speed_source.replace(/_/g, ' ')],
      ['Posted maxspeed', postedMaxspeed],
      ['Class default', m.road_class],
      { sep: true },
      ['Dominant seg.', `${m.speed_kmh.toFixed(0)} km/h`],
      ...(hasSpeedRange ? [['Range (group)', `${m.speed_min_kmh.toFixed(0)}–${m.speed_max_kmh.toFixed(0)} km/h`] as [string, string]] : []),
      '',
      'Values from the loudest segment.',
      ...(hasSpeedRange ? ['Speed varies across grouped segments.'] : []),
    ], 18, 12)
    const adjustmentRatio = nomTotal > 0 ? effTotal / nomTotal : 1
    const hasAdjustment = Math.abs(adjustmentRatio - 1) > 0.01
    const sourceLines = roadSourceDescription(m.traffic_source, m.provenance, m.road_class).split('\n')
    // A baseline provenance record can be speed-only; `default_by_class`
    // remains the source of the traffic count in that case.
    const defaultFootnote = m.provenance?.tier === 'baseline'
      ? '* class default; listed source may apply to speed only'
      : '* class default (no census match for this segment)'
    const trafficText = txtTable([
      ...sourceLines,
      '',
      'Traffic on this road segment:',
      ...(m.aadt_light_effective > 0 ? [['  Light', fmtInt(Math.round(m.aadt_light_effective))] as [string, string]] : []),
      ...(m.aadt_medium_effective > 0 ? [['  Medium', fmtInt(Math.round(m.aadt_medium_effective))] as [string, string]] : []),
      ...(m.aadt_heavy_effective > 0 ? [['  Heavy', fmtInt(Math.round(m.aadt_heavy_effective))] as [string, string]] : []),
      ...(m.aadt_moto_effective > 0 ? [['  Moto', fmtInt(Math.round(m.aadt_moto_effective))] as [string, string]] : []),
      { sep: true },
      ['  Total', `${fmtInt(Math.round(effTotal))}/day${isDefault ? '*' : ''}`] as [string, string],
      ...(isDefault ? ['', defaultFootnote] : []),
      ...(hasAdjustment
        ? [
            '',
            'Adjustment for this segment:',
            ['  Nominal', `${fmtInt(Math.round(nomTotal))}/day`] as [string, string],
            ['  ', `× ${adjustmentRatio.toFixed(2)} combined adjustment`] as [string, string],
            { sep: true },
            ['  This segment', `${fmtInt(Math.round(effTotal))}/day`] as [string, string],
          ]
        : []),
      '',
      `(dominant segment, ${Math.round(m.dominant_distance_m)} m away)`,
    ] as TableRow[], 18, 12)
    const segmentsText = txtTable([
      ['Microsegments', String(m.segment_count)],
      ['Total length', `${(m.total_length_m / 1000).toFixed(2)} km`],
      ['Closest point', `${Math.round(m.closest_distance_m)} m`],
      ['Dominant seg.', `#${m.dominant_segment_idx} (${Math.round(m.dominant_distance_m)} m)`],
      ...(m.bridge_count > 0 ? [['Bridge segments', String(m.bridge_count)] as [string, string]] : []),
      '',
      'Grouped by ref + name + class.',
      'Metadata from loudest segment.',
    ], 18, 12)
    const hasMixedOneway = m.oneway_segment_count > 0 && m.twoway_segment_count > 0
    const surfaceText = txtTable([
      ['Type', m.surface],
      ['Rolling correction', `${fmt(m.surface_corr_db)} dB`],
      ['Lanes', String(m.lanes)],
      ['Oneway', m.oneway ? 'yes' : 'no'],
      ...(hasMixedOneway ? [
        '',
        `Group: ${m.oneway_segment_count} oneway + ${m.twoway_segment_count} two-way segs`,
      ] : []),
    ], 18, 12)
    return (
      <>
        {lineRow(
          <MetricLabel term="speed" />,
          <DataPoint title="Speed used in CNOSSOS emission" text={speedText}>
            {hasSpeedRange ? `${m.speed_min_kmh.toFixed(0)}–${m.speed_max_kmh.toFixed(0)}` : m.speed_kmh.toFixed(0)} km/h
          </DataPoint>,
        )}
        {lineRow(
          <MetricLabel term="aadt">Traffic</MetricLabel>,
          <DataPoint title="Daily traffic on this road segment" text={trafficText}>
            {`${fmtCompact(Math.round(effTotal))}/day${isDefault ? '*' : ''}`}
          </DataPoint>,
        )}
        {lineRow(
          <MetricLabel term="segments">Segments</MetricLabel>,
          <DataPoint title="Road aggregation" text={segmentsText}>
            {m.segment_count} · {(m.total_length_m / 1000).toFixed(1)} km
          </DataPoint>,
        )}
        {lineRow(
          <MetricLabel term="surface">Surface</MetricLabel>,
          <DataPoint title="CNOSSOS surface correction" text={surfaceText}>
            {m.surface}
          </DataPoint>,
        )}
      </>
    )
  }

  if (m.kind === 'rail') {
    const speedText = txtTable([
      ['Source', m.speed_source.replace(/_/g, ' ')],
      ['Posted maxspeed', m.maxspeed_posted_kmh > 0 ? `${m.maxspeed_posted_kmh} km/h` : '— (none)'],
      ['Rail type', m.rail_type],
      ['Usage', m.usage],
      ...(m.highspeed ? [['Highspeed flag', 'yes (default 300)'] as [string, string]] : []),
      { sep: true },
      ['Effective', `${m.speed_kmh.toFixed(0)} km/h`],
    ], 18, 14)
    const trainsText = railTrafficDescription(m.traffic, m.passenger_provenance, m.freight_provenance)
    const segmentsText = txtTable([
      ['Microsegments', String(m.segment_count)],
      ['Total length', `${(m.total_length_m / 1000).toFixed(2)} km`],
      ['Closest point', `${Math.round(m.closest_distance_m)} m`],
      ['Dominant seg.', `#${m.dominant_segment_idx} (${Math.round(m.dominant_distance_m)} m)`],
      ...(m.bridge ? [['Bridge', 'yes'] as [string, string]] : []),
      '',
      'Metadata from loudest segment.',
    ], 18, 12)
    return (
      <>
        {lineRow(
          <MetricLabel term="speed" />,
          <DataPoint title="Speed at the energy-dominant (loudest) segment in this rail group — matches the road-popup pattern. Earlier this was the closest segment, which misrepresented audible traffic whenever a fast mainline sat farther than a quiet siding." text={speedText}>
            {m.speed_kmh.toFixed(0)} km/h
          </DataPoint>,
        )}
        {lineRow(
          <MetricLabel term="trains">Trains/day</MetricLabel>,
          <DataPoint title="Expected passages at the loudest segment, with passenger and freight evidence shown separately." text={trainsText}>
            {railTrafficLabel(m.traffic)}
          </DataPoint>,
        )}
        {lineRow(
          <MetricLabel term="segments">Segments</MetricLabel>,
          <DataPoint title="Rail aggregation" text={segmentsText}>
            {m.segment_count} · {(m.total_length_m / 1000).toFixed(1)} km
          </DataPoint>,
        )}
      </>
    )
  }

  if (m.kind === 'building') {
    const typeLabel = subtypeLabel('building', m.building_type)
    const buildingText = txtTable([
      ['Type', typeLabel],
      ...(m.height_m > 0 ? [['Height', `${m.height_m.toFixed(1)} m`] as [string, string]] : []),
      ...(m.floors > 1 ? [['Floors', String(m.floors)] as [string, string]] : []),
      ...(m.area_m2 > 0 ? [['Footprint', `${Math.round(m.area_m2).toLocaleString()} m²`] as [string, string]] : []),
      ...(m.address ? ['', `Address: ${m.address}`] : []),
    ], 14, 20)
    // Collapsed line stays short (no wrap): type + floors only when multi-storey.
    return lineRow(
      'Building',
      <DataPoint title="Building metadata" text={buildingText}>
        {typeLabel}{m.floors > 1 ? ` · ${m.floors} fl.` : ''}
      </DataPoint>,
    )
  }

  if (m.kind === 'industrial') {
    const hasDetail = m.nace || m.grid_point_count > 0
    const prov = m.provenance
    const siteText = txtTable([
      ['Type', m.source_type.replace(/_/g, ' ')],
      ['Source', prov ? formatProv(prov) : 'OSM tags + NACE profile'],
      ...(m.area_m2 > 0 ? [['Area', `${Math.round(m.area_m2).toLocaleString()} m²`] as [string, string]] : []),
      ...(m.nace ? [['NACE', m.nace] as [string, string]] : []),
      ...(m.grid_point_count > 0 ? [['Grid points', String(m.grid_point_count)] as [string, string]] : []),
      ...(m.grid_point_count > 1
        ? ['', 'Large sites split into a 75 m grid;', 'each cell carries its area share', 'of the total sound power.']
        : []),
    ], 16, 16)
    const summary = m.area_m2 > 0
      ? `${Math.round(m.area_m2).toLocaleString()} m²`
      : m.source_type.replace(/_/g, ' ')
    if (!hasDetail && !(m.area_m2 > 0)) return null
    return lineRow(
      'Industrial',
      <DataPoint title="Industrial site metadata" text={siteText}>
        {summary}
      </DataPoint>,
    )
  }

  if (m.kind === 'aircraft' && m.variant === 'airborne' && m.airborne) {
    const a = m.airborne
    // Display thresholds for the sampling-fragility caveat — the Rust
    // doc on AircraftAirborneDetail points here as their single home.
    const DAY_SHARE_WARN = 0.5
    const FLIGHT_SHARE_WARN = 0.3
    const dayShare = a.top_day_energy_share ?? 0
    const flightShare = a.top_flight_energy_share ?? 0
    const sparse = dayShare > DAY_SHARE_WARN || flightShare > FLIGHT_SHARE_WARN
    // GA full-year hybrid: airline classes sample N days while GA +
    // helicopters use a separate full available-year window. State both
    // bases when they differ; otherwise the popup implies jets used the
    // same full-year window.
    const nDays = a.sample_days
    const gaDays = a.ga_sample_days
    const hybrid = gaDays != null && nDays != null && gaDays !== nDays
    const basisLine = hybrid
      ? `jets ${nDays} d/yr · GA+heli ${gaDays} d/yr.`
      : `Lden averaged from ${nDays ?? '–'} sample days/yr.`
    const sampleText = txtTable([
      'ADS-B flight tracks (adsbexchange + adsb.lol).',
      basisLine,
      ...(sparse
        ? [
            '',
            `⚠ ${Math.round(dayShare * 100)}% of energy from ${a.top_day_date || 'one day'}.`,
            ...(flightShare > FLIGHT_SHARE_WARN
              ? [`${Math.round(flightShare * 100)}% from a single flight.`]
              : []),
            'Sparse local traffic — value may over-',
            'state a one-off (e.g. helicopter work).',
          ]
        : []),
    ], 16, 16)
    const badge = hybrid ? `${nDays}/${gaDays} d/yr` : `${nDays ?? '–'} days/yr`
    return lineRow(
      'Data',
      <DataPoint title="Aircraft data source" text={sampleText}>
        {sparse ? <>⚠ sparse sample</> : <>ADS-B · {badge}</>}
      </DataPoint>,
    )
  }

  if (m.kind === 'aircraft') {
    if (m.variant !== 'ground_ops' || !m.ground_ops) return null
    const g = m.ground_ops
    const total = (g.observed_movements_per_day ?? 0) + (g.modeled_movements_per_day ?? 0)
    const hasModeled = (g.modeled_movements_per_day ?? 0) > 0.01
    const arr = g.arrivals_per_day ?? 0
    const dep = g.departures_per_day ?? 0
    const gseTotal = (g.gse_per_day ?? [0, 0, 0]).reduce((s, v) => s + v, 0)
    const hasGse = gseTotal > 0.05
    const movementsText = txtTable([
      'Airport ground operations per day.',
      'Observed from ADS-B where visible; the',
      'rest comes from the airport-surface model.',
      '',
      ['Observed', `${fmtFloat(g.observed_movements_per_day)}/day`],
      ...(hasModeled ? [['Modeled', `${fmtFloat(g.modeled_movements_per_day)}/day`] as [string, string]] : []),
      { sep: true },
      ['Total', `${fmtFloat(total)}/day`],
      '',
      'Per direction (unique rotations, set-union dedup):',
      ['  Arrivals', `${fmtFloat(arr)}/day`],
      ['  Departures', `${fmtFloat(dep)}/day`],
      '',
      'Per class:',
      ['  Runway roll', `${fmtFloat((g.runway_roll.observed_movements_per_day ?? 0) + (g.runway_roll.modeled_movements_per_day ?? 0))}/day`],
      ['  Taxi', `${fmtFloat((g.taxi.observed_movements_per_day ?? 0) + (g.taxi.modeled_movements_per_day ?? 0))}/day`],
      ['  Apron', `${fmtFloat((g.apron_movement.observed_movements_per_day ?? 0) + (g.apron_movement.modeled_movements_per_day ?? 0))}/day`],
      ...(hasGse
        ? [
            '' as TableRow,
            'Ground support equipment:' as TableRow,
            ['  Light', `${fmtFloat((g.gse_per_day ?? [0, 0, 0])[0])}/day`] as [string, string],
            ['  Medium', `${fmtFloat((g.gse_per_day ?? [0, 0, 0])[1])}/day`] as [string, string],
            ['  Heavy', `${fmtFloat((g.gse_per_day ?? [0, 0, 0])[2])}/day`] as [string, string],
          ]
        : []),
    ] as TableRow[], 16, 12)
    return (
      <>
        {lineRow(
          'Ground movements',
          <DataPoint title="Airport ground ops per day" text={movementsText}>
            {`${total.toFixed(1)}/day`}
          </DataPoint>,
        )}
        {lineRow(
          'Arr / Dep',
          <DataPoint title="Unique rotations per direction" text={movementsText}>
            <span className="tabular-nums">{`${arr.toFixed(1)} / ${dep.toFixed(1)}/day`}</span>
          </DataPoint>,
        )}
        {hasGse && lineRow(
          'GSE',
          <DataPoint title="Ground support equipment events per day" text={movementsText}>
            <span className="tabular-nums">{`${gseTotal.toFixed(1)}/day`}</span>
          </DataPoint>,
        )}
      </>
    )
  }

  return null
}
