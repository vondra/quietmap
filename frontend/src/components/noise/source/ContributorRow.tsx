// One contributor of the popup: label, distance and level; expands to its details.
import { useState } from 'react'
import type { Contributor } from '../../../types/noise'
import { ldenToColor } from '../../../utils/noise-colors'
import { fmtDb, txtTable } from '../../../utils/formatters'
import { DataPoint } from '../noise-tooltips'
import { contributorLabel, formatDist, PERIOD_LABELS_DETAIL } from '../shared'
import { ContributorDetail } from './ContributorDetail'

export function ContributorRow({ c }: { c: Contributor }) {
  // Keyed by the contributor's stable id, so the row stays open across streamed updates.
  const [expanded, setExpanded] = useState(false)
  const lden = c.received_lden ?? 0
  const periodsText = txtTable([
    [PERIOD_LABELS_DETAIL[0], fmtDb(c.received.ld)],
    [PERIOD_LABELS_DETAIL[1], fmtDb(c.received.le)],
    [PERIOD_LABELS_DETAIL[2], fmtDb(c.received.ln)],
    { sep: true },
    ['→ Lden', fmtDb(c.received_lden)],
  ], 16, 9)

  return (
    <div className="border-b border-border/50 last:border-b-0">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={(e) => {
          e.stopPropagation()
          setExpanded(!expanded)
        }}
        className="w-full py-1.5 text-left cursor-pointer hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <div className="flex items-baseline gap-1.5 text-xs px-0">
          <span className="font-medium truncate flex-1">{contributorLabel(c)}</span>
          <span className="text-muted-foreground/60 shrink-0 w-14 text-right tabular-nums">
            {formatDist(c.distance_m)}
          </span>
          <span
            className="font-bold shrink-0 w-14 text-right tabular-nums"
            style={{ color: ldenToColor(lden) }}
          >
            <DataPoint title="Received level per period" text={periodsText}>
              {lden.toFixed(1)} dB
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
