import assert from 'node:assert/strict'
import { lstat, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { prepareSourceReaderAddon } from './source-reader-addon.js'

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
