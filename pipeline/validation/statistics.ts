/**
 * Seeded percentile bootstrap resampling stations within their network; several statistics share
 * each draw, and paired statistics (run A vs run B) see identical station indices.
 */

export type BootstrapConfig = { draws: number; seed: number; level: number }
export type Estimate = { value: number; ci95: [number, number] | null }
/** Below this many samples no interval is reported (a percentile interval of two points is noise). */
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

const round2 = (value: number): number => Math.round(value * 100) / 100

/**
 * Point value and percentile interval of each statistic. Every draw resamples each network's
 * items with replacement to that network's own count, so networks keep their weight.
 */
export function bootstrap<T, K extends string>(
  items: readonly T[], networkOf: (item: T) => string, config: BootstrapConfig,
  statistics: Record<K, (sample: readonly T[]) => number>,
): Record<K, Estimate> {
  const names = Object.keys(statistics) as K[]
  const result = {} as Record<K, Estimate>
  if (items.length === 0) throw new Error('bootstrap of an empty sample')
  const byNetwork = new Map<string, T[]>()
  for (const item of items) byNetwork.set(networkOf(item), [...(byNetwork.get(networkOf(item)) ?? []), item])
  const networks = [...byNetwork.values()]
  const draws = Object.fromEntries(names.map(name => [name, [] as number[]])) as Record<K, number[]>
  if (items.length >= MIN_SAMPLES_FOR_INTERVAL) {
    const random = seededRandom(config.seed)
    const sample: T[] = new Array(items.length)
    for (let draw = 0; draw < config.draws; draw += 1) {
      let index = 0
      for (const group of networks) {
        for (let pick = 0; pick < group.length; pick += 1) sample[index++] = group[Math.floor(random() * group.length)]
      }
      for (const name of names) draws[name].push(statistics[name](sample))
    }
  }
  const tail = (1 - config.level) / 2
  for (const name of names) {
    const sorted = draws[name].sort((a, b) => a - b)
    result[name] = {
      value: round2(statistics[name](items)),
      ci95: sorted.length ? [round2(quantile(sorted, tail)), round2(quantile(sorted, 1 - tail))] : null,
    }
  }
  return result
}

export const mean = (values: readonly number[]): number => values.reduce((sum, value) => sum + value, 0) / values.length
export const share = (values: readonly number[], test: (value: number) => boolean): number => values.filter(test).length / values.length
export const count = (values: readonly number[], test: (value: number) => boolean): number => values.filter(test).length
