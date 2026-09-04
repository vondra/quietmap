import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rename, rm, stat, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test, { type TestContext } from 'node:test'
import { createValidationCohortProvider } from './validation-cohort.js'

async function cohortFixture(t: TestContext) {
  const root = await mkdtemp(join(tmpdir(), '0db-validation-cohort-'))
  t.after(async () => rm(root, { recursive: true, force: true }))

  const runtimeRoot = join(root, 'runtime')
  const preparedYearDir = join(root, 'prepared')
  const runtimePath = join(runtimeRoot, 'server.mjs')
  const preparedPath = join(preparedYearDir, 'roads.arrow')
  const sourceReaderPath = join(root, 'libsource_reader.so')
  await mkdir(runtimeRoot)
  await mkdir(preparedYearDir)
  await writeFile(runtimePath, 'export const model = "a"\n')
  await writeFile(preparedPath, 'prepared-a')
  await writeFile(sourceReaderPath, 'native-addon')

  const providerOptions = {
    cacheMs: 0,
    dataYear: '2099',
    preparedYearDir,
    preparedAuxiliaryInputs: [],
    runtimeIdentityInputs: [],
    runtimeRoot,
    sourceReaderPath,
  }
  return {
    preparedYearDir,
    preparedPath,
    runtimePath,
    providerOptions,
    provider: createValidationCohortProvider(providerOptions),
  }
}

test('validation cohort is stable while runtime and prepared inputs are unchanged', async (t) => {
  const fixture = await cohortFixture(t)

  const first = await fixture.provider()
  const second = await fixture.provider()

  assert.deepEqual(second, first)
  assert.match(first.cohort_id, /^[a-f0-9]{64}$/)
})

test('validation cohort refuses inputs changed before its first fingerprint', async (t) => {
  const fixture = await cohortFixture(t)
  const initial = await stat(fixture.runtimePath)
  const modelProcessStartedAtMs = Math.ceil(Math.max(initial.mtimeMs, initial.ctimeMs)) + 1_000
  const provider = createValidationCohortProvider({
    ...fixture.providerOptions,
    modelProcessStartedAtMs,
  })
  await writeFile(fixture.runtimePath, 'export const model = "changed-before-cohort"\n')
  const changedAt = new Date(modelProcessStartedAtMs + 1_000)
  await utimes(fixture.runtimePath, changedAt, changedAt)

  await assert.rejects(provider(), /server process loaded; restart required/)
})

test('validation cohort refuses runtime changed under a live process', async (t) => {
  const fixture = await cohortFixture(t)
  const before = await fixture.provider()

  await writeFile(fixture.runtimePath, 'export const model = "b"\n')
  await assert.rejects(fixture.provider(), /restart required/)
  const after = await createValidationCohortProvider({
    ...fixture.providerOptions,
    modelProcessStartedAtMs: Date.now() + 1_000,
  })()

  assert.notEqual(after.runtime_sha256, before.runtime_sha256)
  assert.equal(after.prepared_sha256, before.prepared_sha256)
  assert.notEqual(after.cohort_id, before.cohort_id)
})

test('validation cohort refuses a same-size prepared replacement until restart', async (t) => {
  const fixture = await cohortFixture(t)
  const before = await fixture.provider()
  const beforeStat = await stat(fixture.preparedPath)
  const replacementPath = join(fixture.preparedYearDir, 'roads.arrow.next')
  const replacement = 'prepared-b'
  assert.equal(Buffer.byteLength(replacement), beforeStat.size)

  await writeFile(replacementPath, replacement)
  await rename(replacementPath, fixture.preparedPath)
  const afterStat = await stat(fixture.preparedPath)
  assert.notEqual(
    `${afterStat.dev}:${afterStat.ino}`,
    `${beforeStat.dev}:${beforeStat.ino}`,
    'fixture must replace the inode rather than modify the original file',
  )

  await assert.rejects(fixture.provider(), /restart required/)
  const after = await createValidationCohortProvider({
    ...fixture.providerOptions,
    modelProcessStartedAtMs: Date.now() + 1_000,
  })()
  assert.equal(after.runtime_sha256, before.runtime_sha256)
  assert.notEqual(after.prepared_sha256, before.prepared_sha256)
  assert.notEqual(after.cohort_id, before.cohort_id)
})

test('validation cohort also binds raster and sidecar inputs', async (t) => {
  const fixture = await cohortFixture(t)
  const auxiliaryPath = join(fixture.preparedYearDir, '..', 'model-sidecar.bin')
  await writeFile(auxiliaryPath, 'sidecar-a')
  const options = {
    ...fixture.providerOptions,
    preparedAuxiliaryInputs: [{ label: 'model-sidecar', path: auxiliaryPath }],
    modelProcessStartedAtMs: Date.now() + 1_000,
  }
  const provider = createValidationCohortProvider(options)
  const before = await provider()

  await writeFile(auxiliaryPath, 'sidecar-b')
  await assert.rejects(provider(), /restart required/)
  const after = await createValidationCohortProvider({
    ...options,
    modelProcessStartedAtMs: Date.now() + 1_000,
  })()
  assert.notEqual(after.prepared_sha256, before.prepared_sha256)
})

test('validation cohort throttles recomputation but rechecks after its public TTL', async (t) => {
  const fixture = await cohortFixture(t)
  const provider = createValidationCohortProvider({ ...fixture.providerOptions, cacheMs: 30 })
  const before = await provider()
  await writeFile(fixture.runtimePath, 'export const model = "changed-during-cache"\n')

  assert.deepEqual(await provider(), before, 'requests inside the TTL reuse the resolved fingerprint')
  await new Promise(resolve => setTimeout(resolve, 40))
  await assert.rejects(provider(), /restart required/)
})
