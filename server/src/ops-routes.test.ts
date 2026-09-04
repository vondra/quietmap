//! Tests for importOptionalOpsModule: null only for a missing file, rethrow for anything broken.

import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { importOptionalOpsModule } from './ops-routes.js'

test('resolves to null when the module file itself is absent', async () => {
  const missing = await importOptionalOpsModule('./routes/definitely-not-shipped-xyz.js')
  assert.equal(missing, null)
})

test('imports a present module', async () => {
  const present = await importOptionalOpsModule<{ marker: string }>('./test-fixtures/ops-module-present.js')
  assert.equal(present?.marker, 'present')
})

test('rethrows when a present module fails during evaluation', async () => {
  await assert.rejects(
    importOptionalOpsModule('./test-fixtures/ops-module-throws.js'),
    /ops fixture evaluation failure/,
  )
})

test('rethrows when a present module cannot resolve its own dependency', async () => {
  // The unresolvable path is the fixture's dependency, not our specifier —
  // treating that as absence would silently hide a broken ops file.
  await assert.rejects(
    importOptionalOpsModule('./test-fixtures/ops-module-missing-dependency.js'),
    (error: unknown) => {
      assert.equal((error as { code?: unknown }).code, 'ERR_MODULE_NOT_FOUND')
      assert.match((error as Error).message, /ops-fixture-no-such-dependency/)
      return true
    },
  )
})

test('falls back to OPS_ROUTES_DIR when the in-tree file is absent (Model B private layout)', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'ops-routes-dir-'))
  t.after(async () => rm(dir, { recursive: true, force: true }))
  await writeFile(join(dir, 'external-ops.js'), "export const marker = 'external'\n")
  process.env.OPS_ROUTES_DIR = dir
  t.after(() => { delete process.env.OPS_ROUTES_DIR })
  const loaded = await importOptionalOpsModule<{ marker: string }>('./routes/external-ops.js')
  assert.equal(loaded?.marker, 'external')
  // Absent in BOTH places is still a plain null, never an error.
  const missing = await importOptionalOpsModule('./routes/definitely-not-shipped-xyz.js')
  assert.equal(missing, null)
})
