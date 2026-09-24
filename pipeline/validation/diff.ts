/**
 * Diff two validation runs: per station and indicator the model move, per group the change in
 * bias and absolute error, and every move over 3 dB flagged for an evidenced explanation.
 *
 * Run: node --import tsx validation/diff.ts --before <run dir> --after <run dir> --out <new dir>
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'
import { cohort, type StationRow } from './report.ts'
import { summarizeGroups } from './statistics.ts'

/** Plan rule: a model move over 3 dB at any station needs an evidenced cause. */
export const EXPLAIN_MOVE_ABOVE_DB = 3

export type StationMove = {
  key: string
  network: string
  indicator: string
  period: string
  group: string
  measured_before: number | [number | null, number | null] | null
  measured_after: number | [number | null, number | null] | null
  model_before: number | null
  model_after: number | null
  move_db: number | null
  abs_error_change_db: number | null
  layer_lden_moves: Record<string, number | null>
  needs_explanation: boolean
}

const round2 = (value: number): number => Math.round(value * 100) / 100

function readRows(dir: string): StationRow[] {
  return readFileSync(resolve(dir, 'stations.jsonl'), 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line) as StationRow)
}

export function diffRuns(before: StationRow[], after: StationRow[]): { moves: StationMove[]; only_before: string[]; only_after: string[] } {
  const afterByKey = new Map(after.map(row => [row.key, row]))
  const moves: StationMove[] = []
  for (const rowBefore of before) {
    const rowAfter = afterByKey.get(rowBefore.key)
    if (!rowAfter) continue
    const layers = new Set([...Object.keys(rowBefore.model?.layers ?? {}), ...Object.keys(rowAfter.model?.layers ?? {})])
    const layerMoves = Object.fromEntries([...layers].sort().map(layer => {
      const a = rowBefore.model?.layers[layer]?.lden ?? null
      const b = rowAfter.model?.layers[layer]?.lden ?? null
      return [layer, a != null && b != null ? round2(b - a) : null]
    }))
    for (const comparisonBefore of rowBefore.comparisons) {
      const comparisonAfter = rowAfter.comparisons.find(entry => entry.indicator === comparisonBefore.indicator)
      if (!comparisonAfter) continue
      const move = comparisonBefore.model != null && comparisonAfter.model != null ? round2(comparisonAfter.model - comparisonBefore.model) : null
      const errorChange = comparisonBefore.delta_db != null && comparisonAfter.delta_db != null
        ? round2(Math.abs(comparisonAfter.delta_db) - Math.abs(comparisonBefore.delta_db)) : null
      moves.push({
        key: rowBefore.key,
        network: rowBefore.set,
        indicator: comparisonBefore.indicator,
        period: comparisonBefore.period,
        group: `${cohort(rowBefore)} | ${comparisonBefore.period}${comparisonBefore.layer ? ` (${comparisonBefore.layer})` : ''}`,
        measured_before: comparisonBefore.band ?? comparisonBefore.measured,
        measured_after: comparisonAfter.band ?? comparisonAfter.measured,
        model_before: comparisonBefore.model,
        model_after: comparisonAfter.model,
        move_db: move,
        abs_error_change_db: errorChange,
        layer_lden_moves: layerMoves,
        needs_explanation: (move != null && Math.abs(move) > EXPLAIN_MOVE_ABOVE_DB)
          || JSON.stringify([comparisonBefore.measured, comparisonBefore.band]) !== JSON.stringify([comparisonAfter.measured, comparisonAfter.band])
          || (comparisonBefore.model == null) !== (comparisonAfter.model == null),
      })
    }
  }
  const beforeKeys = new Set(before.map(row => row.key))
  return {
    moves,
    only_before: before.filter(row => !afterByKey.has(row.key)).map(row => row.key),
    only_after: after.filter(row => !beforeKeys.has(row.key)).map(row => row.key),
  }
}

function render(result: ReturnType<typeof diffRuns>, identities: { before: unknown; after: unknown }): string {
  const sample = (value: number | null, move: StationMove) => value == null ? null : { delta: value, network: move.network }
  const byGroupMove = summarizeGroups(result.moves, move => move.group, move => sample(move.move_db, move))
  const byGroupError = new Map(summarizeGroups(result.moves, move => move.group, move => sample(move.abs_error_change_db, move))
    .map(entry => [entry.group, entry.summary]))
  const flagged = result.moves.filter(move => move.needs_explanation)
  const ci = (value: { value: number; ci95: [number, number] | null }) => `${value.value.toFixed(2)}${value.ci95 ? ` [${value.ci95[0]}, ${value.ci95[1]}]` : ''}`
  return [
    '# Validation diff', '',
    `Before: ${JSON.stringify(identities.before)}`, '',
    `After: ${JSON.stringify(identities.after)}`, '',
    `Compared ${result.moves.length} station indicators; only before ${result.only_before.length}, only after ${result.only_after.length}; `
      + `moves over ${EXPLAIN_MOVE_ABOVE_DB} dB or changed measurements: ${flagged.length}.`, '',
    '| cohort (before) | period (layer) | n | mean model move dB [95 % CI] | mean change of abs error dB [95 % CI] | max abs move dB |',
    '| --- | --- | --- | --- | --- | --- |',
    ...byGroupMove.map(({ group, summary }) => {
      const maxMove = Math.max(...result.moves.filter(move => move.group === group && move.move_db != null).map(move => Math.abs(move.move_db!)))
      const error = byGroupError.get(group)
      return `| ${group} | ${summary.n} | ${ci(summary.bias)} | ${error ? ci(error.bias) : '-'} | ${maxMove.toFixed(2)} |`
    }),
    '', '## Needs an evidenced explanation', '',
    '| station | indicator | model before | model after | move dB | layer Lden moves dB |', '| --- | --- | --- | --- | --- | --- |',
    ...flagged.map(move => `| ${move.key} | ${move.indicator} | ${move.model_before ?? '-'} | ${move.model_after ?? '-'} | ${move.move_db ?? '-'} | `
      + `${Object.entries(move.layer_lden_moves).map(([layer, value]) => `${layer} ${value ?? '-'}`).join(', ')} |`),
    '',
  ].join('\n')
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { values: args } = parseArgs({ options: { before: { type: 'string' }, after: { type: 'string' }, out: { type: 'string' } } })
  if (!args.before || !args.after || !args.out) {
    console.error('usage: diff.ts --before RUN_DIR --after RUN_DIR --out NEW_DIR')
    process.exit(2)
  }
  const outDir = resolve(args.out)
  if (existsSync(outDir)) throw new Error(`${outDir} exists: a diff directory is written once`)
  const result = diffRuns(readRows(args.before), readRows(args.after))
  const identity = (dir: string) => JSON.parse(readFileSync(resolve(dir, 'identity.json'), 'utf8')) as Record<string, unknown>
  const identities = { before: identity(args.before), after: identity(args.after) }
  mkdirSync(outDir, { recursive: true })
  writeFileSync(resolve(outDir, 'diff.json'), JSON.stringify({ identities, ...result }, null, 2) + '\n', { flag: 'wx' })
  const brief = (value: Record<string, unknown>) => ({ label: value.label, server_cohort: (value.server_cohort as { cohort_id?: string })?.cohort_id, runner_commit: value.runner_commit, receiver_height_mode: value.receiver_height_mode, ops: value.ops })
  writeFileSync(resolve(outDir, 'diff.md'), render(result, { before: brief(identities.before), after: brief(identities.after) }), { flag: 'wx' })
  console.error(`[validation diff] ${result.moves.length} indicators → ${outDir}`)
}
