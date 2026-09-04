import type { SegmentTrace } from '../../../types/noise'
import { adsbTraceHref, unixToIsoDate, unixToIsoDateTimeUtc } from '../../../utils/formatters'
import {
  aircraftFlightTooltip,
  aircraftTooltip,
  classToAnchorTypecode,
  displayTypecode,
  modelName,
} from '../../../utils/aircraft-types'
import { HoverText } from '../../ui/info-tip'
import { roadSourceDescription, railTrainSourceLine } from '../shared'
import { fmtDbSigned } from './display'

export function emissionInputRows(t: SegmentTrace): [React.ReactNode, React.ReactNode][] {
  const e = t.emission
  switch (e.kind) {
    case 'road': {
      const total = e.aadt_light + e.aadt_medium + e.aadt_heavy + e.aadt_moto
      const trafficText =
        roadSourceDescription(e.traffic_source, e.provenance, e.road_class) +
        `\n\n` +
        `Daily traffic (per OSM way):\n` +
        (e.aadt_light > 0 ? `  Light    ${Math.round(e.aadt_light)}\n` : '') +
        (e.aadt_medium > 0 ? `  Medium   ${Math.round(e.aadt_medium)}\n` : '') +
        (e.aadt_heavy > 0 ? `  Heavy    ${Math.round(e.aadt_heavy)}\n` : '') +
        (e.aadt_moto > 0 ? `  Moto     ${Math.round(e.aadt_moto)}\n` : '') +
        `  ──────────────\n  Total    ${Math.round(total)}/day`
      const surfaceText =
        `Source: OSM surface=${e.surface}\n\n` +
        `CNOSSOS Annex II rolling-noise correction relative to the standard\n` +
        `asphalt baseline (0 dB). ${e.surface} → ${fmtDbSigned(e.surface_corr_db)}.`
      return [
        ['Speed', `${e.speed_kmh.toFixed(0)} km/h`],
        [
          'Traffic',
          <HoverText title={trafficText}>
            {Math.round(total)}/day
          </HoverText>,
        ],
        [
          'Surface',
          <HoverText title={surfaceText}>
            {e.surface}
            {e.surface_corr_db !== 0 ? ` (${fmtDbSigned(e.surface_corr_db)})` : ''}
          </HoverText>,
        ],
        ['Class', e.road_class],
        [
          'Flags',
          `${e.oneway ? 'oneway' : 'two-way'} · ${e.lanes} lanes${e.bridge ? ' · bridge' : ''}${
            e.tunnel ? ' · tunnel' : ''
          }`,
        ],
      ]
    }
    case 'railway': {
      const total = e.trains_passenger + e.trains_freight
      // Label on its own line so long dataset names can never collide with
      // the prefix and trigger mid-word wraps.
      const paxSrc = e.trains_passenger > 0
        ? `Passenger source:\n  ${railTrainSourceLine(e.trains_passenger_source, e.provenance, e.rail_type).split('\n').join('\n  ')}\n\n`
        : ''
      const frtSrc = e.trains_freight > 0
        ? `Freight source:\n  ${railTrainSourceLine(e.trains_freight_source, e.provenance, e.rail_type).split('\n').join('\n  ')}\n\n`
        : ''
      const countLines =
        (e.trains_passenger > 0 ? `  Passenger  ${e.trains_passenger.toFixed(1).padStart(6)}\n` : '') +
        (e.trains_freight > 0 ? `  Freight    ${e.trains_freight.toFixed(1).padStart(6)}\n` : '')
      const trainsText =
        paxSrc +
        frtSrc +
        `Daily trains (per track):\n` +
        countLines +
        `  ──────────────\n  Total      ${total.toFixed(1).padStart(6)}/day`
      return [
        ['Speed', `${e.speed_kmh.toFixed(0)} km/h`],
        [
          'Trains',
          <HoverText title={trainsText}>
            {total.toFixed(1)}/day
          </HoverText>,
        ],
        [
          'Type',
          `${e.rail_type}${e.highspeed ? ' · highspeed' : ''}${e.service ? ' · service' : ''}`,
        ],
        ['Bridge', e.bridge ? 'yes' : 'no'],
      ]
    }
    case 'aircraft_ground': {
      // Per-microsegment movements + class mix come from the Stage 2C
      // `flight_ids` set aggregated across all rows at this
      // `(osm_id, segment_idx)`. The arr/dep split is meaningful ONLY
      // for runway-roll microsegments (taxi/apron rows always have
      // is_departure=0 by Stage 2C convention).
      const rows: [React.ReactNode, React.ReactNode][] = []
      const classLabel =
        `${e.class}${t.length_m > 0 ? ` · ${Math.round(t.length_m)} m` : ''}`
      rows.push(['Class', classLabel])
      if (e.osm_ref) {
        rows.push(['OSM ref', e.osm_ref])
      }
      const obs = e.observed_movements ?? 0
      if (obs > 0) {
        // Hover shows arr/dep split (only meaningful for runway-roll;
        // taxi/apron always show 0 split because their is_departure
        // is always 0 by Stage 2C contract).
        const arr = e.arrivals_per_day ?? 0
        const dep = e.departures_per_day ?? 0
        const splitText =
          e.class === 'runway'
            ? `Runway-roll split:\n  Arrivals    ${arr.toFixed(1)}/day\n  Departures  ${dep.toFixed(1)}/day\n\nUnique rotations that crossed this microsegment. Touch semantics\n(Stage 2C v4) — each rotation appears in every microsegment its\nground trajectory intersected. Airport-aggregate totals UNION\nacross microsegment rows for correct deduplication.`
            : `${e.class === 'taxi' ? 'Taxi' : 'Apron'} movements through this microsegment.\nNo arr/dep split — Stage 2C only carries direction\nfor runway-roll rows (is_departure=0 elsewhere).`
        const headline =
          e.class === 'runway'
            ? `${arr.toFixed(1)} arr / ${dep.toFixed(1)} dep`
            : `${obs.toFixed(1)}/day`
        rows.push([
          'Movements',
          <HoverText title={splitText}>{headline}</HoverText>,
        ])
      }
      // Top aircraft classes by energy share at this microsegment.
      // Headline shows ICAO typecodes (A320 / B738 / B38M) the same
      // way TopFlightsTable does; the hover tooltip names the model
      // via `aircraftTooltip` so the user never sees an internal
      // class index.
      if (e.class_mix && e.class_mix.length > 0) {
        const tooltipLines = [
          'Aircraft class mix on this microsegment',
          '(% of A-weighted received energy)',
          '',
        ]
        for (const c of e.class_mix) {
          const display = displayTypecode(c.rep_typecode)
          tooltipLines.push(
            `  ${display.padEnd(11)}  ${modelName(c.rep_typecode)}  ${(c.share * 100).toFixed(1)}%`,
          )
        }
        tooltipLines.push('')
        tooltipLines.push(
          'Each row = the class representative ICAO typecode (Voronoi-',
        )
        tooltipLines.push(
          'anchored from EASA ANP v2.3) plus its energy share at this',
        )
        tooltipLines.push('microsegment.')
        // InlineTable values are whitespace-nowrap (dB numbers must not
        // split from their unit) — this long mix opts back out so it can
        // wrap into a second line instead of overflowing the 320 px panel.
        const headline = (
          <span className="whitespace-normal">
            {e.class_mix.slice(0, 3).map((c, i) => (
              <span key={c.rep_typecode}>
                {i > 0 && ' · '}
                <HoverText title={`${modelName(c.rep_typecode)} (${c.rep_typecode})`}>
                  {displayTypecode(c.rep_typecode)}
                </HoverText>
                {` ${(c.share * 100).toFixed(0)}%`}
              </span>
            ))}
          </span>
        )
        rows.push([
          <HoverText title={tooltipLines.join('\n')}>Top types</HoverText>,
          headline,
        ])
      }
      const gse = e.gse_per_day ?? [0, 0, 0]
      const gseTotal = gse[0] + gse[1] + gse[2]
      if (gseTotal > 0) {
        const tooltip =
          'Ground Support Equipment (GSE) movements / day\n' +
          'on this microsegment, split by class:\n\n' +
          `  LIGHT   ${gse[0].toFixed(1)}/day\n` +
          `  MEDIUM  ${gse[1].toFixed(1)}/day\n` +
          `  HEAVY   ${gse[2].toFixed(1)}/day\n\n` +
          'Stage 2C routes veh_kind=1 rows here.'
        rows.push([
          'GSE',
          <HoverText title={tooltip}>{gseTotal.toFixed(1)}/day</HoverText>,
        ])
      }
      return rows
    }
    case 'aircraft_airborne': {
      // Callsign is the discoverable click target — users recognize callsigns
      // ("TVS100P") more readily than 6-hex ICAO addresses, so the globe.adsbexchange.com
      // trace link hangs off the callsign value and the ICAO hex lives in the
      // hover tooltip instead of consuming a row of its own. Synthetic (empty
      // icao_hex) fids stay plain text — no transponder hex = no globe deep-link.
      // Missing callsign with present hex falls back to the hex itself as link
      // text so the clickable target is never a bare em-dash placeholder.
      // Tooltip uses `aircraftFlightTooltip` so the airborne row sees the
      // same identity card (model + class + NPD + ICAO + click hint) as
      // the two top-flight tables — no hand-rolled string drift.
      let callsignValue: React.ReactNode = e.callsign || '—'
      if (e.icao_hex) {
        const linkText = e.callsign || e.icao_hex.toUpperCase()
        const date = e.start_unix != null ? unixToIsoDate(e.start_unix) : null
        const tooltip = aircraftFlightTooltip({
          typecode: e.aircraft_type,
          callsign: e.callsign,
          icaoHex: e.icao_hex,
        })
        callsignValue = (
          <HoverText title={tooltip}>
            <a href={adsbTraceHref(e.icao_hex, date, { typecode: e.aircraft_type })} target="_blank" rel="noopener noreferrer" className="hover:underline">
              {linkText}
            </a>
          </HoverText>
        )
      }
      const aglTooltip =
        'AGL at CPA = Above Ground Level at the Closest Point of\n' +
        'Approach. Aircraft altitude minus receiver DEM elevation.\n' +
        'Positive = aircraft above receiver; negative = aircraft\n' +
        'below receiver standing surface (e.g. hill receiver above a\n' +
        'sea-level runway). CPA foot is the Doc 29 §4.4.1 infinite-\n' +
        'line projection; may be extrapolated outside the sub-segment\n' +
        'endpoints when the geometry requires.'
      const cpaTooltip =
        'CPA distance = Closest Point of Approach. The shortest\n' +
        'straight-line distance from the receiver to the aircraft\n' +
        "along this sub-segment's flight path (Doc 29 §4.4.1)."
      const direction = e.is_departure ? 'Departure ↑' : 'Arrival ↓'
      const directionTooltip = e.is_departure
        ? 'Departure: Stage 2A classifier saw ROCD median > +500 fpm at this sub-segment.'
        : 'Arrival: Stage 2A classifier saw non-departure ROCD (descent or level cruise).'
      const typecode = e.aircraft_type || ''
      const aircraftDisplay = typecode ? modelName(typecode) : '—'
      const aircraftHoverTooltip = typecode
        ? `ICAO designator: ${typecode}\n\n${aircraftTooltip(typecode, e.class)}`
        : 'No ICAO aircraft typecode broadcast by this transponder.'
      const classDisplay = classToAnchorTypecode(e.class)
      const classHoverTooltip =
        classDisplay === 'Average NPD'
          ? 'No per-typecode NPD profile — energy-mean of 123 EASA ANP v2.3 profiles used.'
          : `Aircraft noise class; anchored on ${classDisplay}'s ANP NPD curve.`
      // Flight start (UTC) — surfaced so repeated rows for the same
      // physical flight (same callsign + typecode + start_unix, different
      // CPA sub-segs along the trajectory) are visibly distinguishable
      // from two distinct flights of the same aircraft type. Synthetic
      // fids carry no real timestamp, so the row is suppressed there.
      const flightStartTooltip =
        'Flight start (UTC) — Unix epoch of the first ADS-B fix observed for this transponder.\n' +
        'Two segment rows with the same Callsign + Aircraft + Flight start are the same\n' +
        'physical flight; the differing dB / CPA values come from distinct sub-segments\n' +
        'along the trajectory.'
      const flightStartCell = e.start_unix != null
        ? <HoverText title={flightStartTooltip}>{unixToIsoDateTimeUtc(e.start_unix)}</HoverText>
        : null
      return [
        ['Callsign', callsignValue],
        [
          'Aircraft',
          <HoverText title={aircraftHoverTooltip}>{aircraftDisplay}</HoverText>,
        ],
        ...(flightStartCell ? [['Flight start', flightStartCell] as [string, React.ReactNode]] : []),
        [<HoverText title={directionTooltip}>Direction</HoverText>, direction],
        [
          <HoverText title={classHoverTooltip}>Class</HoverText>,
          classDisplay,
        ],
        [
          <HoverText title={cpaTooltip}>CPA distance</HoverText>,
          `${Math.round(e.cpa_distance_m)} m`,
        ],
        [<HoverText title={aglTooltip}>AGL at CPA</HoverText>, `${Math.round(e.altitude_m_at_cpa)} m`],
      ]
    }
    case 'aircraft_cruise': {
      return [
        ['R7 hex', e.r7_hex],
        ['Unique flights', `${e.n_unique_flights}`],
      ]
    }
    case 'building': {
      return [
        ['Type', e.building_type],
        ['Height', `${e.height_m.toFixed(1)} m (${e.floors || '—'} floors)`],
        ['Footprint', e.area_m2 > 0 ? `${Math.round(e.area_m2)} m²` : '—'],
      ]
    }
    case 'industrial': {
      const rows: [React.ReactNode, React.ReactNode][] = [
        ['Source type', e.source_type],
        ['Area', e.area_m2 > 0 ? `${Math.round(e.area_m2)} m²` : '—'],
        ['NACE', e.nace ?? '—'],
      ]
      // Wind-turbine-only fields — non-wind industrials send null/0, row
      // adds no value there.
      if (e.hub_height_m != null && e.hub_height_m > 0) {
        rows.push(['Hub height', `${e.hub_height_m.toFixed(1)} m`])
      }
      if (e.rated_power_kw != null && e.rated_power_kw > 0) {
        rows.push(['Rated power', `${Math.round(e.rated_power_kw)} kW`])
      }
      rows.push(['Effective source dist', `${e.effective_area_source_dist_m.toFixed(0)} m`])
      return rows
    }
  }
}
