/**
 * chain/run-directory.ts — concurrent starts get separate readable run IDs.
 *
 * Run: `cd pipeline && npx tsx --test chain/run-directory.test.ts`
 */

import { after, test } from 'node:test'
import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createChainRunDirectory } from './run-directory.js'

const TMP = mkdtempSync(join(tmpdir(), 'chain-run-directory-'))
after(() => rmSync(TMP, { recursive: true, force: true }))

test('same-second starts atomically allocate distinct readable directories', () => {
  const startedAt = new Date('2026-08-11T05:21:34.987Z')
  const first = createChainRunDirectory(TMP, startedAt)
  const second = createChainRunDirectory(TMP, startedAt)

  assert.notEqual(first.runId, second.runId)
  assert.match(first.runId, /^2026-08-11-05-21-34-.{6}$/)
  assert.match(second.runId, /^2026-08-11-05-21-34-.{6}$/)
  assert.equal(first.logDir, join(TMP, 'logs', 'chain', first.runId))
  assert.equal(second.logDir, join(TMP, 'logs', 'chain', second.runId))
  assert.ok(existsSync(first.logDir))
  assert.ok(existsSync(second.logDir))
  const modeControl = join(TMP, 'logs', 'chain', 'mode-control')
  mkdirSync(modeControl)
  assert.equal(statSync(first.logDir).mode & 0o7777, statSync(modeControl).mode & 0o7777)
})
