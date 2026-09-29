// The expanded body of a contributor row: its class, its layer's display fields, and its level
// by day, evening and night.
import type { Contributor } from '../../../types/noise'
import { fmtDbValue } from '../../../utils/formatters'
import { contributorClass, contributorLabel, lineRow, PERIOD_LABELS_DETAIL, subtypeLabel } from '../shared'
import { HoverText } from '../../ui/info-tip'
import { MetadataRows } from './MetadataRows'

const PERIODS_TOOLTIP =
  'Level received here in each period (local time):\n' +
  PERIOD_LABELS_DETAIL.join('\n') +
  '\n\nLden adds +5 dB to the evening and +10 dB to the night.'

export function ContributorDetail({ c }: { c: Contributor }) {
  const cls = contributorClass(c)
  // The row shows the name; the class goes here (a row without a name already shows the class).
  const showClass = cls !== '' && contributorLabel(c) !== subtypeLabel(c.source_type, cls)
  return (
    <div className="mt-1 ml-2 mr-4 mb-1 text-[11px] leading-relaxed font-mono text-muted-foreground">
      {showClass && (
        <div className="text-muted-foreground/60 mb-0.5">{subtypeLabel(c.source_type, cls)}</div>
      )}
      <MetadataRows c={c} />
      {lineRow(
        <HoverText title={PERIODS_TOOLTIP}>Day/Evening/Night</HoverText>,
        `${fmtDbValue(c.received.ld)}/${fmtDbValue(c.received.le)}/${fmtDbValue(c.received.ln)} dB`,
      )}
    </div>
  )
}
