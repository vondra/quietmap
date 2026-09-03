import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rename,
  rm,
  stat,
  symlink,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WORLD_BASE_ZOOM, sha256Identity } from './generation-contract.mjs'
import { ALLOWED_LAYERS } from './routes/heatmap-shared.js'
import { createReadinessCheck } from './runtime-readiness.js'

const REFERENCE_HEX = '841e309ffffffff'
// This checkout's own env for the fixture — arbitrary but fixed, so every test that rewrites
// the manifest writes to the SAME per-env pointer the readiness check under test also reads
// (current.{TILE_ENV}.json, not the shared
// current.json merge head).
const TEST_TILE_ENV = 'dev2'

// The published quality profile of the z13 world, and the Wave-2 ladder it declares
// (engine/tile-painter/src/accuracy_contract.rs).
const ACCEPTED_PROFILE = 'w2-z13-accepted-v1'
const ACCEPTED_SCORER = {
  bias_db_max: 0.5,
  presence_mismatch_percent_max: 0.25,
  quiet_floor_db: 10,
  threshold_percent_max: { 0.5: 20, 1: 1, 3: 0.01, 6: 0.001 },
  unified_threshold_db: 6,
}
const PRODUCER_ROLES = {
  'cpu-cruise': 'stock',
  'gpu-airborne': 'stock',
  'gpu-surface': 'stock',
}

function worker(artifactFamily: string, binary: string, modelRole: string, resolvedRole: string) {
  return {
    artifact_family: artifactFamily,
    binary,
    model_role: modelRole,
    resolved_role: resolvedRole,
    selection_epoch: null,
  }
}

function makeModelRoleContract() {
  return {
    schema: 1,
    line_model_role_sha256: '1'.repeat(64),
    model_source_recipe_sha256: '2'.repeat(64),
    numerical_selection_record_sha256: null,
    output_abi_version: 3,
    role_spec_sha256: '3'.repeat(64),
    workers: {
      'cpu-cruise': worker(
        'aircraft-cpu-production', 'build-heatmap-aircraft', 'stock', 'aircraft-cpu-stock-v1',
      ),
      'gpu-airborne': worker('airborne-production', 'gpu-airborne', 'stock', 'airborne-stock-v1'),
      'gpu-surface': worker(
        'relevant-source-production', 'relevant-source-surface', 'stock', 'relevant-source-stock-v1',
      ),
    },
  }
}

/** One painting run's identity. A partial publish mixes two of these in one manifest. */
function acceptedGeneration({
  rasterGenerationId = 'b'.repeat(16),
  profileName = ACCEPTED_PROFILE,
  datasetYear = 2026,
} = {}) {
  const quality = {
    schema: 1,
    profile_name: profileName,
    product_commit: 'a'.repeat(40),
    dataset_year: datasetYear,
    model_role_contract: makeModelRoleContract(),
    numerical_environment: {},
    producer_requirements: { worker_model_roles: { ...PRODUCER_ROLES } },
    scorer_contract: structuredClone(ACCEPTED_SCORER),
    wave: 'w2',
  }
  const qualityProfileId = sha256Identity(quality)
  const identity = {
    schema: 1,
    zoom: WORLD_BASE_ZOOM,
    dataset_year: datasetYear,
    raster_generation_id: rasterGenerationId,
    quality_profile_id: qualityProfileId,
    quality_profile_name: profileName,
  }
  return {
    schema: 1,
    zoom: WORLD_BASE_ZOOM,
    dataset_year: datasetYear,
    raster_generation_id: rasterGenerationId,
    generation_id: sha256Identity(identity),
    quality_profile_id: qualityProfileId,
    quality_profile_name: profileName,
    quality,
  }
}

async function publisherProof(path: string, sha256: string) {
  const info = await stat(path, { bigint: true })
  return {
    schema: 'sha256-posix-stat-v1',
    sha256,
    dev: info.dev.toString(),
    ino: info.ino.toString(),
    size: info.size.toString(),
    mtime_ns: info.mtimeNs.toString(),
    ctime_ns: info.ctimeNs.toString(),
  }
}

