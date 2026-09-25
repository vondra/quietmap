/**
 * Criteria v2 §5 on the frozen panel: objectives per cohort × metric × variant, paired change rules
 * between two runs (site-cluster bootstrap), per-station protection, the crossing ledger and C1 target.
 */
import type { Criteria, CriteriaCohort, Variant } from './criteria.ts'
import { modelValue, type Manifest } from './manifest.ts'
import type { StationRow } from './report.ts'
import { bootstrap, mean, type Estimate } from './statistics.ts'

type Sample = { key: string; network: string; cluster: string; holdout: boolean; delta: number; u_i: number; band_square: number }
export type Unscored = { key: string; reason: string }

/** Samples of one cohort, metric and variant in one run; frozen stations the run could not score are listed. */
export function samples(rows: StationRow[], manifest: Manifest, cohort: string, metric: string, variant: Variant, geometryOnly = false):
  { values: Sample[]; unscored: Unscored[] } {
  const byKey = new Map(rows.map(row => [row.key, row]))
  const values: Sample[] = []
  const unscored: Unscored[] = []
  for (const station of Object.values(manifest.stations)) {
    const entry = station.metrics[metric]
    if (!station.eligible || station.cohort !== cohort || !entry) continue
    // A station with one convention scores that one in every variant.
    const own = station.variants.includes(variant) ? variant : station.variants[0]
    if (geometryOnly && station.height_pending) continue
    const row = byKey.get(station.station_id)
    const model = row && !row.error && !row.unscored ? modelValue(row, entry, metric) : null
    if (model == null) {
      unscored.push({ key: station.station_id, reason: row?.error ?? row?.unscored ?? (row ? 'no model value' : 'missing from the run') })
      continue
    }
    values.push({ key: station.station_id, network: station.set, cluster: station.site_cluster, holdout: station.z9_holdout_square,
      delta: model - entry.O_by_variant[own]!, u_i: entry.u_i_by_variant[own]!, band_square: entry.band_square })
  }
  return { values, unscored }
}

/** Resample site clusters within their network; statistics see the flattened samples. */
function clustered<T extends { network: string; cluster: string }, K extends string>(
  values: T[], criteria: Criteria, statistics: Record<K, (sample: readonly T[]) => number>,
): Record<K, Estimate> {
  const clusters = [...new Map(values.map(value => [value.cluster, values.filter(other => other.cluster === value.cluster)])).values()]
  const flattened = Object.fromEntries(Object.entries(statistics).map(([name, statistic]) =>
    [name, (sample: readonly T[][]) => (statistic as (sample: readonly T[]) => number)(sample.flat())])) as Record<K, (sample: readonly T[][]) => number>
  return bootstrap(clusters, cluster => cluster[0].network, { ...criteria.constants.bootstrap, level: 0.95 }, flattened)
}

export type Objectives = {
  n: number; networks: number; clusters: number; bias: Estimate; mae: Estimate; share_large: number; share_severe: number; over: number
  band_db: number; o2_limit_db: number; O1: string; O2: string; O3: string
}

export function objectives(values: Sample[], criteria: Criteria): Objectives | null {
  if (values.length === 0) return null
  const c = criteria.constants
  const estimates = clustered(values, criteria, {
    bias: sample => mean(sample.map(value => value.delta)), mae: sample => mean(sample.map(value => Math.abs(value.delta))),
  })
  const band = 2 * Math.sqrt(mean(values.map(value => value.band_square)))
  const o2 = mean(values.map(value => Math.sqrt(2 / Math.PI) * Math.sqrt(value.u_i ** 2 + c.u_method_db.value ** 2)))
  const share = (limit: number) => values.filter(value => Math.abs(value.delta) > limit).length / values.length
  const verdict = (meets: boolean) => values.length < c.n_min.value ? 'insufficient_data' : meets ? 'meets' : 'does_not_meet'
  const ci = estimates.bias.ci95 ?? [estimates.bias.value, estimates.bias.value]
  return {
    n: values.length, networks: new Set(values.map(value => value.network)).size, clusters: new Set(values.map(value => value.cluster)).size,
    ...estimates, share_large: +share(c.large_error_db.value).toFixed(3), share_severe: +share(c.severe_db.value).toFixed(3),
    over: values.filter(value => value.delta > c.upper_bound_k.value * value.u_i).length, band_db: +band.toFixed(2), o2_limit_db: +o2.toFixed(2),
    O1: verdict(ci[1] >= -band && ci[0] <= band), O2: verdict((estimates.mae.ci95?.[0] ?? estimates.mae.value) <= o2),
    O3: verdict(share(c.large_error_db.value) <= c.large_error_net_share.value && share(c.severe_db.value) === 0),
  }
}

