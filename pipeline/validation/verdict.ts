/**
 * The W1 gate C1–C6 of one candidate against its predecessor and the campaign baseline, with the
 * criteria v2 outcomes pass / fail / insufficient_data / incomplete.
 */
import type { Criteria } from './criteria.ts'
import type { StationRow } from './report.ts'
import type { RuleResult, Unscored } from './rules.ts'

export type Move = { station: string; metric: string; layer: string; before: number; after: number; move_db: number; measured: number | null; explained: boolean }
export type CauseRecord = { station: string; metric: string; layer: string; [field: string]: unknown }
export type Check = { holds: boolean | 'insufficient_data'; detail: string }
export type Outcome = 'pass' | 'fail' | 'insufficient_data' | 'incomplete'
export type Gate = { C1: Check; C2: Check; C3: Check; C4: Check; C5: Check; C6: Check; outcome: Outcome; incomplete: string[] }

/**
 * Criteria C3: every station's total, every layer within the relevance margin of the total at A or
 * at B, and every native indicator, in Lden and Ln, moving by more than the explain threshold.
 */
export function movesOverThreshold(before: StationRow[], after: StationRow[], criteria: Criteria, causes: CauseRecord[]): Move[] {
  const threshold = criteria.constants.move_explain_db.value
  const relevance = criteria.constants.layer_relevance_db.value
  const afterByKey = new Map(after.map(row => [row.key, row]))
  const moves: Move[] = []
  const record = (station: string, metric: string, layer: string, a: number | null | undefined, b: number | null | undefined, measured: number | null) => {
    if (a == null || b == null || Math.abs(b - a) <= threshold) return
    const explained = causes.some(cause => cause.station === station && cause.metric === metric && cause.layer === layer)
    moves.push({ station, metric, layer, before: +a.toFixed(2), after: +b.toFixed(2), move_db: +(b - a).toFixed(2), measured, explained })
  }
  for (const rowA of before) {
    const rowB = afterByKey.get(rowA.key)
    if (!rowA.model || !rowB?.model) continue
    const [a, b] = [rowA.model, rowB.model]
    record(rowA.key, 'lden', 'total', a.total.lden, b.total.lden, null)
    record(rowA.key, 'ln', 'total', a.total.periods.night, b.total.periods.night, null)
    for (const layer of new Set([...Object.keys(a.layers), ...Object.keys(b.layers)])) {
      const relevant = (model: typeof a) => model.layers[layer]?.lden != null && model.total.lden != null
        && model.layers[layer].lden! > model.total.lden - relevance
      if (!relevant(a) && !relevant(b)) continue
      record(rowA.key, 'lden', layer, a.layers[layer]?.lden, b.layers[layer]?.lden, null)
      record(rowA.key, 'ln', layer, a.layers[layer]?.periods.night, b.layers[layer]?.periods.night, null)
    }
    for (const comparison of rowA.comparisons) {
      const other = rowB.comparisons.find(entry => entry.indicator === comparison.indicator)
      record(rowA.key, comparison.indicator, comparison.layer ?? 'total', comparison.model, other?.model, comparison.measured)
    }
  }
  return moves
}

/** Criteria C6 (machine-checkable part): runtime, native, prepared, raster and definition identities are pinned. */
export function missingIdentities(identity: Record<string, unknown>): string[] {
  const ops = identity.ops as Record<string, Record<string, unknown>> | undefined
  const missing: string[] = []
  if (!(identity.server_cohort as { runtime_sha256?: string } | undefined)?.runtime_sha256) missing.push('runtime')
  if (!ops?.code?.native_sha256) missing.push('native addon')
  if (!ops?.prepared?.content_sha256 || ops.prepared.verified_current !== true) missing.push('prepared content (verified current)')
  if (!ops?.rasters?.station_square_rasters_sha256) missing.push('rasters')
  if (!ops?.definitions) missing.push('receiver and period definitions')
  return missing
}

export function gate(input: {
  target: { kind: 'cohort'; met: boolean | 'insufficient_data'; detail: string } | { kind: 'deterministic'; evidence: string } | null
  rules: Array<RuleResult & { against: string }>
  moves: Move[]
  guardRegressions: string[]
  unscored: Unscored[]
  guardErrors: string[]
  identity: Record<string, unknown>
}): Gate {
  const C1: Check = !input.target ? { holds: false, detail: 'no target declared before scoring' }
    : input.target.kind === 'deterministic' ? { holds: true, detail: `deterministic evidence declared: ${input.target.evidence} (checked outside the runner)` }
      : { holds: input.target.met, detail: input.target.detail }
  const fired = input.rules.filter(rule => rule.fired.length > 0)
  const C2: Check = { holds: fired.length === 0, detail: fired.map(rule => `${rule.cohort}/${rule.metric}/${rule.variant}/${rule.stratum} vs ${rule.against}: ${rule.fired.join('+')}`).join('; ') || 'no rule fired' }
  const unexplained = input.moves.filter(move => !move.explained)
  const C3: Check = { holds: unexplained.length === 0, detail: `${input.moves.length} moves over the threshold, ${unexplained.length} without a cause record` }
  const C4: Check = { holds: input.guardRegressions.length === 0, detail: input.guardRegressions.join('; ') || 'no guard sub-check regressed' }
  const C5: Check = C1.holds === true && (C2.holds !== true || C4.holds !== true)
    ? { holds: false, detail: 'the target improves while another panel or guard worsens: an exposing fix merges only with its pair and an ablation' }
    : { holds: true, detail: 'no exposing pattern' }
  const missing = missingIdentities(input.identity)
  const C6: Check = { holds: missing.length === 0, detail: missing.length ? `unpinned: ${missing.join(', ')}` : 'identities pinned; holdout use and CPU/CUDA parity are declared by the change record' }
  const incomplete = [...input.unscored.map(entry => `${entry.key}: ${entry.reason}`), ...input.guardErrors]
  const checks = [C1, C2, C3, C4, C5, C6]
  const outcome: Outcome = checks.some(check => check.holds === false) ? 'fail' : incomplete.length ? 'incomplete'
    : C1.holds === 'insufficient_data' ? 'insufficient_data' : 'pass'
  return { C1, C2, C3, C4, C5, C6, outcome, incomplete }
}
