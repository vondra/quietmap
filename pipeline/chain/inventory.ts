//! Survivability inventory (`run.ts --inventory`): every column of every arrow
//! layer on the REFERENCE HEX, joined against chain/columns.ts. ORPHAN columns
//! (live data with no declared parent) are flagged loudly and fail the exit
//! code — an orphan is data nothing would recreate after extract + chain.
//! Every producer TOKEN is additionally resolved against the world-scope
//! manifest plan (exact step ids / non-empty phase:layer families — never
//! substrings), so a renamed or deleted step trips the inventory the same day.

import { readdirSync, readFileSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { tableFromIPC } from 'apache-arrow'
import { DATA_YEAR as YEAR } from '../lib/data-year.js'
import { COLUMN_PARENTS, REQUIRED_PARENT_CHECKS } from './columns.js'
import { buildPlan, REPO_ROOT } from './manifest.js'
import { parseScope } from './scope.js'

/** Dobříš — the project reference hex (CLAUDE.md Data). */
export const REFERENCE_HEX = '841e309ffffffff'

interface PlanIndex {
  stepIds: Set<string>
  /** `${phase}|${layer}` combos that have at least one step. */
  phaseLayers: Set<string>
}

/** Resolve one producer token (grammar: chain/columns.ts module doc) against
 *  the world plan. Returns an error string, or null when the token is valid. */
function resolveParentToken(token: string, plan: PlanIndex): string | null {
  if (token.startsWith('extract:') || token.startsWith('aircraft-extract:')) return null
  if (token.startsWith('overture-parquet:')) {
    const script = token.slice('overture-parquet:'.length)
    return existsSync(resolve(REPO_ROOT, script))
      ? null
      : `overture parquet producer '${script}' does not exist`
  }
  const family = /^(national|city):([a-z]+)$/.exec(token)
  if (family) {
    if (plan.phaseLayers.has(`${family[1]}|${family[2]}`)) return null
    return `family '${token}' matches NO manifest step (phase ${family[1]}, layer ${family[2]})`
  }
  if (plan.stepIds.has(token)) return null
  return `'${token}' is not a manifest step id (nor extract:/aircraft-extract:/overture-parquet:/national:/city: token)`
}

export function runInventory(): number {
  // World scope resolves every step under its base id (no per-bbox fan-out
  // suffixes), which is exactly the id namespace the producer tokens use.
  const { steps } = buildPlan(parseScope('world'))
  const plan: PlanIndex = {
    stepIds: new Set(steps.map((s) => s.id)),
    phaseLayers: new Set(steps.map((s) => `${s.phase}|${s.layer}`)),
  }

  const hexDir = resolve(REPO_ROOT, 'data', 'prepared', YEAR, 'h3r4', REFERENCE_HEX)
  if (!existsSync(hexDir)) {
    console.error(`ERROR: reference hex dir not found: ${hexDir}`)
    return 1
  }
  const files = readdirSync(hexDir).filter((f) => f.endsWith('.arrow')).sort()
  console.log(`=== Survivability inventory — ${REFERENCE_HEX} (${YEAR}), ${files.length} layer files ===\n`)

  let orphans = 0
  let warns = 0
  const w = { file: 26, col: 38, flag: 7 }
  console.log(`${'file'.padEnd(w.file)} ${'column'.padEnd(w.col)} ${'flag'.padEnd(w.flag)} parents`)
  console.log('-'.repeat(w.file + w.col + w.flag + 60))
  const describe = (o: { parents: readonly string[]; note?: string }) =>
    o.parents.join(' + ') + (o.note ? `  (${o.note})` : '')
  for (const file of files) {
    const known = COLUMN_PARENTS[file]
    let cols: string[]
    try {
      cols = tableFromIPC(readFileSync(resolve(hexDir, file))).schema.fields.map((f) => f.name)
    } catch (err) {
      console.log(`${file.padEnd(w.file)} ${'<unreadable>'.padEnd(w.col)} ${'ERROR'.padEnd(w.flag)} ${err instanceof Error ? err.message : err}`)
      orphans++
      continue
    }
    if (!known) {
      // A whole file with no parent map = every column orphaned at once.
      for (const c of cols) {
        console.log(`${file.padEnd(w.file)} ${c.padEnd(w.col)} ${'ORPHAN!'.padEnd(w.flag)} !! no COLUMN_PARENTS entry for this FILE`)
        orphans++
      }
      continue
    }
    for (const c of cols) {
      const origin = known[c]
      if (!origin) {
        console.log(`${file.padEnd(w.file)} ${c.padEnd(w.col)} ${'ORPHAN!'.padEnd(w.flag)} !! live column with NO declared parent — nothing recreates it`)
        orphans++
      } else {
        const flag = origin.chainOnly ? 'CHAIN' : 'source'
        console.log(`${file.padEnd(w.file)} ${c.padEnd(w.col)} ${flag.padEnd(w.flag)} ${describe(origin)}`)
      }
    }
    // Declared-but-absent columns. A missing CHAIN-ONLY column on the enriched
    // reference hex is a loud WARN — it means the chain step that materializes
    // it has not run here (or stopped writing it); non-chain columns can
    // legitimately lag a schema addition, so they stay informational.
    for (const c of Object.keys(known)) {
      if (!cols.includes(c)) {
        if (known[c].chainOnly) {
          warns++
          console.log(`${file.padEnd(w.file)} ${c.padEnd(w.col)} ${'WARN'.padEnd(w.flag)} chain-only column NOT materialized on this hex — producer (${describe(known[c])}) has not run here`)
        } else {
          console.log(`${file.padEnd(w.file)} ${c.padEnd(w.col)} ${'absent'.padEnd(w.flag)} declared (${describe(known[c])}) but not materialized on this hex`)
        }
      }
    }
  }

  // Every producer token in the whole map must resolve against the manifest —
  // this is what makes the inventory non-tautological: a step rename breaks
  // HERE, not silently in a prose string.
  console.log(`\n=== Producer token validation (vs world-scope manifest plan, ${plan.stepIds.size} step ids) ===`)
  let badTokens = 0
  for (const [file, colsMap] of Object.entries(COLUMN_PARENTS)) {
    for (const [col, origin] of Object.entries(colsMap)) {
      for (const token of origin.parents) {
        const err = resolveParentToken(token, plan)
        if (err) {
          badTokens++
          console.log(`  FAIL  ${file} ${col}: ${err}`)
        }
      }
    }
  }
  if (badTokens === 0) console.log(`  PASS  every producer token resolves`)

  // Pinned survivability assertions — exact membership in `parents`, no substrings.
  console.log(`\n=== Required parent checks ===`)
  let failed = 0
  for (const check of REQUIRED_PARENT_CHECKS) {
    const origin = COLUMN_PARENTS[check.file]?.[check.column]
    const listed = origin !== undefined && origin.parents.includes(check.parent)
    const resolves = resolveParentToken(check.parent, plan) === null
    const ok = listed && resolves
    if (!ok) failed++
    console.log(
      `  ${ok ? 'PASS' : 'FAIL'}  ${check.file} ${check.column} → requires producer '${check.parent}'` +
        (ok ? '' : origin ? (listed ? ' (token does not resolve in the manifest)' : ` (parents: ${origin.parents.join(', ')})`) : ' (no COLUMN_PARENTS entry)'),
    )
  }

  const chainOwned = Object.values(COLUMN_PARENTS).reduce(
    (n, m) => n + Object.values(m).filter((o) => o.chainOnly).length,
    0,
  )
  console.log(`\n${orphans === 0 ? 'NO ORPHANS' : `!! ${orphans} ORPHAN column(s) — fix chain/columns.ts or add the missing chain step`}`)
  if (warns > 0) console.log(`!! ${warns} chain-only column(s) not materialized on the reference hex (WARN — see above)`)
  console.log(`${chainOwned} chain-owned column(s) exist only because enrichment ran — the set this chain restores.`)
  return orphans > 0 || failed > 0 || badTokens > 0 ? 1 : 0
}
