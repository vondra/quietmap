import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test, { type TestContext } from 'node:test'
import { createReadinessCheck, REFERENCE_SQUARE } from './runtime-readiness.js'

async function readinessFixture(t: TestContext) {
  const root = await mkdtemp(join(tmpdir(), '0db-readiness-'))
  t.after(async () => rm(root, { recursive: true, force: true }))

  const sourceReaderPath = join(root, 'libsource_reader.so')
  await writeFile(sourceReaderPath, 'native-addon')

  const frontendDist = join(root, 'frontend')
  await mkdir(join(frontendDist, 'assets'), { recursive: true })
  const indexHtml = '<!doctype html><title>fixture</title>'
  const bundle = 'console.log("fixture")'
  const manifestFiles = [
    { file: 'index.html', bytes: Buffer.byteLength(indexHtml), sha256: createHash('sha256').update(indexHtml).digest('hex') },
    { file: 'assets/app.js', bytes: Buffer.byteLength(bundle), sha256: createHash('sha256').update(bundle).digest('hex') },
  ]
  await writeFile(join(frontendDist, 'index.html'), indexHtml)
  await writeFile(join(frontendDist, 'assets', 'app.js'), bundle)
  await writeFile(
    join(frontendDist, 'asset-manifest.json'),
    JSON.stringify({ version: 1, files: manifestFiles }),
  )

  const preparedYearDir = join(root, 'prepared', '2026')
  await mkdir(join(preparedYearDir, REFERENCE_SQUARE), { recursive: true })
  await writeFile(join(preparedYearDir, REFERENCE_SQUARE, 'roads.arrow'), 'arrow-payload')

  const options = {
    engineProbe: async () => {},
    sourceReaderPath,
    frontendDist,
    preparedYearDir,
    filesystemCacheMs: 0,
    engineRetryMs: 0,
    engineSuccessCacheMs: 0,
  }
  return { root, sourceReaderPath, frontendDist, preparedYearDir, options }
}

test('readiness is ready when the addon, frontend manifest, and reference square roads.arrow all check out', async (t) => {
  const fixture = await readinessFixture(t)
  const result = await createReadinessCheck(fixture.options)()
  assert.equal(result.ready, true)
  assert.deepEqual(result.failed, [])
})

test('readiness reports the reference square: a missing roads.arrow fails prepared-data only', async (t) => {
  const fixture = await readinessFixture(t)
  await rm(join(fixture.preparedYearDir, REFERENCE_SQUARE, 'roads.arrow'))
  const result = await createReadinessCheck(fixture.options)()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['prepared-data'])
  assert.match(result.errors['prepared-data'] ?? '', /roads\.arrow/)
})

test('readiness reports a missing native addon as engine', async (t) => {
  const fixture = await readinessFixture(t)
  await rm(fixture.sourceReaderPath)
  const result = await createReadinessCheck({
    ...fixture.options,
    sourceReaderPath: join(fixture.root, 'libsource_reader.so'),
  })()
  assert.equal(result.ready, false)
  assert.ok(result.failed.includes('engine'))
})

test('readiness reports a corrupt frontend manifest as frontend', async (t) => {
  const fixture = await readinessFixture(t)
  await writeFile(join(fixture.frontendDist, 'assets', 'app.js'), 'tampered')
  const result = await createReadinessCheck(fixture.options)()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['frontend'])
})

test('readiness marks engine failed while the native probe throws, without invoking it twice per window', async (t) => {
  const fixture = await readinessFixture(t)
  let probes = 0
  const check = createReadinessCheck({
    ...fixture.options,
    engineProbe: async () => {
      probes++
      throw new Error('worker unavailable')
    },
    engineRetryMs: 60_000,
  })
  const first = await check()
  assert.equal(first.ready, false)
  assert.deepEqual(first.failed, ['engine'])
  assert.match(first.errors.engine ?? '', /worker unavailable/)
  await check()
  assert.equal(probes, 1)
})

test('readiness caches a passing native probe for the success window', async (t) => {
  const fixture = await readinessFixture(t)
  let probes = 0
  const check = createReadinessCheck({
    ...fixture.options,
    engineProbe: async () => {
      probes++
    },
    engineSuccessCacheMs: 60_000,
    filesystemCacheMs: 60_000,
  })
  assert.equal((await check()).ready, true)
  assert.equal((await check()).ready, true)
  assert.equal(probes, 1)
})

test('readiness never probes the engine when the filesystem is already bad', async (t) => {
  const fixture = await readinessFixture(t)
  let probes = 0
  const check = createReadinessCheck({
    ...fixture.options,
    engineProbe: async () => {
      probes++
    },
    preparedYearDir: join(fixture.root, 'no-such-prepared'),
  })
  const result = await check()
  assert.equal(result.ready, false)
  assert.deepEqual(result.failed, ['prepared-data'])
  assert.equal(probes, 0)
})