async function writeFrontendAssetManifest(frontendDist: string, files: string[]) {
  const entries = []
  for (const file of files) {
    const contents = await readFile(join(frontendDist, file))
    entries.push({
      file,
      bytes: contents.length,
      sha256: createHash('sha256').update(contents).digest('hex'),
    })
  }
  await writeFile(
    join(frontendDist, 'asset-manifest.json'),
    JSON.stringify({ version: 1, files: entries }),
  )
}

async function readinessFixture() {
  const root = await mkdtemp(join(tmpdir(), '0db-ready-'))
  const sourceReaderPath = join(root, 'libsource_reader.so')
  const frontendDist = join(root, 'frontend')
  const h3r4Dir = join(root, 'h3r4')
  const pmtilesDir = join(root, 'pmtiles')
  await mkdir(join(h3r4Dir, REFERENCE_HEX), { recursive: true })
  await mkdir(pmtilesDir, { recursive: true })
  await mkdir(join(frontendDist, 'assets'), { recursive: true })
  await writeFile(sourceReaderPath, 'native-addon')
  await writeFile(
    join(frontendDist, 'index.html'),
    '<!doctype html><link rel="stylesheet" href="/assets/index-b2.css"><script type="module" src="/assets/index-a1.js"></script>',
  )
  await writeFile(join(frontendDist, 'assets/index-a1.js'), 'console.log("map")')
  await writeFile(join(frontendDist, 'assets/index-b2.css'), 'body { margin: 0 }')
  await writeFile(join(frontendDist, 'assets/AboutPage-lazy.js'), 'export default function AboutPage() {}')
  await writeFile(join(frontendDist, 'assets/compose.worker-c3.js'), 'self.onmessage = () => {}')
  await writeFrontendAssetManifest(frontendDist, [
    'index.html',
    'assets/index-a1.js',
    'assets/index-b2.css',
    'assets/AboutPage-lazy.js',
    'assets/compose.worker-c3.js',
  ])
  await writeFile(join(h3r4Dir, REFERENCE_HEX, 'roads.arrow'), 'arrow-data')

  const generation = acceptedGeneration()
  const layers: Record<string, {
    file: string
    bytes: number
    sha256: string
    generation?: unknown
    publisher_proof: Awaited<ReturnType<typeof publisherProof>>
  }> = {}
  for (const layer of ALLOWED_LAYERS) {
    const file = `${layer}.b1.pmtiles`
    const content = `pmtiles-${layer}`
    await writeFile(join(pmtilesDir, file), content)
    const sha256 = createHash('sha256').update(content).digest('hex')
    layers[layer] = {
      file,
      bytes: Buffer.byteLength(content),
      sha256,
      generation,
      publisher_proof: await publisherProof(join(pmtilesDir, file), sha256),
    }
  }
  await writeFile(
    join(pmtilesDir, `current.${TEST_TILE_ENV}.json`),
    JSON.stringify({ build: 'b1', layers }),
  )
  return {
    root,
    sourceReaderPath,
    frontendDist,
    h3r4Dir,
    pmtilesDir,
    generation,
    layers,
    tileEnv: TEST_TILE_ENV,
  }
}

test('readiness rejects a release whose static root is missing the SPA', async (t) => {
  // 2026-07-19: a frontend-less release served 404 on every page while /api/ready
  // reported healthy and start.sh activated it. The frontend gate locks that hole.
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  await rm(join(fixture.frontendDist, 'index.html'))

  const missing = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(missing.ready, false)
  assert.deepEqual(missing.failed, ['frontend'])
  assert.match(missing.errors.frontend ?? '', /index\.html/)

  // An EMPTY index.html is just as broken (a truncated frontend copy).
  await writeFile(join(fixture.frontendDist, 'index.html'), '')
  const empty = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(empty.ready, false)
  assert.deepEqual(empty.failed, ['frontend'])
})

