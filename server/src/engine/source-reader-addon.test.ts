import assert from 'node:assert/strict'
import { lstat, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { pinSourceReaderAddonOnMainThread, prepareSourceReaderAddon } from './source-reader-addon.js'

test('source-reader addon is copied atomically to one stable non-symlink path', async (t) => {
  const root = await mkdtemp(join(tmpdir(), '0db-addon-'))
  t.after(async () => rm(root, { recursive: true, force: true }))
  const source = join(root, 'libsource_reader.so')
  const shared = join(root, 'libsource_reader.worker-shared.node')
  const obsolete = join(root, 'libsource_reader.worker-slot-7.node')
  const outside = join(root, 'outside.node')
  await writeFile(source, 'native-v1')
  await writeFile(obsolete, 'obsolete')
  await writeFile(outside, 'outside')
  await symlink(outside, shared)

  assert.equal(prepareSourceReaderAddon(source), shared)
  assert.equal((await lstat(shared)).isSymbolicLink(), false)
  assert.equal(await readFile(shared, 'utf8'), 'native-v1')
  await assert.rejects(lstat(obsolete), { code: 'ENOENT' })

  await new Promise((resolveWait) => setTimeout(resolveWait, 5))
  await writeFile(source, 'native-v2-expanded')
  assert.equal(prepareSourceReaderAddon(source), shared)
  assert.equal(await readFile(shared, 'utf8'), 'native-v2-expanded')
})

test('the addon is required once in the main thread and stays pinned for the process', async (t) => {
  const root = await mkdtemp(join(tmpdir(), '0db-addon-pin-'))
  t.after(async () => rm(root, { recursive: true, force: true }))
  // A CommonJS stand-in for the .node library: loading it is observable.
  const first = join(root, 'first.cjs')
  const second = join(root, 'second.cjs')
  await writeFile(first, "globalThis.__addonPinLoads = (globalThis.__addonPinLoads ?? 0) + 1")
  await writeFile(second, "globalThis.__addonPinLoads = (globalThis.__addonPinLoads ?? 0) + 100")

  pinSourceReaderAddonOnMainThread(first)
  pinSourceReaderAddonOnMainThread(first)
  pinSourceReaderAddonOnMainThread(second)
  assert.equal((globalThis as { __addonPinLoads?: number }).__addonPinLoads, 1)
})
