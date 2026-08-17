//! Direct and adversarial tests for durable popup publication transactions.

import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import {
  chmod, mkdtemp, mkdir, readFile, rm, stat, writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import Fastify from 'fastify'
import { registerPopupPublishIpc } from './popup-publish-ipc.js'
import { noiseOnflyV2Routes } from './routes/noise-onfly-v2.js'
import { tilesManifestRoutes } from './routes/tiles-manifest.js'
import {
  createPublishedLineModel,
  type PublishedLineModel,
} from './published-line-model.js'
import { createRuntimePublishedLineModel } from './published-line-model-runtime.js'
import { ALLOWED_LAYERS } from './routes/heatmap-shared.js'

const OLD_DIGEST = '1'.repeat(64)
const NEW_DIGEST = '2'.repeat(64)
const TOKEN = 'a'.repeat(64)
const TRANSACTION = 'b'.repeat(64)
const TEST_TILE_ENV = 'dev1'

type Fixture = Awaited<ReturnType<typeof fixture>>

function sha256(contents: string | Buffer): string {
  return createHash('sha256').update(contents).digest('hex')
}

async function manifest(
  pmtilesDir: string,
  build: string,
  lineModelRoleSha256: string,
): Promise<string> {
  const layers: Record<string, {
    file: string
    build: string
    bytes: number
    sha256: string
  }> = {}
  for (const layer of ALLOWED_LAYERS) {
    const file = `${layer}.${build}.pmtiles`
    const contents = `archive-${layer}-${build}`
    await writeFile(join(pmtilesDir, file), contents)
    layers[layer] = {
      file,
      build,
      bytes: Buffer.byteLength(contents),
      sha256: sha256(contents),
    }
  }
  return JSON.stringify({
    build,
    line_model_role_sha256: lineModelRoleSha256,
    layers,
  })
}

async function writeReleaseIdentity(f: Fixture, digest: string): Promise<void> {
  await writeFile(f.releaseIdentityPath, `${JSON.stringify({
    schema: 1,
    artifact_family: 'popup-production',
    resolved_role: digest === OLD_DIGEST ? 'popup-stock-v1' : 'popup-h0-e7',
    model_role: digest === OLD_DIGEST ? 'stock' : 'h0',
    selection_epoch: digest === OLD_DIGEST ? null : 7,
    line_model_role_sha256: digest,
    artifact_manifest_sha256: '3'.repeat(64),
    native_addon_sha256: sha256(await readFile(f.sourceReaderPath)),
  })}\n`)
}

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), 'popup-publish-ipc-'))
  const pmtilesDir = join(root, 'pmtiles')
  await mkdir(pmtilesDir)
  const sourceReaderPath = join(root, 'libsource_reader.so')
  const releaseIdentityPath = join(root, 'popup-release-identity.json')
  const statePath = join(root, 'state', 'published-line-model.dev1.json')
  const tokenPath = join(root, '.popup-publish-token')
  await writeFile(sourceReaderPath, 'native-addon-fixture')
  await writeFile(tokenPath, `${TOKEN}\n`, { mode: 0o600 })
  await chmod(tokenPath, 0o600)
  const oldText = await manifest(pmtilesDir, 'b1', OLD_DIGEST)
  const newText = await manifest(pmtilesDir, 'b2', NEW_DIGEST)
  const currentPath = join(pmtilesDir, `current.${TEST_TILE_ENV}.json`)
  await writeFile(currentPath, oldText)
  const f = {
    root,
    pmtilesDir,
    sourceReaderPath,
    releaseIdentityPath,
    statePath,
    tokenPath,
    currentPath,
    oldText,
    newText,
  }
  await writeReleaseIdentity(f, OLD_DIGEST)
  return f
}

async function manager(f: Fixture): Promise<PublishedLineModel> {
  return createPublishedLineModel({
    releaseIdentityPath: f.releaseIdentityPath,
    sourceReaderPath: f.sourceReaderPath,
    statePath: f.statePath,
    tokenPath: f.tokenPath,
    pmtilesDir: f.pmtilesDir,
    tileEnv: TEST_TILE_ENV,
  })
}

function prepareBody(f: Fixture) {
  return {
    schema: 1,
    transaction_id: TRANSACTION,
    previous_manifest_sha256: sha256(f.oldText),
    next_manifest_text: f.newText,
  }
}