test('readiness rejects a frontend cohort with a missing or changed entry asset', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  await rm(join(fixture.frontendDist, 'assets/index-a1.js'))

  const missing = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(missing.ready, false)
  assert.deepEqual(missing.failed, ['frontend'])
  assert.match(missing.errors.frontend ?? '', /index-a1\.js/)

  await writeFile(join(fixture.frontendDist, 'assets/index-a1.js'), '')
  const empty = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(empty.ready, false)
  assert.deepEqual(empty.failed, ['frontend'])
  assert.match(empty.errors.frontend ?? '', /frontend asset manifest/)
})

test('readiness rejects a frontend cohort with a missing lazy chunk', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  await rm(join(fixture.frontendDist, 'assets/AboutPage-lazy.js'))

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['frontend'])
  assert.match(result.errors.frontend ?? '', /AboutPage-lazy\.js/)
})

test('readiness validates artifacts, single-flights, and periodically reprobes the engine', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  let engineProbes = 0
  let now = 1_000
  const check = createReadinessCheck({
    ...fixture,
    engineProbe: async () => { engineProbes++ },
    filesystemCacheMs: 0,
    engineSuccessCacheMs: 10_000,
    now: () => now,
  })

  const [first, concurrent] = await Promise.all([check(), check()])
  assert.deepEqual(first, { ready: true, failed: [], errors: {} })
  assert.deepEqual(concurrent, first)
  assert.equal(engineProbes, 1)
  assert.equal((await check()).ready, true)
  assert.equal(engineProbes, 1, 'successful probes are briefly cached')
  now += 10_001
  assert.equal((await check()).ready, true)
  assert.equal(engineProbes, 2, 'an expired probe is repeated to detect a crashed worker')
})

test('readiness still serves a pre-generation manifest during the migration rollout', async (t) => {
  // The live prod pin is still such a manifest: no layer carries a generation, so the whole
  // contract block is skipped and the world it points at keeps serving at LEGACY_BASE_ZOOM.
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const layers = Object.fromEntries(Object.entries(fixture.layers).map(([layer, entry]) => {
    const { generation: _dropped, ...legacy } = entry
    return [layer, legacy]
  }))
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(result, { ready: true, failed: [], errors: {} })
})

test('a manifest from the retired base-plus-tier publisher is refused, not served', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const check = createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })
  for (const [field, value] of [
    ['generation', acceptedGeneration()],
    ['line_model_role_sha256', '1'.repeat(64)],
    ['tiers', { z13: { packs: [] } }],
    ['qualification_closure', { file: `qualification-${'a'.repeat(64)}.json`, sha256: 'a'.repeat(64) }],
  ] as const) {
    await writeFile(
      join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
      JSON.stringify({ build: 'b1', [field]: value, layers: fixture.layers }),
    )
    const result = await check()
    assert.equal(result.ready, false, `served a manifest carrying ${field}`)
    assert.match(result.errors.pmtiles ?? '', new RegExp(`retired top-level field ${field}`))
  }
})

test('a half-fenced manifest fails instead of serving unattested archives', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const { generation: _dropped, ...unattested } = fixture.layers.road
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers: { ...fixture.layers, road: unattested } }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /layer road has an invalid generation contract/)
})

test('one manifest is one dataset year, however valid each layer is alone', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const layers = {
    ...fixture.layers,
    road: { ...fixture.layers.road, generation: acceptedGeneration({ datasetYear: 2027 }) },
  }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /publishes dataset year 2027 into a 2026 manifest/)
})

