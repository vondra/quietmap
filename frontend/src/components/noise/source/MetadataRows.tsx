import type { Contributor } from '../../../types/noise'
import { fmt, fmtFloat, fmtInt, fmtCompact, txtTable, type TableRow } from '../../../utils/formatters'
import { MetricLabel, DataPoint } from '../noise-tooltips'
import { formatProv, lineRow, railTrafficLabel, railTrafficDescription, roadCategoryEstimated, roadTrafficSourceLine, sourceHost, subtypeLabel } from '../shared'

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
    const total = m.aadt_light + m.aadt_medium + m.aadt_heavy + m.aadt_moto
    // The headline is the whole road; the classes below are the dominant carriageway's.
    const wholeRoad = m.cross_section_aadt > 0
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
    const trafficText = txtTable([
      roadTrafficSourceLine(m.provenance),
      '',
      ...(wholeRoad
        ? [['Whole road', `${fmtInt(Math.round(m.cross_section_aadt))}/day`] as [string, string], 'both directions', '']
        : total === 0
          ? ['This carriageway carries no traffic.', '']
          : ['Only this direction is known.', '']),
      'This carriageway:',
      ...([['Light', m.aadt_light, 1], ['Medium', m.aadt_medium, 2], ['Heavy', m.aadt_heavy, 4], ['Moto', m.aadt_moto, 8]] as const)
        .map(([label, value, bit]) =>
          [label, roadCategoryEstimated(m, bit) ? `${fmtInt(Math.round(value))} (est.)` : `${fmtInt(Math.round(value))}`] as [string, string],
        ),
      { sep: true },
      ['Total', `${fmtInt(Math.round(total))}/day`] as [string, string],
      '',
      'Counts are prepared per vehicle class:',
      'a counted value is an observation, an',
      '"(est.)" value is an estimate or class',
      'prior from the build.',
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
    const timing = m.time_profile_attribution
    // Timing attribution is dominant-segment scoped, mirroring every other
    // metadata row; the "k of N" line keeps a mixed group honest instead of
    // claiming one profile for all segments.
    const timingText = timing
      ? txtTable(
          [
            'Observed day/evening/night traffic timing',
            'from counting stations (dominant segment).',
            ['Source', timing.source],
            ['Window', timing.window.replace('..', '–')],
            '',
            ...(timing.total_transfer
              ? [
                  'Vehicle-class timing estimated:',
                  'total-vehicle shares transferred where',
                  'no class observation exists.',
                ]
              : ['Unmeasured vehicle classes keep the', 'standard default split.']),
            ...((m.profiled_segment_count ?? 0) < m.segment_count
              ? [
                  '' as TableRow,
                  `Group: ${m.profiled_segment_count} of ${m.segment_count} segments`,
                  'carry observed timing.',
                ]
              : []),
          ] as TableRow[],
          18,
          12,
        )
      : ''
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
          <DataPoint title={wholeRoad ? 'Daily traffic on the whole road, both directions' : 'Daily traffic in this direction'} text={trafficText}>
            {wholeRoad ? `${fmtCompact(Math.round(m.cross_section_aadt))}/day` : `${fmtCompact(Math.round(total))}/day · one direction`}
          </DataPoint>,
        )}
        {timing &&
          lineRow(
            'Timing',
            <DataPoint title="Observed traffic timing" text={timingText}>
              <span className="whitespace-normal">
                <a
                  href={timing.source}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="hover:underline"
                >
                  {sourceHost(timing.source)}
                </a>
                {` · ${timing.window.replace('..', '–')}`}
                {timing.total_transfer ? ' · vehicle-class timing estimated' : ''}
              </span>
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

  if (m.kind === 'ship') {
    const [large, work, leisure] = m.hours_per_month
    const prov = m.provenance
    const cellText = txtTable([
      ['Loudest', m.source_type.replace(/_/g, ' ')],
      ['Source', prov ? formatProv(prov) : 'AIS vessel density'],
      ['Cell', `${(m.area_m2 / 1e6).toFixed(2)} km²`],
      ['Large ships', `${large.toFixed(1)} h/month`],
      ['Work boats', `${work.toFixed(1)} h/month`],
      ['Leisure craft', `${leisure.toFixed(1)} h/month`],
      '', 'Mean vessel-hours per month in this', 'water cell, from AIS positions;', 'ships radiate around the clock.',
    ], 16, 16)
    return lineRow(
      'Ships',
      <DataPoint title="Ship traffic cell" text={cellText}>
        {`${(large + work + leisure).toFixed(0)} h/month`}
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
    // Every class: adsb.lol baseline days plus ADSBexchange increment days
    // that add only the traffic adsb.lol did not receive.
    const nDays = a.sample_days
    const incrementDays = a.increment_sample_days ?? 0
    const basisLine = incrementDays > 0
      ? `adsb.lol ${nDays ?? '–'} d/yr + adsbexchange ${incrementDays} d/yr for what adsb.lol missed.`
      : `Lden averaged from ${nDays ?? '–'} adsb.lol days/yr.`
    const sampleText = txtTable([
      'ADS-B flight tracks (adsb.lol + adsbexchange).',
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
    const badge = incrementDays > 0 ? `${nDays ?? '–'}+${incrementDays} d/yr` : `${nDays ?? '–'} days/yr`
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
