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
import { W2_SPATIAL_POPULATION_SCOPES, sha256Identity } from './generation-contract.mjs'
import { ALLOWED_LAYERS } from './routes/heatmap-shared.js'
import { createReadinessCheck } from './runtime-readiness.js'

const REFERENCE_HEX = '841e309ffffffff'
// This checkout's own env for the fixture — arbitrary but fixed, so every test that rewrites
// the manifest writes to the SAME per-env pointer the readiness check under test also reads
// (docs/dev/checkout-restructure-plan.md Track 2: current.{TILE_ENV}.json, not the shared
// current.json merge head).
const TEST_TILE_ENV = 'dev2'

const STOCK_PRODUCER_ROLES = {
  'cpu-airborne': 'stock',
  'cpu-building': 'stock',
  'cpu-cruise': 'stock',
  'cpu-ground': 'stock',
  'cpu-industrial': 'stock',
  'gpu-airborne': 'stock',
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

function makeModelRoleContract(lineModelRole: 'w1' | 'w2-stride4') {
  const w1 = lineModelRole === 'w1'
  return {
    schema: 1,
    line_model_role_sha256: (w1 ? '1' : '4').repeat(64),
    model_source_recipe_sha256: '2'.repeat(64),
    numerical_selection_record_sha256: null,
    output_abi_version: 3,
    role_spec_sha256: '3'.repeat(64),
    workers: {
      'cpu-airborne': worker(
        'aircraft-cpu-production', 'build-heatmap-aircraft', 'stock', 'aircraft-cpu-stock-v1',
      ),
      'cpu-building': worker(
        'surface-cpu-production', 'build-heatmap-surface', 'stock', 'surface-cpu-stock-v1',
      ),
      'cpu-cruise': worker(
        'aircraft-cpu-production', 'build-heatmap-aircraft', 'stock', 'aircraft-cpu-stock-v1',
      ),
      'cpu-ground': worker(
        'surface-cpu-production', 'build-heatmap-surface', 'stock', 'surface-cpu-stock-v1',
      ),
      'cpu-industrial': worker(
        'surface-cpu-production', 'build-heatmap-surface', 'stock', 'surface-cpu-stock-v1',
      ),
      'gpu-airborne': worker(
        'airborne-production', 'gpu-airborne', 'stock', 'airborne-stock-v1',
      ),
      'gpu-line': worker(
        'surface-production',
        'gpu-surface',
        lineModelRole,
        w1 ? 'surface-w1-z12-accepted-v1' : 'surface-w2-z13-stride4-v1',
      ),
    },
  }
}

function baseGeneration(rasterGenerationId = 'b'.repeat(16)) {
  const selectedModelRoleContract = makeModelRoleContract('w1')
  const quality = {
    schema: 1,
    profile_name: 'w1-z12-accepted-v1',
    product_commit: 'a'.repeat(40),
    dataset_year: 2026,
    model_role_contract: selectedModelRoleContract,
    numerical_environment: { QM_W1_INDUSTRIAL_POLICY: 'adaptive-stride5' },
    producer_requirements: {
      worker_model_roles: { ...STOCK_PRODUCER_ROLES, 'gpu-line': 'w1' },
    },
    scorer_contract: {
      bias_db_max: 0.5,
      presence_mismatch_percent_max: 6,
      quiet_floor_db: 26,
      threshold_percent_max: { 1: 30, 2: 15, 6: 1.5 },
    },
    wave: 'w1',
  }
  const qualityProfileId = sha256Identity(quality)
  const identity = {
    schema: 1,
    deployment: 'base',
    zoom: 12,
    tier: '',
    dataset_year: 2026,
    raster_generation_id: rasterGenerationId,
    quality_profile_id: qualityProfileId,
    quality_profile_name: quality.profile_name,
    base_generation_id: null,
    base_quality_profile_id: null,
    base_quality_profile_name: null,
  }
  const generationId = sha256Identity(identity)
  return {
    schema: 1,
    deployment: 'base',
    zoom: 12,
    tier: '',
    dataset_year: 2026,
    generation_id: generationId,
    base_generation_id: generationId,
    raster_generation_id: rasterGenerationId,
    quality_profile_id: qualityProfileId,
    quality_profile_name: quality.profile_name,
    base_quality_profile_id: qualityProfileId,
    base_quality_profile_name: quality.profile_name,
    quality,
  }
}

function spatialGeneration(base: ReturnType<typeof baseGeneration>) {
  const quality = {
    schema: 1,
    profile_name: 'w2-z13-spatial-v1',
    product_commit: 'b'.repeat(40),
    dataset_year: 2026,
    model_role_contract: makeModelRoleContract('w2-stride4'),
    numerical_environment: {},
    producer_requirements: {
      worker_model_roles: { ...STOCK_PRODUCER_ROLES, 'gpu-line': 'w2-stride4' },
    },
    scorer_contract: {
      schema: 'w2-z13-spatial-scorer-v2',
      implementation_sha256: '4864c9f2925a2146a72e08f026deca75b3f099150d789c268e28ad2693ff638d',
      population_scopes: structuredClone(W2_SPATIAL_POPULATION_SCOPES),
      spatial_tolerance_pixels: 1,
      spatial_match_policy: 'symmetric-chebyshev-r1-directional-min-plus-histogram-capacity-v1',
      threshold_percent_max: { 0.5: 2, 1: 1, 3: 0.25, 6: 0.05 },
      quiet_threshold_percent_max: { 10: 0.01, 15: 0.001 },
      presence_multiplicity_percent_max: 0.25,
      bias_db_max: 0.5,
      warm_reference_fingerprint:
        'c92bc8ac4159c2759645cbf5948077ce024d55d633373a6b2aed5c1a7b547dc9',
    },
    wave: 'w2',
  }
  const qualityProfileId = sha256Identity(quality)
  const identity = {
    schema: 1,
    deployment: 'z13',
    zoom: 13,
    tier: 'z13',
    dataset_year: base.dataset_year,
    raster_generation_id: base.raster_generation_id,
    quality_profile_id: qualityProfileId,
    quality_profile_name: quality.profile_name,
    base_generation_id: base.generation_id,
    base_quality_profile_id: base.quality_profile_id,
    base_quality_profile_name: base.quality_profile_name,
  }
  return {
    ...identity,
    generation_id: sha256Identity(identity),
    quality,
  }
}

function unsupportedPublishedSpatialGeneration(base: ReturnType<typeof baseGeneration>) {
  const generation = spatialGeneration(base)
  generation.quality_profile_name = 'w2-z13-spatial-v2'
  const quality = generation.quality as { profile_name: string; scorer_contract: unknown }
  quality.profile_name = generation.quality_profile_name
  quality.scorer_contract = {
    bias_db_max: 0.5,
    presence_mismatch_percent_max: 0.25,
    quiet_floor_db: 10,
    threshold_percent_max: { 0.5: 20, 1: 1, 3: 0.01, 6: 0.001 },
    unified_threshold_db: 6,
  }
  generation.quality_profile_id = sha256Identity(generation.quality)
  generation.generation_id = sha256Identity({
    schema: generation.schema,
    deployment: generation.deployment,
    zoom: generation.zoom,
    tier: generation.tier,
    dataset_year: generation.dataset_year,
    raster_generation_id: generation.raster_generation_id,
    quality_profile_id: generation.quality_profile_id,
    quality_profile_name: generation.quality_profile_name,
    base_generation_id: generation.base_generation_id,
    base_quality_profile_id: generation.base_quality_profile_id,
    base_quality_profile_name: generation.base_quality_profile_name,
  })
  return generation
}

function unsupportedPublishedBaseGeneration() {
  const generation = baseGeneration()
  generation.quality_profile_name = 'w1-z12-accepted-v2'
  generation.base_quality_profile_name = generation.quality_profile_name
  generation.quality.profile_name = generation.quality_profile_name
  generation.quality_profile_id = sha256Identity(generation.quality)
  generation.base_quality_profile_id = generation.quality_profile_id
  generation.generation_id = sha256Identity({
    schema: generation.schema,
    deployment: generation.deployment,
    zoom: generation.zoom,
    tier: generation.tier,
    dataset_year: generation.dataset_year,
    raster_generation_id: generation.raster_generation_id,
    quality_profile_id: generation.quality_profile_id,
    quality_profile_name: generation.quality_profile_name,
    base_generation_id: null,
    base_quality_profile_id: null,
    base_quality_profile_name: null,
  })
  generation.base_generation_id = generation.generation_id
  return generation
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

  const layers: Record<string, {
    file: string
    bytes: number
    sha256: string
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
      publisher_proof: await publisherProof(join(pmtilesDir, file), sha256),
    }
  }
  const generation = baseGeneration()
  await writeFile(
    join(pmtilesDir, `current.${TEST_TILE_ENV}.json`),
    JSON.stringify({
      build: 'b1',
      generation,
      line_model_role_sha256: generation.quality.model_role_contract.line_model_role_sha256,
      layers,
    }),
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

async function writeTierBundle(
  fixture: Awaited<ReturnType<typeof readinessFixture>>,
  generation: ReturnType<typeof spatialGeneration>,
) {
  const layers = { ...fixture.layers }
  const tokens: string[] = []
  for (const layer of ALLOWED_LAYERS) {
    const token = `${layer}-z13-p001`
    const file = `${token}.b1.pmtiles`
    const content = `pmtiles-${token}`
    await writeFile(join(fixture.pmtilesDir, file), content)
    const sha256 = createHash('sha256').update(content).digest('hex')
    layers[token] = {
      file,
      bytes: Buffer.byteLength(content),
      sha256,
      publisher_proof: await publisherProof(join(fixture.pmtilesDir, file), sha256),
    }
    tokens.push(token)
  }
  const qualificationBytes = Buffer.from(JSON.stringify({
    schema: 'w2-qualification-closure-v2',
    generation_id: generation.generation_id,
  }))
  const qualificationSha256 = createHash('sha256').update(qualificationBytes).digest('hex')
  const qualificationFile = `qualification-${qualificationSha256}.json`
  await writeFile(join(fixture.pmtilesDir, qualificationFile), qualificationBytes)
  await chmod(join(fixture.pmtilesDir, qualificationFile), 0o444)
  return {
    layers,
    qualification_closure: {
      file: qualificationFile,
      sha256: qualificationSha256,
    },
    tiers: {
      z13: {
        packs: [{
          pack: 'p001',
          generation,
          coverage_r4: [REFERENCE_HEX],
          layers: tokens,
        }],
      },
    },
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

test('readiness keeps the pre-generation live tier head serveable during one migration rollout', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const layers = { ...fixture.layers }
  const tokens: string[] = []
  for (const layer of ALLOWED_LAYERS) {
    const token = `${layer}-z13-p001`
    const file = `${token}.b2.pmtiles`
    const content = `legacy-${token}`
    const sha256 = createHash('sha256').update(content).digest('hex')
    await writeFile(join(fixture.pmtilesDir, file), content)
    layers[token] = {
      file,
      bytes: Buffer.byteLength(content),
      sha256,
      publisher_proof: await publisherProof(join(fixture.pmtilesDir, file), sha256),
    }
    tokens.push(token)
  }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({
      build: 'b2', layers,
      tiers: { z13: { packs: [{
        pack: 'p001', build: 'b2', created_unix: 1,
        coverage_r4: [REFERENCE_HEX], layers: tokens,
      }] } },
    }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(result, { ready: true, failed: [], errors: {} })
})

test('readiness accepts a named W1 base with its W2 spatial tier and rejects a crossed anchor', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const bundle = await writeTierBundle(fixture, spatialGeneration(fixture.generation))
  const manifest = {
    build: 'b1',
    generation: fixture.generation,
    line_model_role_sha256:
      fixture.generation.quality.model_role_contract.line_model_role_sha256,
    ...bundle,
  }
  const manifestPath = join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`)
  await writeFile(manifestPath, JSON.stringify(manifest))
  const check = createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })

  assert.deepEqual(await check(), { ready: true, failed: [], errors: {} })

  const crossedBase = baseGeneration('c'.repeat(16))
  manifest.tiers.z13.packs[0].generation = spatialGeneration(crossedBase)
  await writeFile(manifestPath, JSON.stringify(manifest))
  const crossed = await check()
  assert.equal(crossed.ready, false)
  assert.deepEqual(crossed.failed, ['pmtiles'])
  assert.match(crossed.errors.pmtiles ?? '', /tier is not anchored to the live base generation/)
})

test('readiness rejects a self-consistent unsupported top-level quality profile', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const generation = unsupportedPublishedBaseGeneration()
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({
      build: 'b1',
      generation,
      line_model_role_sha256: generation.quality.model_role_contract.line_model_role_sha256,
      layers: fixture.layers,
    }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /published quality profile is unsupported/)
})

test('readiness rejects an unsupported quality profile in the first tier pack', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const generation = unsupportedPublishedSpatialGeneration(fixture.generation)
  const bundle = await writeTierBundle(fixture, generation)
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({
      build: 'b1',
      generation: fixture.generation,
      line_model_role_sha256:
        fixture.generation.quality.model_role_contract.line_model_role_sha256,
      ...bundle,
    }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /published quality profile is unsupported/)
})

test('readiness rejects an unsupported quality profile in a later tier pack', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const firstGeneration = spatialGeneration(fixture.generation)
  const bundle = await writeTierBundle(fixture, firstGeneration)
  const secondGeneration = unsupportedPublishedSpatialGeneration(fixture.generation)
  const secondTokens: string[] = []
  for (const layer of ALLOWED_LAYERS) {
    const token = `${layer}-z13-p002`
    const file = `${token}.b1.pmtiles`
    const content = `pmtiles-${token}`
    await writeFile(join(fixture.pmtilesDir, file), content)
    const sha256 = createHash('sha256').update(content).digest('hex')
    bundle.layers[token] = {
      file,
      bytes: Buffer.byteLength(content),
      sha256,
      publisher_proof: await publisherProof(join(fixture.pmtilesDir, file), sha256),
    }
    secondTokens.push(token)
  }
  bundle.tiers.z13.packs.push({
    pack: 'p002',
    generation: secondGeneration,
    coverage_r4: [REFERENCE_HEX],
    layers: secondTokens,
  })
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({
      build: 'b1',
      generation: fixture.generation,
      line_model_role_sha256:
        fixture.generation.quality.model_role_contract.line_model_role_sha256,
      ...bundle,
    }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '',
    /pack p002 has an unsupported published generation: .*published quality profile is unsupported/)
})

test('readiness requires an immutable content-addressed qualification closure for fenced tiers', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const bundle = await writeTierBundle(fixture, spatialGeneration(fixture.generation))
  const manifest = {
    build: 'b1',
    generation: fixture.generation,
    line_model_role_sha256:
      fixture.generation.quality.model_role_contract.line_model_role_sha256,
    ...bundle,
  }
  const manifestPath = join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`)
  const check = async () => createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()

  const missingReference = structuredClone(manifest)
  delete (missingReference as { qualification_closure?: unknown }).qualification_closure
  await writeFile(manifestPath, JSON.stringify(missingReference))
  assert.match((await check()).errors.pmtiles ?? '', /invalid qualification closure/)

  const closurePath = join(fixture.pmtilesDir, bundle.qualification_closure.file)
  await chmod(closurePath, 0o644)
  await writeFile(manifestPath, JSON.stringify(manifest))
  assert.match((await check()).errors.pmtiles ?? '', /is writable/)

  await writeFile(closurePath, '')
  await chmod(closurePath, 0o444)
  assert.match((await check()).errors.pmtiles ?? '', /invalid byte count/)

  await chmod(closurePath, 0o644)
  await writeFile(closurePath, 'tampered')
  await chmod(closurePath, 0o444)
  assert.match((await check()).errors.pmtiles ?? '', /sha256 differs from the manifest/)

  await rm(closurePath)
  const targetPath = join(fixture.pmtilesDir, 'qualification-target.json')
  await writeFile(targetPath, 'tampered')
  await chmod(targetPath, 0o444)
  await symlink(targetPath, closurePath)
  assert.match((await check()).errors.pmtiles ?? '', /is not a regular file/)
})

test('readiness rejects stale qualification evidence without generation-fenced tiers', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const manifestPath = join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`)
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  manifest.qualification_closure = {
    file: `qualification-${'a'.repeat(64)}.json`,
    sha256: 'a'.repeat(64),
  }
  await writeFile(manifestPath, JSON.stringify(manifest))

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.match(result.errors.pmtiles ?? '', /without fenced tiers/)
})

test('readiness rejects a valid tier contract in the top-level base slot', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const tierGeneration = spatialGeneration(fixture.generation)
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({
      build: 'b1',
      generation: tierGeneration,
      line_model_role_sha256:
        tierGeneration.quality.model_role_contract.line_model_role_sha256,
      layers: fixture.layers,
    }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /top-level generation must be a base contract/)
})

test('readiness rejects tier archives when the tiers index is absent', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const { layers } = await writeTierBundle(fixture, spatialGeneration(fixture.generation))
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({
      build: 'b1',
      generation: fixture.generation,
      line_model_role_sha256:
        fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers,
    }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /tier token .* is absent from the tiers index/)
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

test('readiness rejects a top-level line role identity not bound by its generation', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({
      build: 'b1',
      generation: fixture.generation,
      line_model_role_sha256: '0'.repeat(64),
      layers: fixture.layers,
    }),
  )

  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['pmtiles'])
  assert.match(result.errors.pmtiles ?? '', /differs from the base generation/)
})

test('readiness rejects a manifest file name that the tile route would not serve', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const total = fixture.layers.total
  const road = fixture.layers.road
  fixture.layers.total = { ...road }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b1', generation: fixture.generation,
      line_model_role_sha256: fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers: fixture.layers }),
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
    JSON.stringify({ build: 'b1', generation: fixture.generation,
      line_model_role_sha256: fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers: fixture.layers }),
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
    JSON.stringify({ build: 'b2', generation: fixture.generation,
      line_model_role_sha256: fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers }),
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

test('readiness accepts a consistent partial-publish manifest', async (t) => {
  const fixture = await readinessFixture()
  t.after(async () => rm(fixture.root, { recursive: true, force: true }))
  const content = 'pmtiles-total-b2'
  const file = 'total.b2.pmtiles'
  await writeFile(join(fixture.pmtilesDir, file), content)
  const layers = {
    ...fixture.layers,
    total: {
      file,
      build: 'b2',
      bytes: Buffer.byteLength(content),
      sha256: createHash('sha256').update(content).digest('hex'),
      publisher_proof: await publisherProof(
        join(fixture.pmtilesDir, file),
        createHash('sha256').update(content).digest('hex'),
      ),
    },
  }
  await writeFile(
    join(fixture.pmtilesDir, `current.${fixture.tileEnv}.json`),
    JSON.stringify({ build: 'b2', generation: fixture.generation,
      line_model_role_sha256: fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers }),
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
    JSON.stringify({ build: 'b1', generation: fixture.generation,
      line_model_role_sha256: fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers: fixture.layers }),
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
    JSON.stringify({ build: 'b1', generation: fixture.generation,
      line_model_role_sha256: fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers: fixture.layers }),
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
    JSON.stringify({ build: 'b1', generation: fixture.generation,
      line_model_role_sha256: fixture.generation.quality.model_role_contract.line_model_role_sha256,
      layers: fixture.layers }),
  )
  const result = await createReadinessCheck({
    ...fixture,
    engineProbe: async () => {},
    filesystemCacheMs: 0,
  })()
  assert.deepEqual(result, { ready: true, failed: [], errors: {} })
})

// Track 2 (docs/dev/checkout-restructure-plan.md): readiness now gates on THIS deployment's
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
  // completely ignored — this is the exact drift Track 2 exists to prevent (prod must never
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