test('PREPARE survives restart and only the matching popup release can COMMIT', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const oldServer = await manager(f)

  assert.throws(() => oldServer.assertReady(), /bootstrap PREPARE\/COMMIT required/)
  assert.equal(oldServer.mapManifest(), null)
  const prepared = await oldServer.prepare(prepareBody(f))
  assert.equal(prepared.phase, 'prepared')
  assert.equal(prepared.manifest_sha256, sha256(f.oldText))
  assert.equal(oldServer.mapManifest()?.build, 'b1')
  assert.throws(() => oldServer.assertPopupAvailable(), /publication prepared/)

  const duplicate = await oldServer.prepare(prepareBody(f))
  assert.equal(duplicate.idempotent, true)

  await writeFile(f.currentPath, f.newText)
  await assert.rejects(
    oldServer.commit({ schema: 1, transaction_id: TRANSACTION, result: 'next' }),
    /popup release differs/,
  )
  assert.equal(oldServer.mapManifest()?.build, 'b1', 'map stays on the previous in-memory snapshot')

  await writeReleaseIdentity(f, NEW_DIGEST)
  const newServer = await manager(f)
  assert.equal(newServer.mapManifest()?.build, 'b1', 'a restarted PREPARED server retains the old map')
  assert.throws(() => newServer.assertPopupAvailable(), /publication prepared/)
  const committed = await newServer.commit({
    schema: 1,
    transaction_id: TRANSACTION,
    result: 'next',
  })
  assert.equal(committed.phase, 'committed')
  assert.equal(committed.manifest_sha256, sha256(f.newText))
  assert.equal(newServer.mapManifest()?.build, 'b2')
  assert.doesNotThrow(() => newServer.assertPopupAvailable())

  const afterRestart = await manager(f)
  assert.equal(afterRestart.mapManifest()?.build, 'b2')
  assert.doesNotThrow(() => afterRestart.assertReady())
  const idempotent = await afterRestart.commit({
    schema: 1,
    transaction_id: TRANSACTION,
    result: 'next',
  })
  assert.equal(idempotent.idempotent, true)
  await assert.rejects(
    afterRestart.commit({ schema: 1, transaction_id: TRANSACTION, result: 'previous' }),
    /result differs/,
  )
})

test('failed publication can COMMIT the previous side without changing identity', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const oldServer = await manager(f)
  await oldServer.prepare(prepareBody(f))
  const restored = await oldServer.commit({
    schema: 1,
    transaction_id: TRANSACTION,
    result: 'previous',
  })
  assert.equal(restored.build, 'b1')
  assert.doesNotThrow(() => oldServer.assertPopupAvailable())
})

test('an unobserved pointer flip and corrupt durable state both fail closed', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const initial = await manager(f)
  await initial.prepare(prepareBody(f))
  await initial.commit({ schema: 1, transaction_id: TRANSACTION, result: 'previous' })

  await writeFile(f.currentPath, f.newText)
  const unobserved = await manager(f)
  assert.equal(unobserved.mapManifest(), null)
  assert.throws(() => unobserved.assertReady(), /changed without a PREPARE\/COMMIT/)

  await writeFile(f.statePath, '{"schema":1,"phase":"prepared"}\n')
  const corrupt = await manager(f)
  assert.equal(corrupt.mapManifest(), null)
  assert.throws(() => corrupt.assertPopupAvailable(), /transaction_id/)
  await assert.rejects(corrupt.prepare(prepareBody(f)), /persistent state is invalid/)
})

test('PREPARE rejects stale, malformed and unknown candidate identities before state write', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const active = await manager(f)
  const base = prepareBody(f)
  await assert.rejects(active.prepare({ ...base, previous_manifest_sha256: '4'.repeat(64) }), /differs/)
  await assert.rejects(active.prepare({ ...base, extra: true }), /unexpected/)
  const missingDigest = JSON.stringify({ ...JSON.parse(f.newText), line_model_role_sha256: undefined })
  await assert.rejects(active.prepare({ ...base, next_manifest_text: missingDigest }), /line_model_role/)
  await assert.rejects(
    active.prepare({ ...base, next_manifest_text: 'x'.repeat(5 * 1024 * 1024 + 1) }),
    /exceeds .* limit/,
  )
  await assert.rejects(active.prepare({ ...base, transaction_id: 'B'.repeat(64) }), /lowercase/)
  await assert.rejects(readFile(f.statePath, 'utf8'), /ENOENT/)
})

