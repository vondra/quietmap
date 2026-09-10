import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'
import { test } from 'node:test'
import { ROAD_NATIONAL, ROAD_NATIONAL_NETWORKS, ROAD_NATIONAL_POLICIES, buildPlan, parseScope } from './manifest.ts'
import { adminPreflight, commandFor, runChain, spawnStep } from './run.ts'

test('world plan keeps national roads, GTFS, proxies and world-covering heuristics', () => {
  const ids = buildPlan({ kind: 'world' }).map(step => step.id)
  assert.equal(ids[0], 'roads-built-up')
  assert.equal(ids.at(-1), 'roads-taper')
  for (const cc of [...ROAD_NATIONAL, ...ROAD_NATIONAL_POLICIES, ...ROAD_NATIONAL_NETWORKS]) assert.ok(ids.includes(`roads-${cc}`), cc)
  assert.ok(ids.includes('roads-us'))
  assert.ok(ids.includes('railways-gtfs-de'))
  assert.ok(ids.includes('railways-gtfs-us'))
  assert.ok(ids.includes('railways-national-gtfs-ar'))
  assert.ok(ids.includes('railways-national-gtfs-th'))
  assert.ok(ids.includes('railways-spatial-cn'))
  assert.ok(ids.includes('railways-spatial-in'))
  assert.ok(ids.includes('railways-proxies'))
  assert.ok(ids.includes('roads-service-tree'))
  assert.ok(ids.includes('industrial-gem'))
  for (const cc of ['ar', 'cl', 'co', 'id', 'pe', 'sa', 'th']) assert.ok(ids.includes(`roads-${cc}`))
})

test('a country label cannot silently authorize global writers', () => {
  assert.throws(() => parseScope('country:CZ'), /isolated prepared tree/)
})

test('independent layers partition the chain and retain within-layer order', () => {
  const world = buildPlan({ kind: 'world' })
  const layers = ['roads', 'railways', 'industrial', 'buildings'] as const
  const selected = layers.flatMap(layer => buildPlan({ kind: 'world' }, layer))
  assert.deepEqual(selected.map(s => s.id).sort(), world.map(s => s.id).sort())
  for (const layer of layers) {
    const steps = buildPlan({ kind: 'world' }, layer)
    assert.deepEqual(steps, world.filter(step => steps.some(s => s.id === step.id)))
  }
  const roads = buildPlan({ kind: 'world' }, 'roads').map(s => s.id)
  assert.ok(roads.indexOf('roads-cz') < roads.indexOf('roads-service-tree'))
  assert.ok(roads.indexOf('cities-roads') < roads.indexOf('roads-service-tree'))
})

test('command argv never imports enrichers and uses enrich-only on national roads', () => {
  const paths = {
    preparedDir: '/tmp/prepared',
    enrichmentDir: '/tmp/enrichment',
    boundaries: '/tmp/cgaz.geojson',
    boundariesDir: '/tmp',
    gtfsDir: '/tmp/gtfs',
    gtfsCacheDir: '/tmp/gtfs-cache',
    asOfDate: '20260909',
    python: '/tmp/python',
    repoRoot: '/tmp/repo',
    tsx: '/tmp/tsx',
  }
  const cz = commandFor(buildPlan({ kind: 'world' }).find(step => step.id === 'roads-cz')!, paths)
  assert.ok(cz.argv.includes('--enrich-only'))
  assert.ok(cz.argv.some(arg => arg.endsWith('enrich-roads-cz.ts')))
  const cd = commandFor(buildPlan({ kind: 'world' }).find(step => step.id === 'roads-cd')!, paths)
  assert.ok(cd.argv.some(arg => arg.endsWith('enrich-roads-national-policy.ts')))
  assert.deepEqual(cd.argv.slice(-2), ['--country', 'cd'])
  assert.ok(!cd.argv.includes('--enrich-only'))
  const br = commandFor(buildPlan({ kind: 'world' }).find(step => step.id === 'roads-br')!, paths)
  assert.ok(br.argv.some(arg => arg.endsWith('enrich-roads-national-network.ts')))
  assert.deepEqual(br.argv.slice(-2), ['--country', 'br'])
  assert.ok(br.argv.includes('--enrich-only'))
  const built = commandFor(buildPlan({ kind: 'world' }).find(step => step.id === 'roads-built-up')!, paths)
  assert.equal(built.argv[0], '/tmp/python')
  assert.ok(built.argv.some(arg => arg.endsWith('build_built_up.py')))
  assert.deepEqual(built.argv.slice(-2), ['--workers', '3'])
  const globalGtfs = commandFor(buildPlan({ kind: 'world' }).find(step => step.id === 'railways-gtfs-us')!, paths)
  assert.deepEqual(globalGtfs.argv.slice(-4), ['--registry', 'global', '--as-of-date', '20260909'])
  const nationalGtfs = commandFor(buildPlan({ kind: 'world' }).find(step => step.id === 'railways-national-gtfs-pl')!, paths)
  assert.ok(nationalGtfs.argv.includes('/tmp/enrichment'))
  assert.deepEqual(nationalGtfs.argv.slice(-4), ['--registry', 'national', '--as-of-date', '20260909'])
  const spatial = commandFor(buildPlan({ kind: 'world' }).find(step => step.id === 'railways-spatial-cn')!, paths)
  assert.ok(spatial.argv.some(arg => arg.endsWith('enrich-railway-spatial.ts')))
  assert.deepEqual(spatial.argv.slice(-2), ['--country', 'CN'])
})