/** A cohort's two-sided rules apply when gated, or once a conditionally gated cohort reaches n_min. */
export function gatedTwoSided(cohort: CriteriaCohort, n: number, criteria: Criteria): boolean {
  if (cohort.role === 'gated') return true
  return /once n >= n_min|once >= n_min/.test(cohort.role) && n >= criteria.constants.n_min.value
}
/** Every panel but diagnostics carries the per-station upper bound on Lden and Ln. */
export const upperBoundPanel = (cohort: CriteriaCohort): boolean => !cohort.role.startsWith('diagnostic')
export const UPPER_BOUND_METRICS = ['lden', 'ln']

export type Crossing = { key: string; cohort: string; metric: string; variant: Variant; line_db: number; before: number; after: number }
export type RuleResult = {
  cohort: string; metric: string; variant: Variant; stratum: string; n: number; fired: string[]
  mae_before: number; mae_after: number; mae_change: Estimate; bias_before: number; bias_after: number; bias_change: Estimate
  severe_stations: string[]; large_error_net: number; upper_bound_stations: string[]
}

/** Paired rules of one sample set: A and B share stations, receivers and u_i. */
export function pairedRules(before: Sample[], after: Sample[], twoSided: boolean, criteria: Criteria, label: Omit<RuleResult, 'n' | 'fired' | 'mae_before' | 'mae_after' | 'mae_change' | 'bias_before' | 'bias_after' | 'bias_change' | 'severe_stations' | 'large_error_net' | 'upper_bound_stations'>,
  ledger: Crossing[]): RuleResult | null {
  const c = criteria.constants
  const tau = c.tau_db.value
  const a = new Map(before.map(value => [value.key, value]))
  const pairs = after.filter(value => a.has(value.key)).map(value => ({ network: value.network, cluster: value.cluster, a: a.get(value.key)!, b: value }))
  if (pairs.length === 0) return null
  const estimates = clustered(pairs, criteria, {
    mae_change: sample => mean(sample.map(pair => Math.abs(pair.b.delta) - Math.abs(pair.a.delta))),
    bias_change: sample => mean(sample.map(pair => pair.b.delta - pair.a.delta)),
  })
  const maeA = mean(pairs.map(pair => Math.abs(pair.a.delta)))
  const maeB = mean(pairs.map(pair => Math.abs(pair.b.delta)))
  const biasA = mean(pairs.map(pair => pair.a.delta))
  const biasB = mean(pairs.map(pair => pair.b.delta))
  const band = 2 * Math.sqrt(mean(pairs.map(pair => pair.b.band_square)))
  const grew = (pair: typeof pairs[number]) => Math.abs(pair.b.delta) - Math.abs(pair.a.delta)
  const severe = pairs.filter(pair => Math.abs(pair.b.delta) > c.severe_db.value && grew(pair) > tau).map(pair => pair.b.key)
  const newlyLarge = pairs.filter(pair => Math.abs(pair.b.delta) > c.large_error_db.value && Math.abs(pair.a.delta) <= c.large_error_db.value && grew(pair) > tau).length
  const newlySmall = pairs.filter(pair => Math.abs(pair.b.delta) <= c.large_error_db.value && Math.abs(pair.a.delta) > c.large_error_db.value && -grew(pair) > tau).length
  const upper = pairs.filter(pair => pair.b.delta > c.upper_bound_k.value * pair.b.u_i && pair.b.delta - pair.a.delta > tau).map(pair => pair.b.key)
  for (const line of [c.severe_db.value, c.large_error_db.value]) {
    for (const pair of pairs) {
      const crossed = (Math.abs(pair.a.delta) > line) !== (Math.abs(pair.b.delta) > line)
      if (crossed && Math.abs(grew(pair)) <= tau) {
        ledger.push({ key: pair.b.key, cohort: label.cohort, metric: label.metric, variant: label.variant, line_db: line, before: +pair.a.delta.toFixed(2), after: +pair.b.delta.toFixed(2) })
      }
    }
  }
  const fired: string[] = []
  if (twoSided) {
    if (maeB - maeA > tau) fired.push('mae')
    const ci = estimates.bias_change.ci95 ?? [estimates.bias_change.value, estimates.bias_change.value]
    const increasesMagnitude = biasB >= 0 ? ci[0] > 0 : ci[1] < 0
    if (Math.abs(biasB) - Math.abs(biasA) > tau && Math.abs(biasB) > band && increasesMagnitude) fired.push('bias')
    if (severe.length) fired.push('severe_station')
    if (newlyLarge - newlySmall > c.large_error_net_share.value * pairs.length) fired.push('large_error_share')
  }
  if (upper.length) fired.push('upper_bound')
  return {
    ...label, n: pairs.length, fired, mae_before: +maeA.toFixed(2), mae_after: +maeB.toFixed(2), mae_change: estimates.mae_change,
    bias_before: +biasA.toFixed(2), bias_after: +biasB.toFixed(2), bias_change: estimates.bias_change,
    severe_stations: twoSided ? severe : [], large_error_net: newlyLarge - newlySmall, upper_bound_stations: upper,
  }
}

