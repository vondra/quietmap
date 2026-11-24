// Nearby mapped sources whose sound is not modelled (active military firing ranges and training
// areas): shown outside contributor shares and totals, opening to their area, kind and map outline.
import { usePopupState } from '../../../hooks/usePopupState'
import type { UnmodelledSite } from '../../../types/noise'
import { fmtInt } from '../../../utils/formatters'
import { FadingText } from '../../ui/fading-text'
import { unmodelledKindLabel, unmodelledLabel } from '../labels'
import { CAPTION, Chevron, DETAIL_TEXT, formatDist, lineRow } from '../shared'

function formatArea(areaM2: number): string {
  if (areaM2 >= 100_000) return `${(areaM2 / 1e6).toFixed(1)} km²`
  if (areaM2 >= 10_000) return `${(areaM2 / 1e4).toFixed(1)} ha`
  return `${fmtInt(areaM2)} m²`
}

function UnmodelledRow({ site, onHighlight }: {
  site: UnmodelledSite
  onHighlight: (id: string | null) => void
}) {
  const [expanded, setExpanded] = usePopupState<boolean>(`source:${site.id}`, false)
  const label = unmodelledLabel(site)
  const kindLabel = unmodelledKindLabel(site.kind)
  const distance = site.inside ? 'Inside' : formatDist(site.distance_m)
  return (
    <div className="border-b border-border/50 last:border-b-0">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={(e) => {
          e.stopPropagation()
          setExpanded(!expanded)
          onHighlight(!expanded ? site.id : null)
        }}
        className="w-full py-1.5 text-left cursor-pointer hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
      >
        <div className="grid grid-cols-[minmax(0,1fr)_3.5rem_0.75rem] gap-x-2 items-baseline text-xs">
          <FadingText whole={expanded} className="font-medium">{label}</FadingText>
          <span className="text-right whitespace-nowrap tabular-nums text-muted-foreground/60">{distance}</span>
          <Chevron open={expanded} />
        </div>
      </button>
      {expanded && (
        <div className={`mb-2 ml-2 mr-5 space-y-1 ${DETAIL_TEXT}`}>
          {site.name && label.toLowerCase() !== kindLabel.toLowerCase() && <div>{kindLabel}</div>}
          {site.area_m2 > 0 && lineRow('Area', formatArea(site.area_m2))}
          <div className="pt-0.5 text-muted-foreground">
            Firing and blast noise is not included in the level.
          </div>
        </div>
      )}
    </div>
  )
}

export function UnmodelledSection({ sites, onHighlight }: {
  sites: UnmodelledSite[] | undefined
  onHighlight: (id: string | null) => void
}) {
  if (!sites?.length) return null
  return (
    <div data-testid="unmodelled-section" className="mt-1.5 border-t border-border pt-1.5">
      <div className={`${CAPTION} mb-0.5`}>Not in the level</div>
      {sites.map(site => (
        <UnmodelledRow key={site.id} site={site} onHighlight={onHighlight} />
      ))}
    </div>
  )
}
