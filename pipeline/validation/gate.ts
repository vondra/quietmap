/**
 * Evaluate W1 criteria on runs: frozen membership, cohort objectives O1–O3 per metric and column,
 * paired change rules between two runs, and the C1–C6 gate with its verdict.
 */
import type { IndicatorComparison } from './comparison.ts'
import { membership, METRIC_PERIODS, uncertainty, type Criteria, type CriteriaCohort, type Uncertainty } from './criteria.ts'
import type { StationRow } from './report.ts'
import { bootstrap, count, mean, share, type Estimate } from './statistics.ts'

export type FrozenStation = {
  cohort: string | null
  reason: string
  network: string
  holdout_square: boolean
  uncertainty: Record<string, Uncertainty>
}
export type FrozenMembership = { criteria_version: string; baseline_run: string; stations: Record<string, FrozenStation> }
type Sample = { key: string; network: string; holdout_square: boolean; delta: number; u: Uncertainty }

export function cohortMetrics(cohort: CriteriaCohort, criteria: Criteria): string[] {
  const metrics = [...(cohort.metrics_gated ?? []), ...(cohort.metrics_reported ?? [])]
  return metrics.length ? [...new Set(metrics)] : [...criteria.metrics.periods_gated, ...criteria.metrics.periods_reported]
}

function selectComparison(row: StationRow, cohort: CriteriaCohort, metric: string): IndicatorComparison | null {
  const layer = cohort.compare === 'total' ? null : cohort.compare.replace(/^layer:/, '')
  return row.comparisons.find(comparison => comparison.period === METRIC_PERIODS[metric] && comparison.layer === layer
    && comparison.unit === 'dB' && comparison.delta_db != null && comparison.kind !== 'percentile'
    && (comparison.period_mapping === 'exact' || comparison.period_mapping === 'piecewise_constant')) ?? null
}

/** Membership and uncertainty, assigned once from the baseline run (criteria §3, §4 "frozen"). */
export function freezeMembership(baseline: StationRow[], criteria: Criteria, baselineRun: string): FrozenMembership {
  const stations: Record<string, FrozenStation> = {}
  for (const row of baseline) {
    const { cohort, reason } = membership(row, criteria)
    const definition = criteria.cohorts.find(entry => entry.id === cohort)
    const perMetric: Record<string, Uncertainty> = {}
    for (const metric of definition ? cohortMetrics(definition, criteria) : []) {
      const comparison = selectComparison(row, definition!, metric)
      if (comparison) perMetric[metric] = uncertainty(row, comparison, criteria)
    }
    stations[row.key] = { cohort, reason, network: row.set, holdout_square: row.holdout_square, uncertainty: perMetric }
  }
  return { criteria_version: criteria.criteria_version, baseline_run: baselineRun, stations }
}

function samples(rows: StationRow[], frozen: FrozenMembership, cohort: CriteriaCohort, metric: string): Sample[] {
  return rows.flatMap(row => {
    const station = frozen.stations[row.key]
    const u = station?.uncertainty[metric]
    if (station?.cohort !== cohort.id || !u?.eligible) return []
    const comparison = selectComparison(row, cohort, metric)
    if (!comparison || comparison.model == null || comparison.measured == null) return []
    return [{ key: row.key, network: row.set, holdout_square: station.holdout_square, delta: comparison.model - (comparison.measured + u.correction_db), u }]
  })
}

export type CohortStatistics = {
  n: number; networks: number; bias: Estimate; mae: Estimate; tail6: number; tail10: number; over: number
  band_db: number; allowance_db: number; o2_limit_db: number
  objectives: { O1: string; O2: string; O3: string }
}

export function cohortStatistics(values: Sample[], criteria: Criteria): CohortStatistics | null {
  if (values.length === 0) return null
  const constants = criteria.constants
  const [tail6, tail10] = constants.tail_thresholds_db.value
  const estimates = bootstrap(values, value => value.network, criteria.bootstrap, {
    bias: sample => mean(sample.map(value => value.delta)),
    mae: sample => mean(sample.map(value => Math.abs(value.delta))),
  })
  const band = 2 * Math.sqrt(mean(values.map(value => value.u.band_square)))
  const allowance = mean(values.map(value => value.u.one_sided_allowance_db))
  const o2Limit = Math.sqrt(2 / Math.PI) * Math.sqrt(mean(values.map(value => value.u.u_i ** 2)) + constants.u_method_db.value ** 2)
  const deltas = values.map(value => value.delta)
  const shares = { tail6: share(deltas, delta => Math.abs(delta) > tail6), tail10: share(deltas, delta => Math.abs(delta) > tail10) }
  const enough = values.length >= constants.n_min.value
  const verdict = (meets: boolean) => !enough ? 'insufficient_data' : meets ? 'meets' : 'does_not_meet'
  const ci = estimates.bias.ci95 ?? [estimates.bias.value, estimates.bias.value]
  return {
    n: values.length, networks: new Set(values.map(value => value.network)).size, ...estimates,
    tail6: +shares.tail6.toFixed(3), tail10: +shares.tail10.toFixed(3),
    over: count(values.map((value, index) => value.delta - constants.upper_bound_factor.value * values[index].u.u_i), excess => excess > 0),
    band_db: +band.toFixed(2), allowance_db: +allowance.toFixed(2), o2_limit_db: +o2Limit.toFixed(2),
    objectives: {
      O1: verdict(ci[1] >= -band - allowance && ci[0] <= band),
      O2: verdict((estimates.mae.ci95?.[0] ?? estimates.mae.value) <= o2Limit),
      O3: verdict(shares.tail6 <= constants.objective_tail6_max_share.value && shares.tail10 <= constants.objective_tail10_max_share.value),
    },
  }
}

