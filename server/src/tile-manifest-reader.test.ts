import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ALLOWED_TILE_ENVS, resolveManifestPath, resolveTileEnv } from './tile-manifest-reader.js'

test('resolveTileEnv accepts every allowlisted value', () => {
  for (const env of ALLOWED_TILE_ENVS) {
    assert.equal(resolveTileEnv(env), env)
  }
})

test('resolveTileEnv is fail-closed on missing or unrecognized values', () => {
  assert.throws(() => resolveTileEnv(undefined), /TILE_ENV must be one of/)
  assert.throws(() => resolveTileEnv(''), /TILE_ENV must be one of/)
  assert.throws(() => resolveTileEnv('staging'), /TILE_ENV must be one of/)
  assert.throws(() => resolveTileEnv('PROD'), /TILE_ENV must be one of/, 'allowlist match is case-sensitive')
})

test('resolveTileEnv falls back to process.env.TILE_ENV when no override is given', (t) => {
  const before = process.env.TILE_ENV
  t.after(() => { process.env.TILE_ENV = before })

  process.env.TILE_ENV = 'dev3'
  assert.equal(resolveTileEnv(), 'dev3')

  delete process.env.TILE_ENV
  assert.throws(() => resolveTileEnv(), /TILE_ENV must be one of/)
})

async function tmpPmtilesDir() {
  return mkdtemp(join(tmpdir(), 'tile-manifest-reader-test-'))
}

test('all development checkouts share a pointer independent of production', async (t) => {
  const dir = await tmpPmtilesDir()
  t.after(async () => rm(dir, { recursive: true, force: true }))
  assert.equal(resolveManifestPath(dir, 'prod'), join(dir, 'current.prod.json'))
  for (const env of ['dev1', 'dev2', 'dev3']) {
    assert.equal(resolveManifestPath(dir, env), join(dir, 'current.dev1.json'))
  }
})

test('resolveManifestPath fails closed when the served pointer is missing but a packer current.json exists', async (t) => {
  const dir = await tmpPmtilesDir()
  t.after(async () => rm(dir, { recursive: true, force: true }))
  await writeFile(join(dir, 'current.json'), JSON.stringify({ build: 'b1', layers: {} }))
  assert.throws(
    () => resolveManifestPath(dir, 'dev1'),
    (err: unknown) => err instanceof Error && /packer merge head exists/.test(err.message),
  )
})

test('resolveManifestPath prefers an existing served pointer over a packer current.json', async (t) => {
  const dir = await tmpPmtilesDir()
  t.after(async () => rm(dir, { recursive: true, force: true }))
  await writeFile(join(dir, 'current.json'), 'irrelevant-legacy-content')
  await writeFile(join(dir, 'current.dev1.json'), 'irrelevant-per-env-content')
  assert.doesNotThrow(() => resolveManifestPath(dir, 'dev1'))
  assert.equal(resolveManifestPath(dir, 'dev1'), join(dir, 'current.dev1.json'))
})

test('resolveManifestPath propagates a bad TILE_ENV before ever touching the filesystem', async (t) => {
  const dir = await tmpPmtilesDir()
  t.after(async () => rm(dir, { recursive: true, force: true }))
  assert.throws(() => resolveManifestPath(dir, 'staging'), /TILE_ENV must be one of/)
})

test('development never falls back to production', async (t) => {
  const dir = await tmpPmtilesDir()
  t.after(async () => rm(dir, { recursive: true, force: true }))
  await writeFile(join(dir, 'current.prod.json'), JSON.stringify({ build: 'b5' }))
  assert.equal(resolveManifestPath(dir, 'dev1'), join(dir, 'current.dev1.json'))
})
