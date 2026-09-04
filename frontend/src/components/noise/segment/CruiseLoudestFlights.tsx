import type { CruiseHexTopFlight } from '../../../types/noise'
import { adsbTraceHref, metersToKm } from '../../../utils/formatters'
import { aircraftFlightTooltip } from '../../../utils/aircraft-types'
import { HoverText } from '../../ui/info-tip'

export function CruiseLoudestFlights({ tops }: { tops: CruiseHexTopFlight[] }) {
  return (
    <div className="mt-2 -mx-1">
      <div className="font-medium mt-2 mb-0.5 text-foreground/70 text-[10px] px-1">
        <HoverText title={"Top flights in this cruise cell\n\nThe loudest individual ADS-B flights that crossed this cruise cell, ranked by peak A-weighted Lmax. Cruise buckets aggregate many flights — this table picks the top 5 by peak.\n\nDate + time are the flight's first ADS-B sample (≈ takeoff time, UTC), not the moment over this cell. Per-cell aggregation drops per-sample timing."}>
          Top flights
        </HoverText>
      </div>
      <table className="w-full text-[10px] [&_th]:pl-2 [&_td]:pl-2 [&_:first-child]:pl-1">
        <thead>
          <tr className="text-muted-foreground/60 [&_th]:font-normal [&_th]:pb-0.5">
            <th className="text-right">
              <HoverText title={"Peak A-weighted SPL during this flyover.\n\nLooked up from per-class LAmax NPD tables. Informational display only — the Lden total uses SEL, not Lmax."}>
                Lmax
              </HoverText>
            </th>
            <th className="text-right">
              <HoverText title={"Aircraft altitude above receiver during peak encounter in this hex.\nDerived from the cruise bucket's representative altitude at the loudest cell crossing."}>
                Alt(km)
              </HoverText>
            </th>
            <th className="text-right">
              <HoverText title={"Date + time (UTC) of the flight's first ADS-B sample.\nThis is ≈ takeoff time, not the moment over this hex — Stage 2B cruise aggregates drop per-sample timing."}>
                Date
              </HoverText>
            </th>
            <th className="text-right">
              <HoverText title={"Aircraft type + identity\n\nCell shows the 4-letter ICAO designator (B738, A320, …); hover for the full model + ICAO 24-bit hex address. Click to open the flight trace on its source network's globe."}>
                Aircraft
              </HoverText>
            </th>
          </tr>
        </thead>
        <tbody>
          {tops.map((f, i) => {
            const typecode = f.aircraft_type || 'Unknown'
            const hex = f.icao_hex || null
            const aircraftTooltipText = aircraftFlightTooltip({
              typecode,
              callsign: f.callsign,
              icaoHex: hex,
            })
            const globeHref = hex && f.date
              ? adsbTraceHref(hex, f.date, { noiseClass: f.class_name, typecode: f.aircraft_type })
              : null
            const dateCell = `${f.date ? f.date.slice(5) : '—'} ${f.time_utc ? f.time_utc.slice(0, 5) : ''}`
            // Synthetic cruise rows (empty hex ⇒ empty date/time per
            // `cruise.rs:316-317`) drop the date HoverText: the bare cell
            // already reads "—" and a "? UTC" tooltip would only confuse.
            const dateContent = f.date
              ? <HoverText title={`${f.date} ${f.time_utc || ''} UTC (flight start)`}>{dateCell}</HoverText>
              : dateCell
            return (
              <tr key={i} className="[&_td]:text-right">
                <td className="font-medium">{f.lmax_db.toFixed(0)}&nbsp;dB</td>
                <td>{metersToKm(f.altitude_m)}</td>
                <td className="text-muted-foreground/80 tabular-nums whitespace-nowrap">
                  {dateContent}
                </td>
                <td className="whitespace-nowrap">
                  <HoverText title={aircraftTooltipText}>
                    {globeHref ? (
                      <a href={globeHref} target="_blank" rel="noopener noreferrer" className="hover:underline">
                        {typecode}
                      </a>
                    ) : (
                      typecode
                    )}
                    {f.callsign && <span className="text-muted-foreground/60"> · {f.callsign}</span>}
                  </HoverText>
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
