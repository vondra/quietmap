/** Tests for the SQLite-backed process singleton lock. */
import assert from 'node:assert/strict'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { tryAcquireSqliteRunLock } from './sqlite-run-lock.js'

test('a held run lock excludes another owner and becomes available after release', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'sqlite-run-lock-'))
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  const lockPath = join(directory, 'run.lock')

  const firstOwner = tryAcquireSqliteRunLock(lockPath)
  assert.ok(firstOwner)
  assert.equal(tryAcquireSqliteRunLock(lockPath), null)

  firstOwner.release()
  const nextOwner = tryAcquireSqliteRunLock(lockPath)
  assert.ok(nextOwner)
  nextOwner.release()
})
