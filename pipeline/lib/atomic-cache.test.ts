/** Regression tests for collision-free atomic cache publication. */

import { after, test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { replaceCacheFileAtomically, replaceCacheFileAtomicallyAsync } from './atomic-cache.js'

const TMP = mkdtempSync(join(tmpdir(), 'atomic-cache-test-'))
after(() => rmSync(TMP, { recursive: true, force: true }))

test('overlapping writers to one destination own separate temporary files', () => {
  const testDirectory = mkdtempSync(join(TMP, 'overlap-'))
  const destination = join(testDirectory, 'shared.json')

  replaceCacheFileAtomically(destination, outerTemporaryPath => {
    writeFileSync(outerTemporaryPath, 'outer-complete')

    // Model a second process publishing after the outer writer has finished
    // building but before it renames. A fixed `${destination}.tmp` makes the
    // inner rename steal the outer writer's file and the outer rename ENOENT.
    replaceCacheFileAtomically(destination, innerTemporaryPath => {
      assert.notEqual(innerTemporaryPath, outerTemporaryPath)
      writeFileSync(innerTemporaryPath, 'inner-complete')
    })
  })

  assert.equal(readFileSync(destination, 'utf8'), 'outer-complete')
  assert.deepEqual(readdirSync(testDirectory), ['shared.json'], 'no private temp survives publication')
})

test('failed builders leave neither a partial destination nor a temporary file', () => {
  const testDirectory = mkdtempSync(join(TMP, 'failure-'))
  const destination = join(testDirectory, 'failed.json')

  assert.throws(() => replaceCacheFileAtomically(destination, temporaryPath => {
    writeFileSync(temporaryPath, 'partial')
    throw new Error('conversion failed')
  }), /conversion failed/)

  assert.deepEqual(readdirSync(testDirectory), [])
})

test('a failed replacement preserves an existing complete destination', () => {
  const testDirectory = mkdtempSync(join(TMP, 'preserve-'))
  const destination = join(testDirectory, 'existing.json')
  writeFileSync(destination, 'previous-complete')

  assert.throws(() => replaceCacheFileAtomically(destination, temporaryPath => {
    writeFileSync(temporaryPath, 'partial-refresh')
    throw new Error('refresh failed')
  }), /refresh failed/)

  assert.equal(readFileSync(destination, 'utf8'), 'previous-complete')
  assert.deepEqual(readdirSync(testDirectory), ['existing.json'])
})

test('overlapping streamed writers publish only complete private files', async () => {
  const testDirectory = mkdtempSync(join(TMP, 'stream-overlap-'))
  const destination = join(testDirectory, 'shared.zip')

  await replaceCacheFileAtomicallyAsync(destination, async outerTemporaryPath => {
    await writeFile(outerTemporaryPath, 'outer-complete')
    await replaceCacheFileAtomicallyAsync(destination, async innerTemporaryPath => {
      assert.notEqual(innerTemporaryPath, outerTemporaryPath)
      await writeFile(innerTemporaryPath, 'inner-complete')
    })
  })

  assert.equal(readFileSync(destination, 'utf8'), 'outer-complete')
  assert.deepEqual(readdirSync(testDirectory), ['shared.zip'], 'no private stream temp survives publication')
})

test('a rejected streamed builder removes its private temporary file', async () => {
  const testDirectory = mkdtempSync(join(TMP, 'stream-failure-'))
  const destination = join(testDirectory, 'failed.zip')

  await assert.rejects(replaceCacheFileAtomicallyAsync(destination, async temporaryPath => {
    await writeFile(temporaryPath, 'partial-stream')
    throw new Error('stream failed')
  }), /stream failed/)

  assert.deepEqual(readdirSync(testDirectory), [])
})
