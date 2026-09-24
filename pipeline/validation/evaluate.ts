/**
 * Evaluate validation runs against the W1 criteria (JSON): freeze membership at the baseline, report
 * cohort objectives, and gate a candidate against its predecessor and the baseline (C1–C6).
 *
 * Run: node --import tsx validation/evaluate.ts --criteria <criteria.json> --baseline <run dir> \
 *   [--candidate <run dir> [--predecessor <run dir>] [--target <cohort> | --target deterministic:<evidence>] \
 *    [--causes <cause records json>] [--release-candidate]] --out <new dir>
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, resolve } from 'node:path'
import { parseArgs } from 'node:util'
import { loadCriteria, type Criteria } from './criteria.ts'
import { changeRules, cohortTable, freezeMembership, pooledStatistics, type CohortTable, type FrozenMembership, type RuleResult } from './gate.ts'
import type { StationRow } from './report.ts'
import { gate, guardRegressions, movesOverThreshold, type CauseRecord, type Gate, type Move, type Target } from './verdict.ts'

type Run = { dir: string; rows: StationRow[]; identity: Record<string, unknown> }
const readRun = (dir: string): Run => ({
  dir: resolve(dir),
  rows: readFileSync(resolve(dir, 'stations.jsonl'), 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line) as StationRow),
  identity: JSON.parse(readFileSync(resolve(dir, 'identity.json'), 'utf8')) as Record<string, unknown>,
})

/** Membership is frozen once, next to the baseline run, and reused by every later evaluation. */
function frozenMembership(baseline: Run, criteria: Criteria): FrozenMembership {
  const path = resolve(baseline.dir, 'membership.json')
  if (existsSync(path)) {
    const frozen = JSON.parse(readFileSync(path, 'utf8')) as FrozenMembership
    if (frozen.criteria_version !== criteria.criteria_version) {
      throw new Error(`${path} was frozen under criteria ${frozen.criteria_version}, not ${criteria.criteria_version}`)
    }
    return frozen
  }
  const frozen = freezeMembership(baseline.rows, criteria, basename(baseline.dir))
  writeFileSync(path, JSON.stringify(frozen, null, 1) + '\n', { flag: 'wx' })
  return frozen
}

function identitySummary(run: Run): Record<string, unknown> {
  const ops = run.identity.ops as Record<string, Record<string, unknown>> | undefined
  return {
    run: basename(run.dir), label: run.identity.label, runner_commit: run.identity.runner_commit,
    receiver_height_mode: run.identity.receiver_height_mode,
    native_sha256: (ops?.code?.native_sha256 as Record<string, string> | undefined)?.['libsource_reader.so'],
    native_commit: (ops?.code?.native_reproduction as { commit?: string } | null | undefined)?.commit ?? null,
    prepared_content_sha256: ops?.prepared?.content_sha256, prepared_verified_current: ops?.prepared?.verified_current,
    rasters_sha256: ops?.rasters?.station_square_rasters_sha256,
  }
}

const estimate = (value: { value: number; ci95: [number, number] | null }) => `${value.value.toFixed(1)}${value.ci95 ? ` [${value.ci95[0].toFixed(1)}, ${value.ci95[1].toFixed(1)}]` : ''}`

function renderCohorts(title: string, table: CohortTable, criteria: Criteria): string[] {
  const shown = table.filter(entry => ['all', 'holdout', 'training'].includes(entry.column)
    || (entry.gated && entry.statistics.n >= criteria.constants.n_min.value))
  return [
    `## ${title}`, '',
    '| cohort | role | metric | column | n (networks) | bias dB | MAE dB | T6 | T10 | over | U_c | ā | O2 limit | O1 | O2 | O3 |',
    '| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |',
    ...shown.map(({ cohort, role, metric, gated, column, statistics: s }) => `| ${cohort} | ${role} | ${metric}${gated ? ' (gated)' : ''} | ${column} | `
      + `${s.n} (${s.networks}) | ${estimate(s.bias)} | ${estimate(s.mae)} | ${(100 * s.tail6).toFixed(0)} % | ${(100 * s.tail10).toFixed(0)} % | ${s.over} | `
      + `${s.band_db} | ${s.allowance_db} | ${s.o2_limit_db} | ${s.objectives.O1} | ${s.objectives.O2} | ${s.objectives.O3} |`),
    '',
  ]
}

function renderRules(title: string, rules: RuleResult[]): string[] {
  return [
    `## ${title}`, '',
    '| cohort | metric | stratum | n | bias before → after | paired ΔMAE [95 % CI] | rules fired |', '| --- | --- | --- | --- | --- | --- | --- |',
    ...rules.map(rule => `| ${rule.cohort} | ${rule.metric} | ${rule.stratum} | ${rule.n} | ${rule.bias_before} → ${rule.bias_after} | ${estimate(rule.mae_change)} | ${rule.fired.join(', ') || '-'} |`),
    '',
  ]
}

const { values: args } = parseArgs({
  options: {
    criteria: { type: 'string' }, baseline: { type: 'string' }, candidate: { type: 'string' }, predecessor: { type: 'string' },
    target: { type: 'string' }, causes: { type: 'string' }, 'release-candidate': { type: 'boolean', default: false }, out: { type: 'string' },
  },
})
if (!args.criteria || !args.baseline || !args.out) {
  console.error('usage: evaluate.ts --criteria JSON --baseline RUN [--candidate RUN [--predecessor RUN] [--target COHORT|deterministic:REF] [--causes JSON] [--release-candidate]] --out NEW_DIR')
  process.exit(2)
}
const out = resolve(args.out)
if (existsSync(out)) throw new Error(`${out} exists: an evaluation is written once`)
const criteria = loadCriteria(resolve(args.criteria))
const baseline = readRun(args.baseline)
const frozen = frozenMembership(baseline, criteria)
const candidate = args.candidate ? readRun(args.candidate) : null
const predecessor = args.predecessor ? readRun(args.predecessor) : baseline
const scored = candidate ?? baseline

