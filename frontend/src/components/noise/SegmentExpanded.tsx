import type { SegmentTrace } from '../../types/noise'
import { HoverText } from '../ui/info-tip'
import { PathProfileDiagram } from './PathProfileDiagram'
import { Section } from './segment/display'
import { Section1Source } from './segment/SourceEmission'
import { Section4PathEffects } from './segment/PathEffects'
import { Section5Lden } from './segment/LdenTable'
import { Section6Variants } from './segment/Variants'

export function SegmentExpanded({ trace }: { trace: SegmentTrace }) {
  // Airborne sub-segments and cruise cells use Doc 29 SEL chains —
  // not ISO 9613-2 path effects — so the path-profile / terrain /
  // screening / vegetation / per-band Lw / Lden sections render with
  // empty / silent / uniform values that misrepresent the source.
  // Render the emission-input section only for those sub-types and
  // skip the ISO chain entirely.
  const isAirborne =
    trace.kind === 'aircraft' && (trace.aircraft_subtype === 2 || trace.aircraft_subtype === 3)
  if (isAirborne) {
    return (
      <div className="ml-2 mr-4 pb-2 text-[11px] leading-relaxed font-mono text-muted-foreground">
        <Section1Source trace={trace} />
      </div>
    )
  }
  // Ground-ops microsegments: path_profile is intentionally left
  // empty by `airport_traffic.rs::emit_segment_traces` (the per-row
  // path is cached internally but not serialized — would 3-4× the
  // popup payload). Skip the path-profile diagram. Per-effect
  // attenuations (Section4), per-period Lden (Section5), and the
  // what-if variants (Section6) stay because their numbers ARE
  // per-microsegment and useful.
  const isGroundOps = trace.kind === 'aircraft' && trace.aircraft_subtype === 1
  return (
    <div className="ml-2 mr-4 pb-2 text-[11px] leading-relaxed font-mono text-muted-foreground">
      <Section1Source trace={trace} />
      {!isGroundOps && trace.propagation.model === 'cnossos' && (
        <Section>
          <PathProfileDiagram
            trace={trace.propagation.path_profile}
            terrainEdges={trace.propagation.terrain.edges}
            obstacleEdge={trace.propagation.screening.obstacle?.edge}
          />
          <HoverText
            title={
              'Bilateral cadence: one 10 m near-probe at each end (berm catch),\n' +
              'then three samples at 30 m / 60 m / 120 m, then 240 m through the\n' +
              'middle. step_m_med is the median inter-sample gap the engine saw\n' +
              '(propagation::path_profile::fill_t_values).'
            }
          >
            <div className="mt-0.5 text-[10px] text-muted-foreground italic">
              Profile: {trace.propagation.path_profile.t.length} samples · median step{' '}
              {trace.propagation.path_profile.step_m_med.toFixed(1)} m
            </div>
          </HoverText>
        </Section>
      )}
      <Section4PathEffects trace={trace} />
      <Section5Lden trace={trace} />
      <Section6Variants trace={trace} />
    </div>
  )
}
