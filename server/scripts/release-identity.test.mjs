/** A server release names the commit it was built from and says when the checkout differed from it. */
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { productCodeIdentity, releaseIdentityFileName, writeReleaseIdentity } from './release-identity.mjs'

function git(root, ...args) {
  const result = spawnSync('git', ['-c', 'user.name=test', '-c', 'user.email=test@example.invalid', ...args],
    { cwd: root, encoding: 'utf8' })
  assert.equal(result.status, 0, result.stderr)
  return result.stdout.trim()
}

test('identity names HEAD and marks tracked edits and untracked files as dirty', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'release-identity-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  git(root, 'init', '-q')
  writeFileSync(join(root, 'server.ts'), 'one')
  git(root, 'add', 'server.ts')
  git(root, 'commit', '-q', '-m', 'one')
  const commit = git(root, 'rev-parse', 'HEAD')
  assert.deepEqual(productCodeIdentity(root), { product_commit: commit, product_dirty: false })
  writeFileSync(join(root, 'server.ts'), 'two')
  assert.deepEqual(productCodeIdentity(root), { product_commit: commit, product_dirty: true })
  git(root, 'checkout', '-q', '--', 'server.ts')
  writeFileSync(join(root, 'new.ts'), 'untracked')
  assert.equal(productCodeIdentity(root).product_dirty, true)
})

test('release.json is written once and never overwritten', (t) => {
  const stage = mkdtempSync(join(tmpdir(), 'release-stage-'))
  t.after(() => rmSync(stage, { recursive: true, force: true }))
  const identity = { product_commit: 'abc', product_dirty: false, native_sha256: null }
  writeReleaseIdentity(stage, identity)
  assert.deepEqual(JSON.parse(readFileSync(join(stage, releaseIdentityFileName), 'utf8')), identity)
  assert.throws(() => writeReleaseIdentity(stage, { ...identity, product_dirty: true }), { code: 'EEXIST' })
})
