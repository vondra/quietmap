import { useState } from 'react'
import type { SegmentTrace } from '../../../types/noise'
import { HoverText } from '../../ui/info-tip'
import { fmtDbSigned } from './display'

export function Section6Variants({ trace }: { trace: SegmentTrace }) {
  const [open, setOpen] = useState(false)
  const { received_lden } = trace
  // "Full" removed — it duplicates the Lden shown in the collapsed header
  // and at the bottom of §5. The delta column anchors against received_lden.full
  // so the label is still the reference, just not rendered as its own row.
  const rows: [string, number, string][] = [
    [
      'Free field',
      received_lden.free_field,
      'Engine variant: full − obstructions − reflection.\n' +
      'Computed as base − A_gr + A_flc (iso9613.rs:253) —\n' +
      'includes divergence, atmospheric, ground, and finite-line\n' +
      'correction, but NOT terrain/screening/foliage obstructions\n' +
      'and NOT urban reflection. Δ vs Full = combined effect of\n' +
      'obstructions plus reflection at this receiver.',
    ],
    [
      'No terrain',
      received_lden.no_terrain,
      'Lden recalculated without terrain diffraction (A_bar, terrain\n' +
      'component only — building/barrier screening still applied).\n' +
      'Δ vs Full = what the hill contributed.',
    ],
    [
      'No screening',
      received_lden.no_screening,
      'Lden without the building/barrier component of combined\n' +
      'diffraction. Δ vs Full = how much the tallest obstacle helped.',
    ],
    [
      'No vegetation',
      received_lden.no_vegetation,
      'Lden without A_fol foliage attenuation. Δ vs Full = forest\n' +
      'contribution to noise reduction along this path.',
    ],
    [
      'No ground',
      received_lden.no_ground,
      'Lden without A_gr ground effect. SIGNED: over soft ground,\n' +
      'removing it may INCREASE Lden (LF boost disappears), so Δ\n' +
      'can be either sign.',
    ],
    [
      'No atmospheric',
      received_lden.no_atmospheric,
      'Lden without atmospheric absorption (air = perfectly\n' +
      'transmitting). Δ vs Full = what the air ate, mainly HF.',
    ],
  ]
  const whatIfTooltip =
    `Deltas below = Lden when removing each effect, compared to\n` +
    `the full ${received_lden.full.toFixed(1)} dB shown above. Each row\n` +
    `is an engine variant (no_terrain / no_screening / …) from\n` +
    `iso9613.rs; Δ quantifies what that effect contributes.`
  return (
    <div className="mt-2">
      <button
        type="button"
        onClick={() => setOpen(o => !o)}
        className="text-[9px] uppercase tracking-[0.08em] text-muted-foreground/70 hover:text-foreground inline-flex items-center gap-1"
      >
        <span>{open ? '▾' : '▸'}</span>
        <HoverText title={whatIfTooltip}>
          <span>What-if</span>
        </HoverText>
      </button>
      {open && (
        <div className="grid grid-cols-[auto_1fr_auto] gap-x-3 gap-y-0.5 tabular-nums mt-0.5">
          {rows.map(([label, v, conceptTooltip], i) => {
            const delta = v - received_lden.full
            return (
              <div key={i} className="contents">
                <HoverText title={conceptTooltip}>
                  <span className="text-muted-foreground/70">{label}</span>
                </HoverText>
                <span className="text-right">{fmtDbSigned(v, { signed: false })}</span>
                <span className="text-right text-muted-foreground/50">
                  {delta > 0 ? `+${delta.toFixed(1)}` : delta.toFixed(1)}
                </span>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
