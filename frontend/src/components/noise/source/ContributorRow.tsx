import { useState } from 'react'
import type { Contributor, AircraftMetadata } from '../../../types/noise'
import { ldenToColor } from '../../../utils/noise-colors'
import { fmt, fmtDb, txtTable } from '../../../utils/formatters'
import { DataPoint } from '../noise-tooltips'
import { formatDist, subtypeLabel } from '../shared'
import { ContributorDetail } from './ContributorDetail'

export function ContributorRow({ c, onToggle }: { c: Contributor; onToggle?: (geometry: any | null) => void }) {
  const [expanded, setExpanded] = useState(false)
  const isAircraft = c.metadata?.kind === 'aircraft'
  const aircraftMeta = isAircraft ? (c.metadata as AircraftMetadata) : null
  const aircraftAirborne = aircraftMeta?.variant === 'airborne' ? (aircraftMeta.airborne ?? null) : null
  const aircraftGroundOps = aircraftMeta?.variant === 'ground_ops' ? (aircraftMeta.ground_ops ?? null) : null

  // The only tooltip the COLLAPSED row needs (on the dB badge). The rest of the
  // breakdown — ~190 lines of per-effect tooltip tables — is built lazily inside
  // ContributorDetail, which mounts only when the row is expanded.
  const ldenBreakdownText = aircraftAirborne
    ? txtTable([
        ['Free field', `${c.received_lden_free.toFixed(1)} dB`],
        ['Terrain', `${fmt(c.terrain_impact_db)} dB`],
        ['Screening', `${fmt(c.screening_impact_db)} dB`],
        { sep: true },
        ['Day (07–19)', fmtDb(aircraftAirborne.periods.ld_db)],
        ['Evening (19–23)', fmtDb(aircraftAirborne.periods.le_db)],
        ['Night (23–07)', fmtDb(aircraftAirborne.periods.ln_db)],
        { sep: true },
        ['→ Final Lden', fmtDb(aircraftAirborne.periods.lden_db)],
      ], 14, 9)
    : txtTable([
        ['Free field', `${c.received_lden_free.toFixed(1)} dB`],
        ['Terrain', `${fmt(c.terrain_impact_db)} dB`],
        ['Screening', `${fmt(c.screening_impact_db)} dB`],
        ['Vegetation', `${fmt(c.vegetation_impact_db)} dB`],
        { sep: true },
        ['→ Final Lden', `${c.received_lden.toFixed(1)} dB`],
      ], 14, 9)

  return (
    <div className="border-b border-border/50 last:border-b-0">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={(e) => {
          e.stopPropagation()
          const next = !expanded
          setExpanded(next)
          onToggle?.(next ? c.geometry : null)
        }}
        className="w-full py-1.5 text-left cursor-pointer hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <div className="flex items-baseline gap-1.5 text-xs px-0">
          <span className="font-medium truncate flex-1">
            {isAircraft
              ? (c.name || (aircraftAirborne ? 'Airborne aircraft' : 'Ground operations'))
              : (c.name || subtypeLabel(c.source_type, c.subtype))}
          </span>
          {(!isAircraft || aircraftGroundOps) ? (
            <span className="text-muted-foreground/60 shrink-0 w-14 text-right tabular-nums">
              {formatDist(c.distance_m)}
            </span>
          ) : (
            <span className="shrink-0 w-14" aria-hidden="true" />
          )}
          <span
            className="font-bold shrink-0 w-14 text-right tabular-nums"
            style={{ color: ldenToColor(c.received_lden) }}
          >
            <DataPoint title="Lden breakdown" text={ldenBreakdownText}>
              {c.received_lden.toFixed(1)} dB
            </DataPoint>
          </span>
          <span className="text-[10px] text-muted-foreground/40 shrink-0">
            {expanded ? '▲' : '▼'}
          </span>
        </div>
      </button>

      {expanded && <ContributorDetail c={c} />}
    </div>
  )
}
