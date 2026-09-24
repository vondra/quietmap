/**
 * The W1 gate C1–C6 of one candidate run against its predecessor and the campaign baseline:
 * target, no worsening, explained moves over 3 dB, guard identities, coupling and pinned identities.
 */
import type { Criteria } from './criteria.ts'
import type { RuleResult } from './gate.ts'
import type { StationRow } from './report.ts'

export type Move = { station: string; metric: string; layer: string; before: number; after: number; move_db: number; measured: number | null; explained: boolean }
export type CauseRecord = { station: string; metric: string; layer: string; [field: string]: unknown }
export type Target = { cohort: string } | { deterministic: string }
export type Check = { holds: boolean | 'insufficient_data'; detail: string }
export type Gate = { C1: Check; C2: Check; C3: Check; C4: Check; C5: Check; C6: Check; verdict: 'pass' | 'fail' | 'insufficient_data' }

/**
 * Criteria C3: the total, every layer within the relevance margin of the total, and every native
 * indicator, for Lden and Ln, moving more than the explain threshold between two runs.
 */
export function movesOverThreshold(before: StationRow[], after: StationRow[], criteria: Criteria, causes: CauseRecord[]): Move[] {
  const threshold = criteria.constants.move_explain_db.value
  const relevance = criteria.constants.layer_relevance_db.value
  const afterByKey = new Map(after.map(row => [row.key, row]))
  const moves: Move[] = []
  const record = (station: string, metric: string, layer: string, a: number | null, b: number | null, measured: number | null) => {
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
      record(rowA.key, 'lden', layer, a.layers[layer]?.lden ?? null, b.layers[layer]?.lden ?? null, null)
      record(rowA.key, 'ln', layer, a.layers[layer]?.periods.night ?? null, b.layers[layer]?.periods.night ?? null, null)
    }
    for (const comparison of rowA.comparisons) {
      const other = rowB.comparisons.find(entry => entry.indicator === comparison.indicator)
      record(rowA.key, comparison.indicator, comparison.layer ?? 'total', comparison.model, other?.model ?? null, comparison.measured)
    }
  }
  return moves
}

/** Criteria C4: a guard whose source identity held before and fails now. */
export function guardRegressions(before: StationRow[], after: StationRow[]): string[] {
  const afterByKey = new Map(after.map(row => [row.key, row]))
  return before.filter(row => row.guard?.passed && afterByKey.get(row.key)?.guard?.passed === false).map(row => row.key)
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
  criteria: Criteria
  target: Target | null
  targetRules: RuleResult | null
  predecessorRules: RuleResult[]
  baselineRules: RuleResult[]
  moves: Move[]
  guardRegressions: string[]
  identity: Record<string, unknown>
}): Gate {
  const nMin = input.criteria.constants.n_min.value
  const gatedFailures = [...input.predecessorRules.map(rule => ({ ...rule, against: 'predecessor' })), ...input.baselineRules.map(rule => ({ ...rule, against: 'baseline' }))]
    .filter(rule => rule.fired.length > 0)
  let C1: Check
  if (!input.target) C1 = { holds: false, detail: 'no target declared before scoring' }
  else if ('deterministic' in input.target) C1 = { holds: true, detail: `deterministic target evidence declared: ${input.target.deterministic} (checked outside the runner)` }
  else if (!input.targetRules || input.targetRules.n < nMin) C1 = { holds: 'insufficient_data', detail: `target ${input.target.cohort} has n = ${input.targetRules?.n ?? 0} < ${nMin}` }
  else {
    const improved = (input.targetRules.mae_change.ci95?.[1] ?? input.targetRules.mae_change.value) < 0
    C1 = { holds: improved && input.targetRules.fired.length === 0, detail: `target ${input.target.cohort}: paired ΔMAE ${input.targetRules.mae_change.value} `
      + `CI ${JSON.stringify(input.targetRules.mae_change.ci95)}; rules fired: ${input.targetRules.fired.join(', ') || 'none'}` }
  }
  const C2: Check = { holds: gatedFailures.length === 0, detail: gatedFailures.map(rule => `${rule.cohort}/${rule.metric}/${rule.stratum} vs ${rule.against}: ${rule.fired.join('+')}`).join('; ') || 'no rule fired' }
  const unexplained = input.moves.filter(move => !move.explained)
  const C3: Check = { holds: unexplained.length === 0, detail: `${input.moves.length} moves over ${input.criteria.constants.move_explain_db.value} dB, ${unexplained.length} without a cause record` }
  const C4: Check = { holds: input.guardRegressions.length === 0, detail: input.guardRegressions.join(', ') || 'no guard identity regressed' }
  const C5: Check = C1.holds === true && (C2.holds !== true || C4.holds !== true)
    ? { holds: false, detail: 'the target improves while another cohort or guard worsens: an exposing fix merges only with its pair and an ablation' }
    : { holds: true, detail: 'no exposing pattern' }
  const missing = missingIdentities(input.identity)
  const C6: Check = { holds: missing.length === 0, detail: missing.length ? `unpinned: ${missing.join(', ')}` : 'identities pinned; holdout use and CPU/CUDA parity are declared by the change record' }
  const checks = [C1, C2, C3, C4, C5, C6]
  const verdict = checks.some(check => check.holds === false) ? 'fail' : C1.holds === 'insufficient_data' ? 'insufficient_data' : 'pass'
  return { C1, C2, C3, C4, C5, C6, verdict }
}