test('IPC routes require both loopback and the private token', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const active = await manager(f)
  const app = Fastify()
  await registerPopupPublishIpc(app, active)
  t.after(async () => app.close())

  const remote = await app.inject({
    method: 'POST',
    url: '/api/internal/popup-publish/prepare',
    remoteAddress: '203.0.113.8',
    headers: { 'x-qm-popup-publish-token': TOKEN },
    payload: prepareBody(f),
  })
  assert.equal(remote.statusCode, 404)

  const unauthenticated = await app.inject({
    method: 'POST',
    url: '/api/internal/popup-publish/prepare',
    remoteAddress: '127.0.0.1',
    headers: { 'x-qm-popup-publish-token': 'c'.repeat(64) },
    payload: prepareBody(f),
  })
  assert.equal(unauthenticated.statusCode, 403)

  const accepted = await app.inject({
    method: 'POST',
    url: '/api/internal/popup-publish/prepare',
    remoteAddress: '127.0.0.1',
    headers: { 'x-qm-popup-publish-token': TOKEN },
    payload: prepareBody(f),
  })
  assert.equal(accepted.statusCode, 200)
  assert.equal(accepted.json().phase, 'prepared')
})

test('map and popup routes share the same in-memory PREPARED snapshot', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const active = await manager(f)
  await active.prepare(prepareBody(f))
  await writeFile(f.currentPath, f.newText)

  const app = Fastify()
  await tilesManifestRoutes(app, { publishedLineModel: active })
  await noiseOnflyV2Routes(app, active)
  t.after(async () => app.close())

  const map = await app.inject('/api/tiles-manifest')
  assert.equal(map.statusCode, 200)
  assert.equal(map.json().build, 'b1', 'pointer flip stays invisible until COMMIT')
  const popup = await app.inject('/api/noise-onfly-v2?lat=50&lng=14')
  assert.equal(popup.statusCode, 503)
  assert.equal(popup.json().error, 'published line model unavailable')
})

test('release identity and authentication files are strict immutable inputs', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  await chmod(f.tokenPath, 0o644)
  await assert.rejects(manager(f), /private .*single-link/)
  await chmod(f.tokenPath, 0o600)
  const active = await manager(f)
  await active.prepare(prepareBody(f))
  await chmod(f.statePath, 0o644)
  const exposedState = await manager(f)
  assert.throws(() => exposedState.assertReady(), /private owner-only single-link/)
  await chmod(f.statePath, 0o600)
  await writeFile(f.sourceReaderPath, 'different-addon')
  await assert.rejects(manager(f), /addon differs/)
})

test('a committed role mismatch closes both map and popup until recovery', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const active = await manager(f)
  await active.prepare(prepareBody(f))
  await active.commit({ schema: 1, transaction_id: TRANSACTION, result: 'previous' })
  assert.equal(active.mapManifest()?.build, 'b1')

  await writeReleaseIdentity(f, NEW_DIGEST)
  const mismatched = await manager(f)
  assert.equal(mismatched.mapManifest(), null)
  assert.throws(() => mismatched.assertReady(), /release identity differs/)
  await assert.rejects(mismatched.prepare(prepareBody(f)), /persistent state is invalid/)
})

test('a durable write narrows a pre-existing state directory to owner-only', async (t) => {
  const f = await fixture()
  t.after(async () => rm(f.root, { recursive: true, force: true }))
  const stateDirectory = join(f.root, 'state')
  await mkdir(stateDirectory, { mode: 0o755 })
  await chmod(stateDirectory, 0o755)
  const active = await manager(f)
  await active.prepare(prepareBody(f))
  assert.equal((await stat(stateDirectory)).mode & 0o777, 0o700)
})

test('the prep mechanism stays dormant until a role artifact installs its identity', async () => {
  const previous = process.env.QM_POPUP_PUBLISH_IPC_REQUIRED
  try {
    delete process.env.QM_POPUP_PUBLISH_IPC_REQUIRED
    const dormant = await createRuntimePublishedLineModel()
    assert.equal(dormant.enabled, false)
    assert.doesNotThrow(() => dormant.assertReady())

    process.env.QM_POPUP_PUBLISH_IPC_REQUIRED = '1'
    await assert.rejects(
      createRuntimePublishedLineModel(),
      /required but immutable popup identity is absent/,
    )
  } finally {
    if (previous === undefined) delete process.env.QM_POPUP_PUBLISH_IPC_REQUIRED
    else process.env.QM_POPUP_PUBLISH_IPC_REQUIRED = previous
  }
})
