/** Spawn z9 enrichers in manifest order. Never import an enricher — several run main() on load. */

import { existsSync, readdirSync } from 'node:fs'
import { spawn } from 'node:child_process'
import { availableParallelism } from 'node:os'
import { resolve, dirname } from 'node:path'
import { parseArgs } from 'node:util'
import { pathToFileURL } from 'node:url'
import { buildPlan, layerForStep, parseScope, LAYERS, type Layer, type PlanStep } from './manifest.ts'

export interface ChainPaths {
  preparedDir: string
  enrichmentDir: string
  boundaries: string
  boundariesDir: string
  gtfsDir: string
  gtfsCacheDir: string
  asOfDate: string
  python: string
  repoRoot: string
  tsx: string
  jobs: number
}

const PIPELINE_DIR = resolve(import.meta.dirname, '..')
const REPO_ROOT = resolve(PIPELINE_DIR, '..')

export function commandFor(step: PlanStep, paths: ChainPaths): { argv: string[]; cwd: string } {
  const tsx = (script: string, args: string[]) => ({
    argv: [paths.tsx, resolve(PIPELINE_DIR, script), ...args],
    cwd: PIPELINE_DIR,
  })
  const prepared = ['--prepared-dir', paths.preparedDir]
  const enrichment = ['--enrichment-dir', paths.enrichmentDir]
  switch (step.kind) {
    case 'built-up':
      return {
        argv: [paths.python, resolve(paths.repoRoot, 'scripts/roads/build_built_up.py'),
               ...prepared, '--workers', String(paths.jobs)],
        cwd: paths.repoRoot,
      }
    case 'roads-europe':
      return tsx('enrich-roads-europe.ts', [...prepared, '--enrichment-dir', resolve(paths.enrichmentDir, 'global/eu-city-traffic')])
    case 'roads-national':
      return tsx(`enrich-roads-${step.cc}.ts`, [...prepared, ...enrichment, '--enrich-only'])
    case 'roads-national-policy':
      return tsx('enrich-roads-national-policy.ts', [...prepared, '--country', step.cc!])
    case 'roads-national-network':
      return tsx('enrich-roads-national-network.ts', [
        ...prepared, ...enrichment, '--enrich-only', '--country', step.cc!,
      ])
    case 'buildings-national':
      return tsx('enrich-buildings-national.ts', [...prepared, ...enrichment])
    case 'cities-roads':
      return tsx('enrich-cities-roads.ts', [...prepared, ...enrichment, '--boundaries-dir', paths.boundariesDir])
    case 'service-tree':
      return tsx('enrich-roads-service-tree.ts', prepared)
    case 'continuity':
      return tsx('enrich-roads-continuity-fill.ts', prepared)
    case 'taper':
      return tsx('enrich-roads-taper.ts', prepared)
    case 'industrial-global':
      return tsx('enrich-global-industrial.ts', [...prepared, '--enrichment-dir', resolve(paths.enrichmentDir, 'global')])
    case 'industrial-gem':
      return tsx('enrich-industrial-gem.ts', [...prepared, ...enrichment, '--boundaries', paths.boundaries])
    case 'industrial-special':
      return tsx('enrich-industrial-special.ts', [...prepared, ...enrichment, '--boundaries', paths.boundaries])
    case 'industrial-wind':
      return tsx('enrich-industrial-wind.ts', [...prepared, ...enrichment])
    case 'industrial-name':
      return tsx('enrich-industrial-name-heuristic.ts', prepared)
    case 'railways-cz':
      return tsx('enrich-railways-cz.ts', ['--source-dir', resolve(paths.enrichmentDir, 'cz'), ...prepared])
    case 'railways-gtfs':
      return tsx('enrich-railway-europe.ts', [
        '--source-dir', paths.gtfsDir, ...prepared,
        '--cache-dir', paths.gtfsCacheDir, '--country', step.cc!,
        '--registry', 'global', '--as-of-date', paths.asOfDate,
      ])
    case 'railways-national-gtfs':
      return tsx('enrich-railway-europe.ts', [
        '--source-dir', paths.enrichmentDir, ...prepared,
        '--cache-dir', paths.gtfsCacheDir, '--country', step.cc!,
        '--registry', 'national', '--as-of-date', paths.asOfDate,
      ])
    case 'railways-spatial':
      return tsx('enrich-railway-spatial.ts', [
        '--source-dir', paths.enrichmentDir, ...prepared, '--country', step.cc!,
      ])
    case 'railways-proxies':
      return tsx('enrich-railway-proxies.ts', prepared)
    case 'railways-parallel':
      return tsx('enrich-railways-parallel.ts', [...prepared, '--world'])
  }
}

function countSquareCountryCityRecords(preparedDir: string): { dirs: number; records: number } {
  const z9 = resolve(preparedDir, 'z9')
  let dirs = 0, records = 0
  if (!existsSync(z9)) return { dirs, records }
  for (const x of readdirSync(z9, { withFileTypes: true })) {
    if (!x.isDirectory()) continue
    for (const y of readdirSync(resolve(z9, x.name), { withFileTypes: true })) {
      if (!y.isDirectory()) continue
      dirs++
      if (existsSync(resolve(z9, x.name, y.name, 'square-country-city.bin'))) records++
    }
  }
  return { dirs, records }
}

export function squareCountryCityPreflight(preparedDir: string): string | null {
  const { dirs, records } = countSquareCountryCityRecords(preparedDir)
  if (!dirs) return `${preparedDir}: no z9 squares`
  if (records < dirs) return `square-country-city.bin ${records}/${dirs} — country bake must finish before enrichment`
  return null
}

