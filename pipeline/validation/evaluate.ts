/**
 * Evaluate validation runs against the W1 criteria (JSON, v2): freeze the panel manifest at the
 * baseline, report objectives and guard sub-checks, and gate a candidate (C1–C6, outcome).
 *
 * Run: node --import tsx validation/evaluate.ts --criteria <criteria.json> --baseline <run dir> \
 *   [--candidate <run dir> [--predecessor <run dir>] [--target <cohort> | --target deterministic:<evidence>] \
 *    [--geometry-sensitive] [--causes <json>] [--release-candidate] [--candidate-log <jsonl>]] --out <new dir>
 */
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { loadCriteria, type Criteria, type Variant } from './criteria.ts'
import { evaluateGuardSubchecks, subcheckRegressions, type SubcheckResult } from './guards.ts'
import { freezeManifest, type Manifest } from './manifest.ts'
import type { GuardPointRow, StationRow } from './report.ts'
import { changeRules, objectives, samples, targetMet, type Objectives, type RuleResult } from './rules.ts'
import { gate, movesOverThreshold, type CauseRecord, type Gate } from './verdict.ts'

type Run = { dir: string; rows: StationRow[]; guards: GuardPointRow[]; identity: Record<string, unknown> }
const jsonl = <T>(path: string): T[] => existsSync(path) ? readFileSync(path, 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line) as T) : []
const readRun = (dir: string): Run => ({
  dir: resolve(dir), rows: jsonl<StationRow>(resolve(dir, 'stations.jsonl')), guards: jsonl<GuardPointRow>(resolve(dir, 'guard-points.jsonl')),
  identity: JSON.parse(readFileSync(resolve(dir, 'identity.json'), 'utf8')) as Record<string, unknown>,
})

/** The panel is frozen once, next to the baseline run, and reused by every later evaluation. */
function frozenManifest(baseline: Run, criteria: Criteria): Manifest {
  const path = resolve(baseline.dir, 'manifest.json')
  if (existsSync(path)) {
    const manifest = JSON.parse(readFileSync(path, 'utf8')) as Manifest
    if (manifest.criteria_version !== criteria.criteria_version) throw new Error(`${path} was frozen under criteria ${manifest.criteria_version}, not ${criteria.criteria_version}`)
    return manifest
  }
  const manifest = freezeManifest(baseline.rows, criteria, basename(baseline.dir), baseline.identity.catalogue)
  writeFileSync(path, JSON.stringify(manifest, null, 1) + '\n', { flag: 'wx' })
  return manifest
}

function identitySummary(run: Run): Record<string, unknown> {
  const ops = run.identity.ops as Record<string, Record<string, unknown>> | undefined
  return {
    run: basename(run.dir), label: run.identity.label, runner_commit: run.identity.runner_commit, receiver_height_mode: run.identity.receiver_height_mode,
    native_sha256: (ops?.code?.native_sha256 as Record<string, string> | undefined)?.['libsource_reader.so'],
    native_commit: (ops?.code?.native_reproduction as { commit?: string } | null | undefined)?.commit ?? (ops?.code?.checkout as { head_at_run?: string } | undefined)?.head_at_run ?? null,
    prepared_content_sha256: ops?.prepared?.content_sha256, prepared_verified_current: ops?.prepared?.verified_current,
    rasters_sha256: ops?.rasters?.station_square_rasters_sha256,
  }
}

type ObjectiveRow = { cohort: string; role: string; metric: string; variant: Variant; column: string; objectives: Objectives }
function objectiveTable(run: Run, manifest: Manifest, criteria: Criteria): ObjectiveRow[] {
  const table: ObjectiveRow[] = []
  for (const cohort of criteria.cohorts) {
    const metrics = [...new Set([...(cohort.metrics_gated ?? []), ...(cohort.metrics_reported ?? []), 'lden', 'ln'])].filter(metric => /^l(den|n|d|e)$/.test(metric))
    for (const metric of metrics) {
      for (const variant of ['as_published', 'minus3'] as Variant[]) {
        const all = samples(run.rows, manifest, cohort.id, metric, variant).values
        const columns: Array<[string, typeof all]> = [['all', all], ['holdout', all.filter(value => value.holdout)], ['training', all.filter(value => !value.holdout)],
          ...[...new Set(all.map(value => value.network))].sort().map(network => [`network ${network}`, all.filter(value => value.network === network)] as [string, typeof all])]
        for (const [column, values] of columns) {
          const result = objectives(values, criteria)
          if (result) table.push({ cohort: cohort.id, role: cohort.role, metric, variant, column, objectives: result })
        }
      }
    }
  }
  return table
}

