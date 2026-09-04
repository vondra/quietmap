import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdtemp, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import Fastify from 'fastify'
import { docsRoutes, resolveAboutRegularFile } from './docs.js'

// The committed docs tree (repo docs/about) is another surface's content —
// this checkout may not carry it yet. The route contract below still asserts
// the full read path whenever the tree is present.
const committedDoc = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'docs', 'about', 'europe', 'cz.md')

test('About API serves committed public documents', async (t) => {
  if (!existsSync(committedDoc)) {
    t.skip('committed docs/about tree not present in this checkout')
    return
  }
  const app = Fastify()
  await app.register(docsRoutes)
  t.after(async () => app.close())

  const response = await app.inject('/api/docs/about/europe/cz')
  assert.equal(response.statusCode, 200)
  assert.equal(typeof response.json().body, 'string')
})

for (const traversal of [
  '/api/docs/about/%2e%2e%2fdev%2fchecks',
  '/api/docs/about/..%2fdev%2fchecks',
  '/api/docs/about/%2e%2e%5cdev%5cchecks',
]) {
  test(`About API rejects traversal ${traversal}`, async (t) => {
    const app = Fastify()
    await app.register(docsRoutes)
    t.after(async () => app.close())

    const response = await app.inject(traversal)
    assert.equal(response.statusCode, 404)
    assert.doesNotMatch(response.body, /Runtime checks — when and how to use them/)
  })
}

test('About asset API cannot escape docs/about', async (t) => {
  const app = Fastify()
  await app.register(docsRoutes)
  t.after(async () => app.close())

  const response = await app.inject('/api/docs/assets/%2e%2e%2fdev%2fsystem-audit.md%2ejpg')
  assert.equal(response.statusCode, 404)
})

test('About file boundary rejects symlinks to files outside its root', async (t) => {
  const root = await mkdtemp(join(tmpdir(), '0db-about-root-'))
  const outside = await mkdtemp(join(tmpdir(), '0db-about-secret-'))
  t.after(async () => {
    await Promise.all([
      rm(root, { recursive: true, force: true }),
      rm(outside, { recursive: true, force: true }),
    ])
  })
  const secret = join(outside, 'secret.jpg')
  const link = join(root, 'leak.jpg')
  await writeFile(secret, 'not public')
  await symlink(secret, link)

  assert.equal(resolveAboutRegularFile(link, root), null)
})