// H1: networks the frozen baseline never saw stay sealed; a release candidate reports them pooled.
const sealed = scored.rows.filter(row => !(row.key in frozen.stations))
const sealedFrozen = freezeMembership(sealed, criteria, 'sealed')
const sealedAggregate = args['release-candidate']
  ? criteria.metrics.periods_gated.map(metric => ({ metric, statistics: pooledStatistics(sealed, sealedFrozen, criteria, metric) }))
  : null

const membershipCounts: Record<string, number> = {}
for (const station of Object.values(frozen.stations)) {
  const key = station.cohort ?? `(none) ${station.reason.split(':')[0]}`
  membershipCounts[key] = (membershipCounts[key] ?? 0) + 1
}
const tables = { scored: cohortTable(scored.rows, frozen, criteria) }
let evaluation: { rules_vs_predecessor: RuleResult[]; rules_vs_baseline: RuleResult[]; moves: Move[]; guard_regressions: string[]; gate: Gate } | null = null
if (candidate) {
  const target: Target | null = !args.target ? null : args.target.startsWith('deterministic:')
    ? { deterministic: args.target.slice('deterministic:'.length) } : { cohort: args.target }
  const causes = args.causes ? JSON.parse(readFileSync(resolve(args.causes), 'utf8')) as CauseRecord[] : []
  const rulesVsPredecessor = changeRules(predecessor.rows, candidate.rows, frozen, criteria)
  const rulesVsBaseline = predecessor === baseline ? [] : changeRules(baseline.rows, candidate.rows, frozen, criteria)
  const targetRules = target && 'cohort' in target
    ? rulesVsPredecessor.find(rule => rule.cohort === target.cohort && rule.stratum === 'all' && rule.metric === 'lden') ?? null : null
  const moves = movesOverThreshold(predecessor.rows, candidate.rows, criteria, causes)
  const regressions = guardRegressions(predecessor.rows, candidate.rows)
  evaluation = {
    rules_vs_predecessor: rulesVsPredecessor, rules_vs_baseline: rulesVsBaseline, moves, guard_regressions: regressions,
    gate: gate({ criteria, target, targetRules, predecessorRules: rulesVsPredecessor, baselineRules: rulesVsBaseline, moves, guardRegressions: regressions, identity: candidate.identity }),
  }
}

const identities = { baseline: identitySummary(baseline), predecessor: candidate ? identitySummary(predecessor) : null, candidate: candidate ? identitySummary(candidate) : null }
mkdirSync(out, { recursive: true })
writeFileSync(resolve(out, 'evaluation.json'), JSON.stringify({
  criteria: resolve(args.criteria), criteria_version: criteria.criteria_version, identities, membership_counts: membershipCounts,
  sealed_stations: sealed.map(row => row.key), sealed_aggregate: sealedAggregate, release_candidate: args['release-candidate'], cohorts: tables.scored, evaluation,
}, null, 1) + '\n', { flag: 'wx' })
const lines = [
  `# W1 evaluation (criteria ${criteria.criteria_version})`, '',
  `Scored run: ${JSON.stringify(identitySummary(scored))}`, '',
  ...(candidate ? [`Predecessor: ${JSON.stringify(identities.predecessor)}`, '', `Baseline: ${JSON.stringify(identities.baseline)}`, ''] : []),
  `Frozen membership (${frozen.baseline_run}): ${Object.entries(membershipCounts).sort().map(([cohort, n]) => `${cohort} ${n}`).join('; ')}.`, '',
  `Sealed stations (networks unseen at baseline, H1): ${sealed.length}${sealed.length ? ' — scored only for release candidates, in aggregate' : ''}.`
    + (sealedAggregate ? ' ' + sealedAggregate.map(({ metric, statistics }) => statistics
      ? `${metric}: n ${statistics.n}, bias ${estimate(statistics.bias)}, MAE ${estimate(statistics.mae)}` : `${metric}: none`).join('; ') : ''), '',
  ...renderCohorts('Cohorts: objectives O1–O3 (all | holdout | training; gated network strata with n ≥ n_min)', tables.scored, criteria),
  ...(evaluation ? [
    `## Gate: ${evaluation.gate.verdict.toUpperCase()}`, '',
    '| check | holds | detail |', '| --- | --- | --- |',
    ...(['C1', 'C2', 'C3', 'C4', 'C5', 'C6'] as const).map(id => `| ${id} | ${evaluation!.gate[id].holds} | ${evaluation!.gate[id].detail} |`), '',
    ...renderRules('Change rules vs predecessor', evaluation.rules_vs_predecessor),
    ...(evaluation.rules_vs_baseline.length ? renderRules('Change rules vs campaign baseline (cumulative)', evaluation.rules_vs_baseline) : []),
    `## Moves over ${criteria.constants.move_explain_db.value} dB (C3)`, '',
    '| station | metric | layer | before | after | move dB | measured | cause record |', '| --- | --- | --- | --- | --- | --- | --- | --- |',
    ...evaluation.moves.map(move => `| ${move.station} | ${move.metric} | ${move.layer} | ${move.before} | ${move.after} | ${move.move_db} | ${move.measured ?? '-'} | ${move.explained ? 'yes' : 'MISSING'} |`),
    '',
  ] : []),
].join('\n')
writeFileSync(resolve(out, 'evaluation.md'), lines, { flag: 'wx' })
console.error(`[evaluation] → ${out}`)