test('admin preflight refuses a tree with squares but no admin.bin', () => {
  const work = mkdtempSync(resolve(tmpdir(), 'qm-chain-admin-'))
  mkdirSync(resolve(work, 'z9/276/173'), { recursive: true })
  writeFileSync(resolve(work, 'z9/276/173/roads.arrow'), '')
  assert.match(adminPreflight(work) ?? '', /admin\.bin 0\/1/)
  rmSync(work, { recursive: true, force: true })
})

test('dry-run world does not spawn and prints every planned id', async () => {
  const code = await runChain([
    '--scope', 'world',
    '--prepared-dir', '/tmp/missing-prepared',
    '--enrichment-dir', '/tmp/enrichment',
    '--boundaries', '/tmp/cgaz.geojson',
    '--as-of-date', '20260909',
    '--dry-run',
    '--skip-admin-check',
  ])
  assert.equal(code, 0)
})

test('unknown scope is rejected', () => {
  assert.throws(() => parseScope('europe'), /scope must be world/)
})

test('full and railway chains require an explicit generation date', async () => {
  const base = [
    '--scope', 'world',
    '--prepared-dir', '/tmp/missing-prepared',
    '--enrichment-dir', '/tmp/enrichment',
    '--boundaries', '/tmp/cgaz.geojson',
    '--dry-run',
  ]
  await assert.rejects(runChain(base), /as-of-date YYYYMMDD/)
  await assert.rejects(runChain([...base, '--layer', 'railways']), /as-of-date YYYYMMDD/)
  assert.equal(await runChain([...base, '--layer', 'buildings']), 0)
})

test('active admin blocks geography-dependent layers but permits independent building refinement', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'qm-chain-lock-'))
  const marker = resolve(work, 'enricher-ran')
  const holder = spawn('flock', ['--exclusive', resolve(work, '.admin-build.lock'),
    process.execPath, '-e', 'process.stdout.write("ready"); process.stdin.resume()'])
  await once(holder.stdout!, 'data')
  try {
    const code = await spawnStep([process.execPath, '-e',
      `require('node:fs').writeFileSync(${JSON.stringify(marker)}, '')`], work, work, 'roads')
    assert.notEqual(code, 0)
    assert.equal(existsSync(marker), false)
    assert.equal(await spawnStep([process.execPath, '-e',
      `require('node:fs').writeFileSync(${JSON.stringify(marker)}, '')`], work, work, 'buildings'), 0)
    assert.equal(existsSync(marker), true)
  } finally {
    holder.stdin!.end()
    await once(holder, 'exit')
    rmSync(work, { recursive: true, force: true })
  }
})


test('two writers of the same output layer cannot overlap', async () => {
  const work = mkdtempSync(resolve(tmpdir(), 'qm-chain-family-lock-'))
  const holder = spawn('flock', ['--exclusive', resolve(work, '.enrichment-buildings.lock'),
    process.execPath, '-e', 'process.stdout.write("ready"); process.stdin.resume()'])
  await once(holder.stdout!, 'data')
  try {
    assert.notEqual(await spawnStep([process.execPath, '-e', 'process.exit(0)'], work, work, 'buildings'), 0)
  } finally {
    holder.stdin!.end()
    await once(holder, 'exit')
    rmSync(work, { recursive: true, force: true })
  }
})