/** Every applicable panel, metric, variant and gated network stratum of criteria v2 §5. */
export function changeRules(before: StationRow[], after: StationRow[], manifest: Manifest, criteria: Criteria):
  { results: RuleResult[]; ledger: Crossing[]; unscored: Unscored[] } {
  const results: RuleResult[] = []
  const ledger: Crossing[] = []
  const unscored = new Map<string, Unscored>()
  for (const cohort of criteria.cohorts) {
    if (!upperBoundPanel(cohort)) continue
    const metrics = [...new Set([...UPPER_BOUND_METRICS, ...(cohort.metrics_gated ?? []).filter(metric => /^l\w+$/.test(metric))])]
    for (const metric of metrics) {
      for (const variant of ['as_published', 'minus3'] as Variant[]) {
        const a = samples(before, manifest, cohort.id, metric, variant)
        const b = samples(after, manifest, cohort.id, metric, variant)
        for (const entry of b.unscored) unscored.set(entry.key, entry)
        const twoSided = gatedTwoSided(cohort, b.values.length, criteria) && (cohort.metrics_gated ?? []).includes(metric)
        const whole = pairedRules(a.values, b.values, twoSided, criteria, { cohort: cohort.id, metric, variant, stratum: 'all' }, ledger)
        if (whole) results.push(whole)
        if (!twoSided) continue
        for (const network of [...new Set(b.values.map(value => value.network))].sort()) {
          const stratumB = b.values.filter(value => value.network === network)
          if (stratumB.length < criteria.constants.n_min.value || stratumB.length === b.values.length) continue
          const stratum = pairedRules(a.values.filter(value => value.network === network), stratumB, true, criteria,
            { cohort: cohort.id, metric, variant, stratum: `network ${network}` }, [])
          if (stratum) results.push(stratum)
        }
      }
    }
  }
  return { results, ledger, unscored: [...unscored.values()] }
}

/** Criteria v2 `rules.target` on one cohort: MAE down by at least τ with its interval below 0, in every variant. */
export function targetMet(before: StationRow[], after: StationRow[], manifest: Manifest, criteria: Criteria, cohort: string, geometryOnly: boolean):
  { met: boolean | 'insufficient_data'; detail: string } {
  const details: string[] = []
  let met: boolean | 'insufficient_data' = true
  for (const variant of ['as_published', 'minus3'] as Variant[]) {
    const a = samples(before, manifest, cohort, 'lden', variant, geometryOnly).values
    const b = samples(after, manifest, cohort, 'lden', variant, geometryOnly).values
    if (b.length === 0) continue
    if (b.length < criteria.constants.n_min.value) {
      met = met === false ? false : 'insufficient_data'
      details.push(`${variant}: n = ${b.length}`)
      continue
    }
    const rule = pairedRules(a, b, true, criteria, { cohort, metric: 'lden', variant, stratum: 'all' }, [])!
    const improved = rule.mae_after - rule.mae_before <= -criteria.constants.tau_db.value && (rule.mae_change.ci95?.[1] ?? rule.mae_change.value) < 0
    if (!improved || rule.fired.length) met = false
    details.push(`${variant}: ΔMAE ${(rule.mae_after - rule.mae_before).toFixed(2)} CI ${JSON.stringify(rule.mae_change.ci95)}, rules ${rule.fired.join('+') || 'none'}`)
  }
  return { met, detail: `${cohort}${geometryOnly ? ' (documented heights only)' : ''}: ${details.join('; ') || 'no stations'}` }
}