function guardResults(run: Run, manifest: Manifest, criteria: Criteria): SubcheckResult[] {
  return criteria.guards.flatMap(guard => evaluateGuardSubchecks(guard, run.guards, run.rows, manifest, criteria))
}

const estimate = (value: { value: number; ci95: [number, number] | null }) => `${value.value.toFixed(2)}${value.ci95 ? ` [${value.ci95[0].toFixed(2)}, ${value.ci95[1].toFixed(2)}]` : ''}`

const { values: args } = parseArgs({
  options: {
    criteria: { type: 'string' }, baseline: { type: 'string' }, candidate: { type: 'string' }, predecessor: { type: 'string' },
    target: { type: 'string' }, 'geometry-sensitive': { type: 'boolean', default: false }, causes: { type: 'string' },
    'release-candidate': { type: 'boolean', default: false }, 'candidate-log': { type: 'string' }, out: { type: 'string' },
  },
})
if (!args.criteria || !args.baseline || !args.out) {
  console.error('usage: evaluate.ts --criteria JSON --baseline RUN [--candidate RUN [--predecessor RUN] [--target COHORT|deterministic:REF] '
    + '[--geometry-sensitive] [--causes JSON] [--release-candidate] [--candidate-log JSONL]] --out NEW_DIR')
  process.exit(2)
}
const out = resolve(args.out)
if (existsSync(out)) throw new Error(`${out} exists: an evaluation is written once`)
const criteria = loadCriteria(resolve(args.criteria))
const baseline = readRun(args.baseline)
const manifest = frozenManifest(baseline, criteria)
const candidate = args.candidate ? readRun(args.candidate) : null
const predecessor = args.predecessor ? readRun(args.predecessor) : baseline
const scored = candidate ?? baseline

// Sealed release panel: networks the frozen baseline never saw are scored only for a release candidate, pooled.
const sealed = scored.rows.filter(row => !(row.key in manifest.stations))
const sealedAggregate = args['release-candidate'] && sealed.length
  ? objectiveTable({ ...scored, rows: sealed }, freezeManifest(sealed, criteria, 'sealed', null), criteria).filter(row => row.column === 'all') : null

const panel: Record<string, number> = {}
for (const station of Object.values(manifest.stations)) {
  const key = station.eligible ? station.cohort ?? `(no cohort) ${station.site_class}`
    : `(excluded) ${(station.exclusion_reason ?? '').split(':')[0].replace(/ of physical station .*/, '')}`
  panel[key] = (panel[key] ?? 0) + 1
}
const table = objectiveTable(scored, manifest, criteria)
const guardsScored = guardResults(scored, manifest, criteria)
let evaluation: { rules: Array<RuleResult & { against: string }>; ledger: unknown[]; moves: ReturnType<typeof movesOverThreshold>; guard_regressions: string[]; gate: Gate } | null = null
if (candidate) {
  const references: Array<[string, Run]> = [['predecessor', predecessor], ...(predecessor === baseline ? [] : [['baseline', baseline] as [string, Run]])]
  const evaluations = references.map(([against, reference]) => ({ against, ...changeRules(reference.rows, candidate.rows, manifest, criteria) }))
  const rules = evaluations.flatMap(entry => entry.results.map(result => ({ ...result, against: entry.against })))
  const causes = args.causes ? JSON.parse(readFileSync(resolve(args.causes), 'utf8')) as CauseRecord[] : []
  const moves = movesOverThreshold(predecessor.rows, candidate.rows, criteria, causes)
  const regressions = references.flatMap(([against, reference]) => subcheckRegressions(guardResults(reference, manifest, criteria), guardsScored, criteria).map(entry => `${entry} (vs ${against})`))
  const target = !args.target ? null : args.target.startsWith('deterministic:')
    ? { kind: 'deterministic' as const, evidence: args.target.slice('deterministic:'.length) }
    : { kind: 'cohort' as const, ...targetMet(predecessor.rows, candidate.rows, manifest, criteria, args.target, args['geometry-sensitive']) }
  evaluation = {
    rules, ledger: evaluations.flatMap(entry => entry.ledger.map(crossing => ({ ...crossing, against: entry.against }))), moves, guard_regressions: regressions,
    gate: gate({ target, rules, moves, guardRegressions: regressions, unscored: evaluations[0].unscored,
      guardErrors: candidate.guards.filter(point => point.error).map(point => `guard ${point.guard} ${point.label}: ${point.error}`), identity: candidate.identity }),
  }
  if (args['candidate-log']) {
    appendFileSync(resolve(args['candidate-log']), JSON.stringify({
      evaluated_utc: new Date().toISOString(), criteria_version: criteria.criteria_version, candidate: identitySummary(candidate),
      predecessor: basename(predecessor.dir), baseline: basename(baseline.dir), target: args.target ?? null, outcome: evaluation.gate.outcome, evaluation: basename(out),
    }) + '\n')
  }
}

