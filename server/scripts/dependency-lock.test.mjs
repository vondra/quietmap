import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { assertInstalledDependenciesMatchLock } from './dependency-lock.mjs'

async function fixture({ installedVersion = '1.0.0', includeDirect = true } = {}) {
  const root = await mkdtemp(join(tmpdir(), '0db-dependency-lock-'))
  await mkdir(join(root, 'node_modules'), { recursive: true })
  const resolved = { version: '1.0.0', integrity: 'sha512-current' }
  const rootLock = {
    lockfileVersion: 3,
    packages: {
      '': {
        dependencies: { direct: '^1.0.0' },
        optionalDependencies: { 'other-platform': '^2.0.0' },
      },
      'node_modules/direct': resolved,
      // npm omits an optional package that is not installable on this host.
      'node_modules/other-platform': { version: '2.0.0', os: ['darwin'] },
    },
  }
  const installedPackages = includeDirect
    ? { 'node_modules/direct': { ...resolved, version: installedVersion } }
    : {}
  if (includeDirect) await mkdir(join(root, 'node_modules/direct'))
  await writeFile(join(root, 'package-lock.json'), JSON.stringify(rootLock))
  await writeFile(join(root, 'node_modules/.package-lock.json'), JSON.stringify({
    lockfileVersion: 3,
    packages: installedPackages,
  }))
  return root
}

test('accepts the platform-specific installed subset of package-lock.json', async (t) => {
  const root = await fixture()
  t.after(() => rm(root, { recursive: true, force: true }))
  assert.doesNotThrow(() => assertInstalledDependenciesMatchLock(root))
})

test('rejects a lock entry whose installed directory was deleted', async (t) => {
  const root = await fixture()
  t.after(() => rm(root, { recursive: true, force: true }))
  await rm(join(root, 'node_modules/direct'), { recursive: true })
  assert.throws(
    () => assertInstalledDependenciesMatchLock(root),
    /installed lock entry is missing on disk: node_modules\/direct/,
  )
})

test('rejects a stale installed package with an npm ci repair hint', async (t) => {
  const root = await fixture({ installedVersion: '0.9.0' })
  t.after(() => rm(root, { recursive: true, force: true }))
  assert.throws(
    () => assertInstalledDependenciesMatchLock(root),
    /stale installed entry: node_modules\/direct.*npm ci|npm ci.*stale installed entry: node_modules\/direct/,
  )
})

test('rejects a newly declared direct dependency missing from node_modules', async (t) => {
  const root = await fixture({ includeDirect: false })
  t.after(() => rm(root, { recursive: true, force: true }))
  assert.throws(
    () => assertInstalledDependenciesMatchLock(root),
    /missing or stale direct dependency: direct.*npm ci|npm ci.*missing or stale direct dependency: direct/,
  )
})
