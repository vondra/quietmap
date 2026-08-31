import type { AircraftTopFlight } from '../../../types/noise'
import { adsbTraceHref, metersToKm, unixToIsoDate, unixToIsoDateTimeUtc } from '../../../utils/formatters'
import { aircraftFlightTooltip, displayTypecode, parseProfileName } from '../../../utils/aircraft-types'
import { HoverText } from '../../ui/info-tip'
import { PERIOD_LABELS, PERIOD_LABELS_DETAIL, PERIOD_TOOLTIP } from '../shared'

const PERIOD_COLORS: Record<number, string | undefined> = { 2: '#818cf8', 1: '#f59e0b' }

export function TopFlightsTable({ flights, detailed }: { flights: AircraftTopFlight[]; detailed?: boolean }) {
  if (!flights.length) return null
  return (
    <>
      <div className="font-medium mt-2 mb-0.5 text-foreground/70 text-[10px]">
        {detailed ? (
          <HoverText title={"Top flights by Lmax\n\nThe loudest individual ADS-B flights ranked by peak A-weighted Lmax at this point — interleaves both low-altitude airborne (approach / departure) events and high-altitude cruise overflights. Each row is one unique flight observation.\n\nThe `energy %` column is the flight's share of total Lden energy and is only computed for airborne entries; cruise rows show 0 because the bucket aggregates many real flights.\n\nUseful for diagnosing why noise is unexpectedly high — e.g. a single low-altitude night flight dominating total energy."}>
            Top flights
          </HoverText>
        ) : 'Top flights'}
      </div>
      <table className="w-full text-[10px] [&_th]:pl-2 [&_td]:pl-2 [&_:first-child]:pl-0">
        <thead>
          <tr className="text-muted-foreground/60 [&_th]:font-normal [&_th]:pb-0.5">
            <th className="text-right">
              {detailed ? <HoverText title={"Peak A-weighted SPL during this flyover.\n\nLooked up from per-class LAmax NPD tables — EASA ANP v2.3 LAmax curves where available, generated SEL−12 fallback otherwise (manual GA / helicopter profiles, ANP entries without LAmax). Informational display only — the Lden total uses SEL, not Lmax.\n\nFull Doc 29 Eq. 4-12 also applies ΔI / Λ per segment; this display estimate skips those (< 2 dB residual)."}>Lmax</HoverText> : 'Lmax'}
            </th>
            <th className="text-right">
              {detailed ? <HoverText title={"Closest Point of Approach — shortest 3D slant distance from the flight track to this receiver.\nComputed on the infinite line extension of the segment (Doc 29 §4.4.1).\nSmaller CPA = louder."}>CPA(km)</HoverText> : 'CPA(km)'}
            </th>
            <th className="text-right">
              {detailed ? <HoverText title={"Aircraft altitude above receiver at the closest point of approach.\nDerived from ADS-B barometric altitude minus receiver ground elevation.\nVery low values (<0.10 km) may indicate ADS-B altitude glitches."}>Alt(km)</HoverText> : 'Alt(km)'}
            </th>
            <th className="text-right">
              {detailed ? <HoverText title={`Date & period\n\n${PERIOD_TOOLTIP}`}>Date</HoverText> : 'Date'}
            </th>
            <th className="text-right">
              {detailed ? <HoverText title={"Aircraft type + identity. Cell shows the 4-letter ICAO designator (B738, A320, …) or 'Average NPD' if no real typecode was carried in ADS-B. Click to open the flight trace on its source network's globe (adsb.lol for GA/heli, adsbexchange for airliners)."}>Aircraft</HoverText> : 'Aircraft'}
            </th>
            <th className="text-right">
              {detailed ? <HoverText title={"Energy share (%)\n\nThis flight's contribution to total airborne Lden energy.\n100% = this single flight causes all airborne noise.\nEnergy is in linear (not dB) scale, so a flight with 90%\ndominates even if other flights have similar Lmax."}>%</HoverText> : '%'}
            </th>
          </tr>
        </thead>
        <tbody>
          {flights.map((f, i) => {
            const periodLetter = (PERIOD_LABELS[f.period] ?? '?').charAt(0)
            const periodColor = PERIOD_COLORS[f.period]
            const dateShort = f.date ? f.date.slice(5) : ''
            // Backend `aircraft_type` (extract-time ICAO typecode) is the
            // canonical source. Fall back to the profile-anchor name when
            // typecode metadata was missing or pre-v11 reader.
            const profileTypecode = parseProfileName(f.profile)
            const rawTypecode = f.aircraft_type && f.aircraft_type.length > 0
              ? f.aircraft_type
              : profileTypecode
            const typecodeDisplay = displayTypecode(rawTypecode)
            const isSynth = f.synthetic
            const icaoHex = f.icao_hex ? f.icao_hex.toUpperCase() : null
            const exactTime = f.start_unix != null ? unixToIsoDateTimeUtc(f.start_unix) : null
            const dateTooltip = exactTime
              ? `${exactTime} (flight start)\n${PERIOD_LABELS_DETAIL[f.period] ?? '?'}`
              : `${f.date}\n${PERIOD_LABELS_DETAIL[f.period] ?? '?'}`
            // `synthetic: isSynth` is the authoritative signal; the helper
            // only consults `icaoHex` as a fallback. Pass the raw hex —
            // helper handles the synth-suppression branch internally.
            const aircraftTooltipText = aircraftFlightTooltip({
              typecode: rawTypecode,
              callsign: f.callsign,
              icaoHex,
              synthetic: isSynth,
            })
            // `start_unix` is the flight's first ADS-B sample (≈ takeoff
            // UTC). The globe indexes traces by takeoff date, so an
            // overnight flight peaking after 00:00 UTC would deep-link to
            // the wrong day if we used `f.date` (peak overflight UTC date).
            const traceDate = f.start_unix != null
              ? unixToIsoDate(f.start_unix)
              : f.date
            const globeHref = icaoHex && !isSynth && traceDate
              ? adsbTraceHref(icaoHex, traceDate, { typecode: rawTypecode })
              : null
            return (
              <tr key={i} className="[&_td]:text-right">
                <td className="font-medium">{f.lmax_db.toFixed(0)}&nbsp;dB</td>
                <td>{metersToKm(f.cpa_distance_m)}</td>
                <td>{metersToKm(f.altitude_m)}</td>
                <td className="whitespace-nowrap" style={periodColor ? { color: periodColor } : undefined}>
                  {detailed ? (
                    <HoverText title={dateTooltip}>
                      {dateShort} {periodLetter}
                    </HoverText>
                  ) : `${dateShort} ${periodLetter}`}
                </td>
                <td className="whitespace-nowrap">
                  <HoverText title={aircraftTooltipText}>
                    {globeHref ? (
                      <a href={globeHref} target="_blank" rel="noopener noreferrer" className="hover:underline">
                        {typecodeDisplay}
                      </a>
                    ) : (
                      typecodeDisplay
                    )}
                  </HoverText>
                </td>
                <td className="text-muted-foreground/60">{f.energy_pct.toFixed(0)}%</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </>
  )
}