test('development serves a structurally valid experiment that production rejects', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const experiment = acceptedGeneration({ profileName: 'w2-z13-accepted-v2' })
  const layers = Object.fromEntries(Object.entries(fixture.layers)
    .map(([layer, entry]) => [layer, { ...entry, generation: experiment }]))
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers }),
  )

  const development = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(development, { ready: true, failed: [], errors: {} })

  await writeFile(
    join(fixture.pmtilesDir, 'current.prod.json'),
    await readFile(join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`)),
  )
  const production = await createReadinessCheck({
    ...fixture,
    tileEnv: 'prod',
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(production.ready, false)
  assert.deepEqual(production.failed, ['pmtiles'])
  assert.match(production.errors.pmtiles ?? '', /published quality profile is unsupported/)
})

test('readiness rejects a PMTiles archive that disagrees with its manifest', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const total = fixture.layers.total
  await writeFile(join(fixture.pmtilesDir, total.file), 'wrong-size')

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /does not match manifest/)
})

test('readiness rejects a symlinked manifest pin leaf', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const manifestPath = join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`)
  const targetPath = join(fixture.pmtilesDir, '.manifest-target.json')
  await rename(manifestPath, targetPath)
  await symlink(targetPath, manifestPath)

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /not a regular file/)
})

test('readiness rejects a symlinked PMTiles archive leaf', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const archivePath = join(fixture.pmtilesDir, fixture.layers.total.file)
  const targetPath = join(fixture.pmtilesDir, '.archive-target.pmtiles')
  await rename(archivePath, targetPath)
  await symlink(targetPath, archivePath)

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /not a regular file/)
})

test('readiness rejects a manifest file name that the tile route would not serve', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const total = fixture.layers.total
  const road = fixture.layers.road
  fixture.layers.total = { ...road }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers: fixture.layers }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /does not match the served archive name/)
  // The aliased road archive exists and has the declared size; filename-to-route
  // consistency, rather than the existing size check, must reject this manifest.
  assert.equal(total.file, 'total.b1.pmtiles')
})

test('readiness rejects a manifest with a layer the tile route does not serve', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  fixture.layers.debug = { ...fixture.layers.road }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers: fixture.layers }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /unexpected layer debug/)
})

test('readiness rejects a per-layer build that disagrees with its archive file', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const layers = {
    ...fixture.layers,
    total: { ...fixture.layers.total, build: 'b2' },
  }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b2', layers }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /build does not match its archive file/)
})

test('readiness accepts a partial publish: one repainted layer, seven carried forward', async (t) => {
  // The normal steady state. The seven untouched entries keep build b1 AND the b1
  // generation they were published with; only `total` carries the new run's identity.
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const content = 'pmtiles-total-b2'
  const file = 'total.b2.pmtiles'
  await writeFile(join(fixture.pmtilesDir, file), content)
  const sha256 = createHash('sha256').update(content).digest('hex')
  const layers = {
    ...fixture.layers,
    total: {
      file,
      build: 'b2',
      bytes: Buffer.byteLength(content),
      sha256,
      generation: acceptedGeneration({ rasterGenerationId: 'c'.repeat(16) }),
      publisher_proof: await publisherProof(join(fixture.pmtilesDir, file), sha256),
    },
  }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b2', layers }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(result, { ready: true, failed: [], errors: {} })
})

test('readiness does not spawn the engine against missing prepared data', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  await rm(join(fixture.h3r4Dir, REFERENCE_HEX, 'roads.arrow'))
  let engineProbes = 0

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => { engineProbes++ },
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['prepared-data'])
  assert.equal(engineProbes, 0)
})

test('filesystem failures are retried immediately during startup', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const roadsPath = join(fixture.h3r4Dir, REFERENCE_HEX, 'roads.arrow')
  await rm(roadsPath)

  const check = createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 10_000,
  })
  assert.equal((await check()).ready, false)

  await writeFile(roadsPath, 'repaired-arrow-data')
  assert.equal((await check()).ready, true)
})

// The readiness contract was RELAXED on purpose (owner call, 2026-07-15): the boot-time
// stat-identity gate (dev/ino/ctime vs the publisher proof) caught zero real faults in
// production and broke the first deploy after a PLANNED storage move (Track D: every
// archive's inode changed, readiness went permanently red, release activation blocked).
// Content integrity belongs to pack-time validation, fsck and --rebind-verified; boot-time
// readiness keeps only the cheap operational invariants (manifest coherence + exact size).
// These tests LOCK IN the relaxation so a future "harden readiness" pass can't quietly
// re-introduce the planned-maintenance foot-gun without meeting this decision record.
test('readiness ACCEPTS a same-size archive replacement — identity is deliberately not verified at boot', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const total = fixture.layers.total
  const archivePath = join(fixture.pmtilesDir, total.file)
  const replacement = join(fixture.pmtilesDir, '.replacement.pmtiles')
  await writeFile(replacement, 'pmtiles-total')
  await rename(replacement, archivePath)

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, true)
})