function parseCli(argv: string[]) {
  const { values } = parseArgs({
    args: argv,
    strict: true,
    options: {
      scope: { type: 'string' },
      layer: { type: 'string' },
      'prepared-dir': { type: 'string' },
      'enrichment-dir': { type: 'string' },
      boundaries: { type: 'string' },
      'boundaries-dir': { type: 'string' },
      'gtfs-dir': { type: 'string' },
      'gtfs-cache-dir': { type: 'string' },
      'as-of-date': { type: 'string' },
      python: { type: 'string' },
      from: { type: 'string' },
      jobs: { type: 'string' },
      'dry-run': { type: 'boolean', default: false },
      'skip-square-country-city-check': { type: 'boolean', default: false },
    },
  })
  if (!values.scope || !values['prepared-dir'] || !values['enrichment-dir'] || !values.boundaries) {
    throw new Error(
      'usage: chain/run.ts --scope world [--layer roads|railways|industrial|buildings] --prepared-dir DIR --enrichment-dir DIR --boundaries CGAZ [--dry-run]',
    )
  }
  if (values.layer && !LAYERS.includes(values.layer as Layer)) throw new Error(`unknown layer '${values.layer}'`)
  const jobs = values.jobs === undefined ? availableParallelism() : Number(values.jobs)
  if (!Number.isInteger(jobs) || jobs < 1) throw new Error('--jobs must be a positive integer')
  const requiresGtfsDate = !values.layer || values.layer === 'railways'
  if (requiresGtfsDate && (!values['as-of-date'] || !/^\d{8}$/.test(values['as-of-date']))) {
    throw new Error('--as-of-date YYYYMMDD is required for a full or railways chain')
  }
  const preparedDir = resolve(values['prepared-dir'])
  const enrichmentDir = resolve(values['enrichment-dir'])
  const boundaries = resolve(values.boundaries)
  return {
    scope: parseScope(values.scope),
    layer: values.layer as Layer | undefined,
    dryRun: values['dry-run'] ?? false,
    skipSquareCountryCityCheck: values['skip-square-country-city-check'] ?? false,
    from: values.from,
    paths: {
      preparedDir,
      enrichmentDir,
      boundaries,
      boundariesDir: resolve(values['boundaries-dir'] ?? dirname(boundaries)),
      gtfsDir: resolve(values['gtfs-dir'] ?? resolve(enrichmentDir, 'global/gtfs')),
      gtfsCacheDir: resolve(values['gtfs-cache-dir'] ?? resolve(preparedDir, '../../gtfs-chain-cache')),
      asOfDate: values['as-of-date'] ?? '',
      python: values.python ?? resolve(REPO_ROOT, '.venv/bin/python'),
      repoRoot: REPO_ROOT,
      tsx: resolve(PIPELINE_DIR, 'node_modules/.bin/tsx'),
      jobs,
    } satisfies ChainPaths,
  }
}

export function spawnStep(argv: string[], cwd: string, preparedDir: string, layer: Layer, jobs?: number): Promise<number> {
  return new Promise((resolvePromise, reject) => {
    // Admin only touches roads, railways and industrial; buildings can run independently.
    const command = layer === 'buildings' ? argv
      : ['flock', '--shared', '--nonblock', resolve(preparedDir, '.square-country-city-build.lock'), ...argv]
    const child = spawn('flock', ['--exclusive', '--nonblock',
      resolve(preparedDir, `.enrichment-${layer}.lock`), ...command], {
      cwd,
      stdio: 'inherit',
      // National GTFS parses (AU Sydney) exceed V8's 4 GiB default heap; the layer's cgroup caps at 20 GiB.
      env: {
        ...process.env,
        NODE_OPTIONS: process.env.NODE_OPTIONS ?? '--max-old-space-size=8192',
        ...(jobs === undefined ? {} : { QM_ROAD_WORKERS: String(jobs) }),
      },
    })
    child.on('error', reject)
    child.on('close', code => resolvePromise(code ?? 1))
  })
}

export async function runChain(argv: string[]): Promise<number> {
  const cli = parseCli(argv)
  const plan = buildPlan(cli.scope, cli.layer)
  const start = cli.from ? plan.findIndex(step => step.id === cli.from) : 0
  if (cli.from && start < 0) throw new Error(`unknown --from step '${cli.from}'`)
  const selected = plan.slice(start)
  if (!cli.skipSquareCountryCityCheck && !cli.dryRun && selected.some(step => layerForStep(step) !== 'buildings')) {
    const problem = squareCountryCityPreflight(cli.paths.preparedDir)
    if (problem) {
      console.error(problem)
      return 2
    }
  }
  for (const step of selected) {
    const { argv: command, cwd } = commandFor(step, cli.paths)
    console.log(JSON.stringify({ step: step.id, phase: step.phase, argv: command }))
    if (cli.dryRun) continue
    const started = Date.now()
    const code = await spawnStep(command, cwd, cli.paths.preparedDir, layerForStep(step), cli.paths.jobs)
    console.log(JSON.stringify({ step: step.id, exit: code, elapsedSeconds: (Date.now() - started) / 1000 }))
    if (code !== 0) {
      console.error(`chain stopped at ${step.id} (exit ${code}); resume with --from ${step.id}`)
      return code
    }
  }
  console.log(JSON.stringify({ ok: true, steps: selected.map(step => step.id), dryRun: cli.dryRun }))
  return 0
}

async function main(): Promise<void> {
  process.exitCode = await runChain(process.argv.slice(2))
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