/** One metric pooled over every cohort's samples (criteria H1: sealed networks in aggregate). */
export function pooledStatistics(rows: StationRow[], frozen: FrozenMembership, criteria: Criteria, metric: string): CohortStatistics | null {
  return cohortStatistics(criteria.cohorts.flatMap(cohort => cohortMetrics(cohort, criteria).includes(metric) ? samples(rows, frozen, cohort, metric) : []), criteria)
}

export type CohortTable = Array<{ cohort: string; role: string; metric: string; gated: boolean; column: string; statistics: CohortStatistics }>

/** Every cohort × metric in all | holdout | training columns, plus network strata. */
export function cohortTable(rows: StationRow[], frozen: FrozenMembership, criteria: Criteria): CohortTable {
  const table: CohortTable = []
  for (const cohort of criteria.cohorts) {
    for (const metric of cohortMetrics(cohort, criteria)) {
      const all = samples(rows, frozen, cohort, metric)
      const gated = cohort.role === 'gated' && (cohort.metrics_gated ?? []).includes(metric)
      const columns: Array<[string, Sample[]]> = [
        ['all', all], ['holdout', all.filter(value => value.holdout_square)], ['training', all.filter(value => !value.holdout_square)],
        ...[...new Set(all.map(value => value.network))].sort().map(network => [`network ${network}`, all.filter(value => value.network === network)] as [string, Sample[]]),
      ]
      for (const [column, values] of columns) {
        const statistics = cohortStatistics(values, criteria)
        if (statistics) table.push({ cohort: cohort.id, role: cohort.role, metric, gated, column, statistics })
      }
    }
  }
  return table
}

export type RuleResult = { cohort: string; metric: string; stratum: string; n: number; fired: string[]; mae_change: Estimate; bias_before: number; bias_after: number }

/** Criteria §5 change rules on paired samples (same stations and u_i; identical resampled indices). */
export function changeRules(before: StationRow[], after: StationRow[], frozen: FrozenMembership, criteria: Criteria): RuleResult[] {
  const constants = criteria.constants
  const [tail6, tail10] = constants.tail_thresholds_db.value
  const results: RuleResult[] = []
  for (const cohort of criteria.cohorts) {
    const upperBoundOnly = cohort.role === 'named_gap'
    if (cohort.role !== 'gated' && !upperBoundOnly) continue
    const metrics = upperBoundOnly ? cohortMetrics(cohort, criteria) : cohort.metrics_gated ?? []
    for (const metric of metrics) {
      const a = new Map(samples(before, frozen, cohort, metric).map(value => [value.key, value]))
      const pairs = samples(after, frozen, cohort, metric).filter(value => a.has(value.key)).map(value => ({ a: a.get(value.key)!, b: value }))
      // Named gaps carry only the cohort-wide upper bound; gated cohorts add their network strata.
      const networks = upperBoundOnly ? [] : [...new Set(pairs.map(pair => pair.b.network))].sort()
      const strata: Array<[string, typeof pairs]> = [['all', pairs],
        ...networks.map(network => [`network ${network}`, pairs.filter(pair => pair.b.network === network)] as [string, typeof pairs])]
      for (const [stratum, values] of strata) {
        if (values.length < constants.n_min.value && !(stratum === 'all' && !upperBoundOnly && values.length > 0)) continue
        const over = (value: Sample) => value.delta > constants.upper_bound_factor.value * value.u.u_i
        const change = (test: (value: Sample) => boolean) => (sample: readonly typeof values[number][]) =>
          sample.filter(pair => test(pair.b)).length - sample.filter(pair => test(pair.a)).length
        const estimates = bootstrap(values, pair => pair.b.network, criteria.bootstrap, {
          mae_change: sample => mean(sample.map(pair => Math.abs(pair.b.delta) - Math.abs(pair.a.delta))),
          bias_after: sample => mean(sample.map(pair => pair.b.delta)),
          tail6_change: change(value => Math.abs(value.delta) > tail6),
          tail10_change: change(value => Math.abs(value.delta) > tail10),
          over_change: change(over),
        })
        const band = 2 * Math.sqrt(mean(values.map(pair => pair.b.u.band_square)))
        const biasBefore = mean(values.map(pair => pair.a.delta))
        const biasAfter = mean(values.map(pair => pair.b.delta))
        const above = (estimate: Estimate) => (estimate.ci95?.[0] ?? estimate.value) > 0
        const fired: string[] = []
        const enough = values.length >= constants.n_min.value
        if (!upperBoundOnly) {
          const ci = estimates.bias_after.ci95 ?? [biasAfter, biasAfter]
          if (Math.abs(biasAfter) - Math.abs(biasBefore) > constants.tau_db.value && (ci[0] > band || ci[1] < -band)) fired.push('bias')
          if (estimates.mae_change.value > constants.tau_db.value && above(estimates.mae_change)) fired.push('mae')
          if (above(estimates.tail6_change) || above(estimates.tail10_change)) fired.push('tails')
        }
        if (enough && above(estimates.over_change)) fired.push('upper_bound')
        results.push({ cohort: cohort.id, metric, stratum, n: values.length, fired, mae_change: estimates.mae_change, bias_before: +biasBefore.toFixed(2), bias_after: +biasAfter.toFixed(2) })
      }
    }
  }
  return results
}