test('readiness still rejects a malformed manifest sha256, but ignores proof mutations', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const total = fixture.layers.total
  const goodSha256 = total.sha256
  total.sha256 = 'NOT-A-SHA'
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers: fixture.layers }),
  )
  const badManifest = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(badManifest.ready, false)
  assert.match(badManifest.errors.pmtiles ?? '', /invalid sha256/)

  total.sha256 = goodSha256
  total.publisher_proof.sha256 = '0'.repeat(64)   // stale proof — boot-time readiness must not care
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers: fixture.layers }),
  )
  const staleProof = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(staleProof.ready, true)
})

test('readiness never opens PMTiles archive content (stat-only, always)', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  for (const entry of Object.values(fixture.layers)) {
    const archivePath = join(fixture.pmtilesDir, entry.file)
    await chmod(archivePath, 0o000)
    entry.publisher_proof = await publisherProof(archivePath, entry.sha256)
  }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', layers: fixture.layers }),
  )
  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(result, { ready: true, failed: [], errors: {} })
})

// Readiness gates on THIS deployment's
// own per-environment pin (current.{TILE_ENV}.json), selected via the shared
// tile-manifest-reader.ts, rather than the packer's shared current.json merge head.

test('readiness fails closed on a missing/unrecognized TILE_ENV', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))

  const missing = await createReadinessCheck({
    ...fixture,
    tileEnv: '',
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(missing.ready, false)
  assert.deepEqual(missing.failed, ['pmtiles'])
  assert.match(missing.errors.pmtiles ?? '', /TILE_ENV must be one of/)

  const bogus = await createReadinessCheck({
    ...fixture,
    tileEnv: 'staging',
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(bogus.ready, false)
  assert.match(bogus.errors.pmtiles ?? '', /TILE_ENV must be one of/)
})

test('readiness fails closed when this env pin is missing but a legacy current.json exists (un-seeded rollout)', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  // Simulate a checkout mid-Track-2-rollout: the per-env pin was never seeded, but the OLD
  // shared current.json is still sitting there from before the cutover.
  await rm(join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`))
  await writeFile(join(fixture.pmtilesDir, 'current.json'), JSON.stringify({ build: 'b1', layers: fixture.layers }))

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /legacy current\.json exists/)
  assert.match(result.errors.pmtiles ?? '', /seed it/)
})

test('readiness treats a genuinely fresh checkout (neither pin nor legacy manifest) as ordinary not-ready', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  await rm(join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`))

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.doesNotMatch(result.errors.pmtiles ?? '', /seed it/, 'a fresh checkout is not a seeding error')
})

test('readiness reads THIS environment pin, never the legacy current.json, when both exist', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  // A stale/foreign legacy current.json sitting next to a valid per-env pin must be
  // completely ignored — production must never
  // read what dev's shared merge head happens to say).
  await writeFile(join(fixture.pmtilesDir, 'current.json'), 'not even valid JSON')

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(result, { ready: true, failed: [], errors: {} })
})

test('readiness is independent across two environments sharing one pmtiles dir', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  // A second environment pin, deliberately broken, must not affect THIS environment's result
  // — each `current.{env}.json` is read in total isolation from every other one.
  await writeFile(join(fixture.pmtilesDir, 'current.prod.json'), '{ not json')

  const thisEnv = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(thisEnv, { ready: true, failed: [], errors: {} })

  const otherEnv = await createReadinessCheck({
    ...fixture,
    tileEnv: 'prod',
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(otherEnv.ready, false)
  assert.deepEqual(otherEnv.failed, ['pmtiles'])
})