const identities = { baseline: identitySummary(baseline), predecessor: candidate ? identitySummary(predecessor) : null, candidate: candidate ? identitySummary(candidate) : null }
mkdirSync(out, { recursive: true })
writeFileSync(resolve(out, 'evaluation.json'), JSON.stringify({
  criteria: resolve(args.criteria), criteria_version: criteria.criteria_version, identities, panel, sealed_stations: sealed.map(row => row.key),
  sealed_aggregate: sealedAggregate, objectives: table, guards: guardsScored, evaluation,
}, null, 1) + '\n', { flag: 'wx' })

const shownColumns = (row: ObjectiveRow) => ['all', 'holdout', 'training'].includes(row.column) || (row.objectives.n >= criteria.constants.n_min.value && row.column.startsWith('network'))
const lines = [
  `# W1 evaluation (criteria ${criteria.criteria_version})`, '',
  `Scored run: ${JSON.stringify(identitySummary(scored))}`, '',
  ...(candidate ? [`Predecessor: ${JSON.stringify(identities.predecessor)}`, '', `Baseline: ${JSON.stringify(identities.baseline)}`, ''] : []),
  `Frozen panel (${manifest.baseline_run}): ${Object.entries(panel).sort().map(([key, n]) => `${key} ${n}`).join('; ')}.`, '',
  `Sealed stations (networks unseen at baseline): ${sealed.length}.`, '',
  ...(evaluation ? [
    `## Outcome: ${evaluation.gate.outcome.toUpperCase()}`, '',
    '| check | holds | detail |', '| --- | --- | --- |',
    ...(['C1', 'C2', 'C3', 'C4', 'C5', 'C6'] as const).map(id => `| ${id} | ${evaluation!.gate[id].holds} | ${evaluation!.gate[id].detail} |`), '',
    `Incomplete: ${evaluation.gate.incomplete.join('; ') || 'none'}.`, '',
    '## Change rules (paired; fired rules and gated strata)', '',
    '| against | cohort | metric | variant | stratum | n | MAE A → B | bias A → B | Δbias [95 %] | fired |', '| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |',
    ...evaluation.rules.filter(rule => rule.stratum === 'all' || rule.fired.length).map(rule => `| ${rule.against} | ${rule.cohort} | ${rule.metric} | ${rule.variant} | ${rule.stratum} | ${rule.n} | `
      + `${rule.mae_before} → ${rule.mae_after} | ${rule.bias_before} → ${rule.bias_after} | ${estimate(rule.bias_change)} | ${rule.fired.join(', ') || '-'} |`), '',
    `## Moves over ${criteria.constants.move_explain_db.value} dB (C3)`, '',
    '| station | metric | layer | before | after | move dB | measured | cause record |', '| --- | --- | --- | --- | --- | --- | --- | --- |',
    ...evaluation.moves.map(move => `| ${move.station} | ${move.metric} | ${move.layer} | ${move.before} | ${move.after} | ${move.move_db} | ${move.measured ?? '-'} | ${move.explained ? 'yes' : 'MISSING'} |`), '',
    `Crossing ledger (sub-τ crossings of the ${criteria.constants.large_error_db.value}/${criteria.constants.severe_db.value} dB lines): ${evaluation.ledger.length} entries (evaluation.json).`, '',
  ] : []),
  '## Objectives (reported, never merge conditions)', '',
  '| cohort | role | metric | variant | column | n (networks, clusters) | bias dB [95 %] | MAE dB [95 %] | >6 dB | >10 dB | over 2u | U_c | O2 limit | O1 | O2 | O3 |',
  '| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |',
  ...table.filter(shownColumns).map(({ cohort, role, metric, variant, column, objectives: o }) => `| ${cohort} | ${role.split(' (')[0]} | ${metric} | ${variant} | ${column} | `
    + `${o.n} (${o.networks}, ${o.clusters}) | ${estimate(o.bias)} | ${estimate(o.mae)} | ${(100 * o.share_large).toFixed(0)} % | ${(100 * o.share_severe).toFixed(0)} % | `
    + `${o.over} | ${o.band_db} | ${o.o2_limit_db} | ${o.O1} | ${o.O2} | ${o.O3} |`), '',
  '## Guard sub-checks', '',
  '| guard | sub-check | state | quantity dB | detail |', '| --- | --- | --- | --- | --- |',
  ...guardsScored.map(check => `| ${check.guard} | ${check.subcheck} | ${check.state} | ${check.quantity_db ?? '-'} | ${check.detail} |`), '',
].join('\n')
writeFileSync(resolve(out, 'evaluation.md'), lines, { flag: 'wx' })
console.error(`[evaluation] → ${out}`)
