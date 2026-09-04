import { useMemo } from 'react'
import type { SegmentTrace } from '../../../types/noise'
import { ldenToColor } from '../../../utils/noise-colors'
import { HoverText } from '../../ui/info-tip'
import { PERIOD_LABELS_DETAIL, PERIOD_TOOLTIP } from '../shared'
import { Section, bandsTooltip, fmtDbSigned } from './display'

// CNOSSOS period weights (hours in each bucket, out of 24). Labels
// share PERIOD_LABELS_DETAIL with other tabs so the wording matches.
const PERIOD_ROWS = [
  { key: 'day', label: PERIOD_LABELS_DETAIL[0], weight: 12 },
  { key: 'evening', label: PERIOD_LABELS_DETAIL[1], weight: 4 },
  { key: 'night', label: PERIOD_LABELS_DETAIL[2], weight: 8 },
] as const

export function Section5Lden({ trace }: { trace: SegmentTrace }) {
  // Section 5 is CNOSSOS-only (per-band Lw / received bands). Doc 29
  // aircraft uses single-number SEL — no band breakdown to render here.
  const cn = trace.propagation.model === 'cnossos' ? trace.propagation : null
  const periodCells = useMemo(
    () =>
      cn
        ? PERIOD_ROWS.map(p => {
            const bands = cn.received_bands[p.key]
            // `null` bands (engine emits NEG_INFINITY → JSON null for
            // silent / unsupported sources) coerce to 0 inside `b / 10`
            // and produce `10^0 = 1` per band — 8 bands then sum to ~9
            // dB, the fake "L_rec 9 dB" that surfaced in pre-Tier-1+2
            // ground-ops popups. Filter non-finite before the power
            // sum so a silent band stays silent.
            const energy = bands.reduce(
              (acc: number, b: number | null) => acc + (Number.isFinite(b) ? Math.pow(10, (b as number) / 10) : 0),
              0,
            )
            const lrec = energy > 0 ? 10 * Math.log10(energy) : Number.NEGATIVE_INFINITY
            return {
              ...p,
              lw: cn.lw_db_a[p.key],
              lwBands: cn.lw_bands[p.key],
              lrec,
              lrecBands: bands,
            }
          })
        : [],
    [cn],
  )
  return (
    <Section>
      <table className="w-full tabular-nums">
        <thead>
          <tr className="text-muted-foreground/60">
            <th className="text-left font-normal pb-0.5">Period</th>
            <th className="text-right font-normal pb-0.5">
              <HoverText
                title={
                  'L_w — A-weighted sound power level at the source,\n' +
                  'per period. Line sources: dB(A)/m; point sources: dB(A).\n' +
                  'CNOSSOS D/E/N split applied (12/4/8 hour buckets).'
                }
              >
                Lw (A)
              </HoverText>
            </th>
            <th className="text-right font-normal pb-0.5">
              <HoverText
                title={
                  'L_rec — A-weighted received level at this point, per\n' +
                  'period. = L_w − (all propagation terms). What someone\n' +
                  "standing here would actually hear during that period."
                }
              >
                L_rec (A)
              </HoverText>
            </th>
            <th className="text-right font-normal pb-0.5">hours</th>
          </tr>
        </thead>
        <tbody>
          {periodCells.map(p => (
            <tr key={p.key}>
              <td className="text-muted-foreground/80">{p.label}</td>
              <td className="text-right">
                <HoverText
                  title={bandsTooltip(p.lwBands, {
                    title: `${p.label} — Lw per band`,
                    signed: false,
                    note: 'Scalar above = A-weighted sum of all 8 bands (dB(A)).',
                  })}
                                 >
                  {fmtDbSigned(p.lw, { signed: false })}
                </HoverText>
              </td>
              <td className="text-right">
                <HoverText
                  title={bandsTooltip(p.lrecBands, {
                    title: `${p.label} — L_received per band`,
                    signed: false,
                    note: 'Scalar above = A-weighted sum of all 8 bands (dB(A)).',
                  })}
                                 >
                  {fmtDbSigned(p.lrec, { signed: false })}
                </HoverText>
              </td>
              <td className="text-right text-muted-foreground/50">{p.weight} h</td>
            </tr>
          ))}
          <tr className="border-t border-border/40">
            <td>
              <HoverText
                title={
                  'L_den — Day-Evening-Night weighted 24 h noise level.\n' +
                  'Standard END 2002/49/EC metric. Evening gets a +5 dB\n' +
                  'penalty, night +10 dB to reflect higher annoyance.\n' +
                  'Hover the value for the exact formula applied here.'
                }
              >
                Lden
              </HoverText>
            </td>
            <td
              colSpan={3}
              className="text-right font-medium"
              style={{ color: ldenToColor(trace.received_lden.full) }}
            >
              <HoverText
                title={
                  `received_lden.full = ${trace.received_lden.full.toFixed(2)} dB (engine scalar).\n\n` +
                  'Formula:\n' +
                  '  Lden = 10·log₁₀((12·10^(Ld/10)\n' +
                  '              + 4·10^((Le+5)/10)\n' +
                  '              + 8·10^((Ln+10)/10)) / 24)\n\n' +
                  'Ld / Le / Ln are the per-period L_rec(A) rows above\n' +
                  '(day / evening / night). Engine sums per-band received\n' +
                  'energies into each period\'s A-weighted total, then the\n' +
                  'weighted log-sum into Lden.\n\n' +
                  PERIOD_TOOLTIP
                }
              >
                {trace.received_lden.full.toFixed(1)} dB
              </HoverText>
            </td>
          </tr>
        </tbody>
      </table>
    </Section>
  )
}
