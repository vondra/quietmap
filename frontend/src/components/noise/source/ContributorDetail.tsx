import type { Contributor, AircraftMetadata } from '../../../types/noise'
import { fmt, fmtDbValue, fmtFloat, fmtInt, metersToKm, txtTable, type TableRow } from '../../../utils/formatters'
import { aircraftTooltip, classToAnchorTypecode, displayTypecode } from '../../../utils/aircraft-types'
import { MetricLabel, DataPoint } from '../noise-tooltips'
import { HoverText } from '../../ui/info-tip'
import { lineRow, subtypeLabel } from '../shared'
import { TopFlightsTable } from './TopFlightsTable'
import { MetadataRows } from './MetadataRows'

/**
 * The expanded body of a contributor row: the full propagation breakdown + every
 * per-effect tooltip table. Split out of ContributorRow so the ~190 lines of
 * `txtTable` builders below run ONLY when a row is actually expanded — the
 * Sources tab opens with every row collapsed, and building these eagerly for each
 * one was the dominant popup-open cost (mirrors the lazy-Segments pattern).
 */
export function ContributorDetail({ c }: { c: Contributor }) {
  const isAircraft = c.metadata?.kind === 'aircraft'
  const aircraftMeta = isAircraft ? (c.metadata as AircraftMetadata) : null
  const aircraftAirborne = aircraftMeta?.variant === 'airborne' ? (aircraftMeta.airborne ?? null) : null
  const aircraftGroundOps = aircraftMeta?.variant === 'ground_ops' ? (aircraftMeta.ground_ops ?? null) : null

  // Engine truncates to top-3 (`PROFILE_MIX_TOP_N`) WITHOUT
  // renormalizing — shares stay absolute fractions of total
  // ground-ops received energy. When the top-3 leaves a material
  // tail (≥ 5 %), we render an "Other" entry so the chip sums to
  // ~100 % and users don't read 91 % + 6 % + 2 % as a bug.
  // /gg consensus (Gemini + DeepSeek + GPT-5.5).
  const profileMix = aircraftGroundOps?.profile_mix ?? []
  const profileMixPct = (share: number) => `${Math.round(share * 100)}%`
  const profileMixOther = profileMix.length === 0
    ? 0
    : Math.max(0, 1 - profileMix.reduce((s, e) => s + e.share, 0))
  const showOther = Math.round(profileMixOther * 100) >= 5
  const profileMixDisplay: Array<[string, string]> = [
    ...profileMix.map((e) => [displayTypecode(e.rep_typecode), profileMixPct(e.share)] as [string, string]),
    ...(showOther ? [['Other', profileMixPct(profileMixOther)] as [string, string]] : []),
  ]
  const profileMixSummary = profileMix.length === 0
    ? null
    : profileMixDisplay.map(([code, pct]) => `${code} ${pct}`).join(' · ')
  const profileMixText = profileMix.length === 0
    ? null
    : txtTable([
        'Top noise classes by',
        'received energy at this point.',
        '',
        ...profileMixDisplay,
        '',
        'Each class is anchored on a',
        'representative ICAO typecode.',
      ], 18, 8)

  const emissionText = (() => {
    const m = c.metadata
    if (m?.kind === 'aircraft' && m.variant === 'ground_ops' && m.ground_ops) {
      return txtTable([
        'Airport ground operations',
        'Runway roll + taxi + apron',
        '',
        ['Observed', `${fmtFloat(m.ground_ops.observed_movements_per_day)}/day`],
        ['Modeled', `${fmtFloat(m.ground_ops.modeled_movements_per_day)}/day`],
        { sep: true },
        ['Line source', `${c.emission_db.toFixed(1)} dB`],
      ], 18, 14)
    }
    if (m?.kind === 'road') {
      return txtTable([
        'CNOSSOS-EU rolling + propulsion',
        'Speed-dependent vehicle coefficients',
        '',
        ['Speed', `${m.speed_kmh.toFixed(0)} km/h`],
        ['Surface', m.surface],
        ['Vehicle classes', 'L · M · H · Moto'],
        { sep: true },
        ['Line source', `${c.emission_db.toFixed(1)} dB/m`],
      ], 18, 14)
    }
    if (m?.kind === 'rail') {
      return txtTable([
        'CNOSSOS-EU Annex IV (RMR)',
        'Rolling + traction model',
        '',
        ['Speed', `${m.speed_kmh.toFixed(0)} km/h`],
        ['Trains', `${fmtInt(m.trains_passenger_effective + m.trains_freight_effective)}/day`],
        { sep: true },
        ['Line source', `${c.emission_db.toFixed(1)} dB/m`],
      ], 18, 14)
    }
    return txtTable([
      'Area-scaled sound power Lw',
      'Lw = perm2 + 10·log10(footprint)',
      'summed over the source sub-points',
      '',
      { sep: true },
      ['Total Lw', `${c.emission_db.toFixed(1)} dB`],
    ], 20, 12)
  })()

  // Tooltip HEADER must match the body: roads / rails / ground-roll are line
  // sources (CNOSSOS L'w per metre); buildings / industrial / leisure are our
  // area-scaled POINT sources (total Lw — NOT CNOSSOS, which has no building
  // source model; see /about/methodology).
  const emissionTitle = (() => {
    const k = c.metadata?.kind
    if (k === 'road') return 'Road emission — CNOSSOS-EU line source'
    if (k === 'rail') return 'Railway emission — CNOSSOS-EU line source'
    if (k === 'aircraft') return 'Aircraft ground-roll — line source'
    return 'Sound power Lw — area-scaled point source'
  })()

  const baselineText = txtTable([
    ['Geometric divergence', `${fmt(c.baseline.geometric_db)} dB`],
    [`Ground factor G`, c.baseline.ground_factor.toFixed(2)],
    '',
    'Divergence is closest-segment only. For',
    'the energy-weighted effect across all',
    'grouped segments, see the per-effect',
    'rows below (A-weighted ΔL_A).',
  ], 22, 14)

  const atmosphericText = txtTable([
    // Ground-ops distance_m is 0 by design (the source surrounds the
    // receiver), so skip the placeholder "Distance: 0 m (closest)". The
    // A-weighted ΔL_A below is the real per-microsegment atmospheric effect.
    ...((aircraftGroundOps
      ? []
      : [['Distance', `${c.distance_m.toFixed(0)} m (closest)`], { sep: true }]) as TableRow[]),
    ['A-weighted ΔL_A', `${fmt(c.atmospheric_impact_db)} dB`],
    '',
    'ISO 9613-2 §7.2 atmospheric absorption',
    '(humid air, 15 °C, 70 % RH). Energy-',
    'weighted across all grouped segments —',
    "full_lden − no_atmospheric_lden.",
  ], 22, 14)

  const groundText = txtTable([
    // Ground ops are a distributed set of microsegments — no single
    // source-level G, so skip the placeholder "G at closest segment: 0.00".
    // The A-weighted ΔL_A below is the real energy-weighted ground effect.
    ...((aircraftGroundOps
      ? []
      : [[`G at closest segment`, c.baseline.ground_factor.toFixed(2)], { sep: true }]) as TableRow[]),
    ['A-weighted ΔL_A', `${fmt(c.ground_impact_db)} dB`],
    '',
    'ISO 9613-2 §7.3 ground effect.',
    'Signed: over soft ground at 63/125 Hz,',
    'CF[i] < 0 — ground BOOSTS LF energy, so',
    'no_ground can be quieter than full',
    '(positive ΔL_A means ground added dB).',
  ], 22, 14)

  const terrainText = c.terrain.delta_m > 0
    ? txtTable([
        ['Path difference δ', `${c.terrain.delta_m.toFixed(2)} m`],
        ['DEM points', String(c.terrain.profile_points)],
        ['Cadence', 'bilateral 30/60/120/240 m'],
        { sep: true },
        ['A-weighted ΔL_A', `${fmt(c.terrain_impact_db)} dB`],
        '',
        'ISO 9613-2 §7.3 + C₃ frequency term',
        'Copernicus GLO-30 DEM (30 m raster).',
        'Shared bilateral terrain profile — SPEC §4.2.',
      ], 18, 14)
    : txtTable([
        'No terrain obstruction.',
        '',
        'Unified bilateral sampler scanned the',
        'full path (30/60/120/240 m cadence,',
        'dense near source + receiver); no',
        'sample sits above the line of sight.',
      ], 18, 12)

  const screeningText = (() => {
    const rows: Array<[string, string] | { sep: true } | string> = []
    rows.push('Representative propagation edge', 'on the closest-segment/source ray:')
    if (c.screening.obstacle) {
      const edge = c.screening.obstacle.edge
      rows.push(
        ['  Obstacle kind', edge.kind],
        ['  Height', `${edge.height_m.toFixed(1)} m`],
        ['  Position', `${(edge.t * 100).toFixed(0)}% of path`],
        ['  Above LoS', `${edge.screen_h_m.toFixed(1)} m`],
        ['  Fresnel δ', `${c.screening.obstacle.delta_m.toFixed(2)} m`],
        ['  Path cadence', `${c.screening.obstacle.step_m.toFixed(0)} m`],
      )
    } else {
      rows.push(['  Screening', 'no increment'])
    }
    if (c.metadata && (c.metadata.kind === 'road' || c.metadata.kind === 'rail') && c.metadata.segment_count > 1) {
      rows.push('', `Across all ${c.metadata.segment_count} segments:`)
      if (c.metadata.obstacle_segment_count > 0) {
        rows.push(
          ['  With obstacle', `${c.metadata.obstacle_segment_count}/${c.metadata.segment_count}`],
          ['  Avg height', `${c.metadata.obstacle_avg_height_m.toFixed(1)} m`],
          ['  Max height', `${c.metadata.obstacle_max_height_m.toFixed(1)} m`],
          ['  Tallest at segment', `#${c.metadata.obstacle_max_segment_idx}`],
        )
      } else {
        rows.push(['  With obstacle', '0 (all clear)'])
      }
    }
    rows.push({ sep: true }, ['A-weighted ΔL_A', `${fmt(c.screening_impact_db)} dB`])
    rows.push('', 'Each ray retains the maximum attenuation',
      'per band across terrain and exact building', 'or barrier crossings. The edge above is',
      'real, but other edges can supply other', 'bands; it does not explain the whole fan.', '(SPEC §4.7).')
    return txtTable(rows, 22, 14)
  })()

  const vegetationText = c.vegetation.sampled_path_m > 0
    ? txtTable([
        ['Forest depth', `${c.vegetation.forest_depth_m.toFixed(0)} m`],
        ['Path sampled', `${c.vegetation.sampled_path_m.toFixed(0)} m`],
        { sep: true },
        ['A-weighted ΔL_A', `${fmt(c.vegetation_impact_db)} dB`],
        '',
        'WorldCover 30 m raster, sampled via',
        'unified bilateral path profile; forest',
        'runs <10 m discarded (ISO 9613-2 §A.2.2,',
        'capped 200 m). SPEC §4.8.',
      ], 18, 14)
    : "Vegetation skipped\n(segment beyond model's applicable distance)."

  return (
    <div className="mt-1 ml-2 mr-4 mb-1 text-[11px] leading-relaxed font-mono text-muted-foreground">
      {/* Source type — hidden from collapsed row. Traffic / Trains /
          Flights counts are rendered by MetadataRows below, so no
          separate "48 veh/day" line here. */}
      {!isAircraft && c.name && (
        <div className="text-muted-foreground/60 mb-0.5">{subtypeLabel(c.source_type, c.subtype)}</div>
      )}
      {aircraftAirborne ? (
        <>
          {/* Data-source row (+ sparse-sample warning) — MetadataRows'
              aircraft airborne branch renders ONLY that row, so it
              composes here without duplicating the tables below. */}
          <MetadataRows c={c} />
          {/* One total the end-user actually asks for: how many aircraft
              were heard per day. Low airborne approach/departure events +
              identified high-altitude cruise overflights, summed. The Lmax
              bands below are subsets of this union crossing each threshold —
              the old airborne-only headline sat BELOW the >30 dB band count
              and looked broken. Value renders as fixed-wing+helicopter
              addends ("189+1.9") — space-free so the phone popup can never
              wrap it mid-value (owner 2026-07-03). (Dual-phase flights
              counted in both phases; cruise is the identified-loud set, a
              lower bound.) */}
          {lineRow(
            <HoverText title={'Aircraft + helicopter movements heard per day at this point, shown as fixed-wing+helicopter: low airborne approach/departure events plus identified high-altitude cruise overflights, each real flight_id deduped per phase. The Lmax band counts below are subsets of the combined total crossing each threshold. Caveats: a single flight crossing both low and high over this exact point is counted in both phases; cruise transits are the identified-loud set, so that part is a lower bound.'}>
              Aircraft+helicopter (per day)
            </HoverText>,
            // toFixed(0) matches the band counts below; the helicopter share
            // (subset of airborne) is split out as its own addend so the two
            // numbers SUM to the combined total instead of restating it.
            (() => {
              const total = aircraftAirborne.observed_flights_per_day + aircraftAirborne.cruise_transits_per_day
              const heli = aircraftAirborne.helicopter_flights_per_day
              return heli >= 0.05
                ? `${(total - heli).toFixed(0)}+${heli.toFixed(1)}`
                : total.toFixed(0)
            })(),
          )}
          {aircraftAirborne.lmax_peak != null && lineRow('Peak Lmax', `${aircraftAirborne.lmax_peak.toFixed(1)} dB`)}
          {lineRow(
            'Day/Evening/Night',
            `${fmtDbValue(aircraftAirborne.periods.ld_db)}/${fmtDbValue(aircraftAirborne.periods.le_db)}/${fmtDbValue(aircraftAirborne.periods.ln_db)} dB`,
          )}
          {lineRow('Free field', `${fmtDbValue(c.received_lden_free)} dB`)}
          {lineRow('Terrain', `${fmt(c.terrain_impact_db)} dB`)}
          {lineRow('Screening', `${fmt(c.screening_impact_db)} dB`)}
          <table className="w-full text-[10px] mt-1 [&_th]:pl-2 [&_td]:pl-2 [&_:first-child]:pl-0">
            <thead>
              <tr className="text-muted-foreground/60 [&_th]:font-normal [&_th]:pb-0.5">
                <th className="text-left">
                  <HoverText title={"Lmax threshold\n\nPer-event peak A-weighted SPL band looked up from per-class LAmax NPD tables (EASA ANP v2.3 where available, generated SEL−12 fallback for manual GA / helicopter profiles).\nA flight is counted in this band if its Lmax at this point exceeds the threshold."}>Lmax</HoverText>
                </th>
                <th className="text-right">
                  <HoverText title={"Observed flights per day\n\nSegments contributing to this Lmax band, divided by n_days from the ADS-B archive (currently 365)."}>Flights/day</HoverText>
                </th>
                <th className="text-right">
                  <HoverText title={"Mean aircraft altitude AMSL in this band.\nLow values indicate approach/departure traffic; high values indicate en-route cruise."}>Avg alt(km)</HoverText>
                </th>
                <th className="text-right">
                  <HoverText title={"Dominant aircraft type in this band — anchor typecode of the dominant noise class, e.g. B738. 'Average NPD' = the traffic-weighted energy-mean class (no per-typecode profile)."}>Aircraft</HoverText>
                </th>
              </tr>
            </thead>
            <tbody>
              {[
                { label: '>60 dB', bucket: aircraftAirborne.disruptive, color: '#ef4444' },
                { label: '>45 dB', bucket: aircraftAirborne.audible, color: '#f59e0b' },
                { label: '>30 dB', bucket: aircraftAirborne.faint, color: '#6b7280' },
              ].map(({ label, bucket, color }) => {
                if (bucket.observed_events_per_day <= 0) return null
                const anchor = classToAnchorTypecode(bucket.top_aircraft)
                // `classToAnchorTypecode` already maps WING_FALLBACK → 'Average NPD',
                // so the explicit re-mapping that used to live here is gone.
                return (
                  <tr key={label} className="[&_td]:text-right">
                    <td style={{ color }} className="font-medium !text-left">{label}</td>
                    <td>{bucket.observed_events_per_day.toFixed(0)}</td>
                    <td>{metersToKm(bucket.avg_altitude_m)}</td>
                    <td>
                      <HoverText title={aircraftTooltip(anchor, bucket.top_aircraft)}>
                        {anchor}
                      </HoverText>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
          {aircraftAirborne.top_flights && <TopFlightsTable flights={aircraftAirborne.top_flights} detailed />}
        </>
      ) : (
        <>
          <MetadataRows c={c} />
          {lineRow(
            <MetricLabel term="emission" />,
            <DataPoint title={emissionTitle} text={emissionText}>
              {c.emission_db.toFixed(1)} dB
            </DataPoint>,
          )}
          {profileMixSummary && profileMixText && lineRow(
            'Top types',
            <DataPoint title="Profile mix at receiver" text={profileMixText}>
              <span className="tabular-nums">{profileMixSummary}</span>
            </DataPoint>,
          )}
          <div className="mt-1.5 mb-0.5 pt-1 border-t border-border/40">
            <div className="text-[9px] uppercase tracking-[0.08em] text-muted-foreground/70">
              Sound path
            </div>
          </div>
          {/* Ground ops surround the receiver (distance_m = 0 by design),
              so there's no meaningful single source-level geometric
              divergence — the contributor baseline is a placeholder 0.
              Per-microsegment divergence is shown in the Segments tab.
              Suppress the row here rather than display a bogus "0.0 dB"
              (the terrain / screening / vegetation / ground impacts below
              are real, mirrored from the ground-ops metadata). */}
          {!aircraftGroundOps && lineRow(
            <MetricLabel term="baseline" />,
            <DataPoint title="Baseline propagation breakdown" text={baselineText}>
              {fmt(c.baseline.geometric_db)} dB
            </DataPoint>,
          )}
          {lineRow(
            'Atmospheric',
            <DataPoint title="Atmospheric absorption (A-weighted)" text={atmosphericText}>
              <span className={c.atmospheric_impact_db < -0.05 ? '' : 'text-muted-foreground/40'}>
                {fmt(c.atmospheric_impact_db)} dB
              </span>
            </DataPoint>,
          )}
          {lineRow(
            aircraftGroundOps ? 'Ground' : `Ground (G=${c.baseline.ground_factor.toFixed(1)})`,
            <DataPoint title="Ground effect (signed A-weighted ΔL_A)" text={groundText}>
              <span className={Math.abs(c.ground_impact_db) < 0.05 ? 'text-muted-foreground/40' : ''}>
                {fmt(c.ground_impact_db)} dB
              </span>
            </DataPoint>,
          )}
          {lineRow(
            <MetricLabel term="terrain" />,
            <DataPoint title="Terrain diffraction" text={terrainText}>
              <span className={c.terrain_impact_db < -0.5 ? '' : 'text-muted-foreground/40'}>
                {fmt(c.terrain_impact_db)} dB
              </span>
            </DataPoint>,
          )}
          {lineRow(
            <MetricLabel term="screening" />,
            <DataPoint title="Screening obstacle" text={screeningText}>
              <span className={c.screening_impact_db < -0.5 ? '' : 'text-muted-foreground/40'}>
                {fmt(c.screening_impact_db)} dB
              </span>
            </DataPoint>,
          )}
          {lineRow(
            <MetricLabel term="vegetation" />,
            <DataPoint title="Vegetation attenuation" text={vegetationText}>
              <span className={c.vegetation_impact_db < -0.5 ? '' : 'text-muted-foreground/40'}>
                {fmt(c.vegetation_impact_db)} dB
              </span>
            </DataPoint>,
          )}
        </>
      )}

      {!aircraftAirborne && c.received_bands && c.received_bands.length === 8 && c.received_bands.some(b => b !== 0) && (
        <div className="mt-1 text-[10px] text-muted-foreground/60">
          <MetricLabel term="per_band">
            [{c.received_bands.map(b => Math.round(b)).join(' ')}]
          </MetricLabel>
        </div>
      )}
    </div>
  )
}
