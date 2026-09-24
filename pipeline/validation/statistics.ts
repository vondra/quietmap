/**
 * Error statistics of model − measurement deltas with a seeded bootstrap stratified by network:
 * bias, mean absolute error, tails beyond 3/6/10 dB, P10 and P90, each with a 95 % interval.
 */

export type Metric = 'bias' | 'mae' | 'share_beyond_3db' | 'share_beyond_6db' | 'share_beyond_10db' | 'p10' | 'p90'
export type MetricEstimate = { value: number; ci95: [number, number] | null }
export type ErrorSummary = { n: number; networks: number } & Record<Metric, MetricEstimate>
/** One delta and the network it was measured by; the bootstrap resamples within each network. */
export type Sample = { delta: number; network: string }

/** W1 criteria v1 §2: 10,000 draws, stations resampled within their network, seed 20260924. */
export const BOOTSTRAP_RESAMPLES = 10_000
const BOOTSTRAP_SEED = 20260924
/** W1 criteria v1 §2 (`n_min`): below ten samples a percentile interval undercovers; none is shown under three. */
const MIN_SAMPLES_FOR_INTERVAL = 3

/** mulberry32: a small deterministic PRNG so a rerun reproduces the same intervals. */
function seededRandom(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/** Linear-interpolated quantile of a sorted sample. */
export function quantile(sorted: readonly number[], fraction: number): number {
  if (sorted.length === 0) throw new Error('quantile of an empty sample')
  const position = (sorted.length - 1) * fraction
  const lower = Math.floor(position)
  const upper = Math.ceil(position)
  return sorted[lower] + (sorted[upper] - sorted[lower]) * (position - lower)
}

function metrics(deltas: number[]): Record<Metric, number> {
  const n = deltas.length
  const beyond = (limit: number) => deltas.filter(delta => Math.abs(delta) > limit).length / n
  const sorted = deltas.sort((a, b) => a - b)
  return {
    bias: sorted.reduce((sum, delta) => sum + delta, 0) / n,
    mae: sorted.reduce((sum, delta) => sum + Math.abs(delta), 0) / n,
    share_beyond_3db: beyond(3),
    share_beyond_6db: beyond(6),
    share_beyond_10db: beyond(10),
    p10: quantile(sorted, 0.1),
    p90: quantile(sorted, 0.9),
  }
}

const round2 = (value: number): number => Math.round(value * 100) / 100

/** Point estimates plus percentile-bootstrap 95 % intervals resampled within each network. */
export function summarizeErrors(samples: readonly Sample[], resamples = BOOTSTRAP_RESAMPLES): ErrorSummary | null {
  if (samples.length === 0) return null
  const point = metrics(samples.map(sample => sample.delta))
  const names = Object.keys(point) as Metric[]
  const draws = Object.fromEntries(names.map(name => [name, [] as number[]])) as Record<Metric, number[]>
  const byNetwork = new Map<string, number[]>()
  for (const sample of samples) byNetwork.set(sample.network, [...(byNetwork.get(sample.network) ?? []), sample.delta])
  const networks = [...byNetwork.values()]
  if (samples.length >= MIN_SAMPLES_FOR_INTERVAL) {
    const random = seededRandom(BOOTSTRAP_SEED)
    for (let draw = 0; draw < resamples; draw += 1) {
      const resampled: number[] = []
      for (const deltas of networks) {
        for (let index = 0; index < deltas.length; index += 1) resampled.push(deltas[Math.floor(random() * deltas.length)])
      }
      const estimate = metrics(resampled)
      for (const name of names) draws[name].push(estimate[name])
    }
  }
  const summary = { n: samples.length, networks: networks.length } as ErrorSummary
  for (const name of names) {
    const sorted = draws[name].sort((a, b) => a - b)
    summary[name] = {
      value: round2(point[name]),
      ci95: sorted.length ? [round2(quantile(sorted, 0.025)), round2(quantile(sorted, 0.975))] : null,
    }
  }
  return summary
}

/** Group rows by a key and summarize each group's samples; groups are sorted by key. */
export function summarizeGroups<T>(
  rows: readonly T[], key: (row: T) => string | null, sample: (row: T) => Sample | null,
): Array<{ group: string; summary: ErrorSummary }> {
  const groups = new Map<string, Sample[]>()
  for (const row of rows) {
    const group = key(row)
    const value = sample(row)
    if (group == null || value == null) continue
    groups.set(group, [...(groups.get(group) ?? []), value])
  }
  return [...groups.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([group, values]) => ({ group, summary: summarizeErrors(values)! }))
}
