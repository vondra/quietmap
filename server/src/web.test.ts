// The static frontend and the 404 shapes.
import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import Fastify from 'fastify'
import { registerWeb } from './web.ts'

test('the map, its files and its About pages are served; nothing else pretends to exist', async (t) => {
  const frontend = await mkdtemp(join(tmpdir(), '0db-web-'))
  await writeFile(join(frontend, 'index.html'), '<!doctype html><title>quiet-map-test</title>')
  await writeFile(join(frontend, 'known.js'), 'console.log("known")')
  await writeFile(join(frontend, 'about-pages.json'), JSON.stringify(['', 'credits', 'europe', 'europe/cz', 'methodology', 'news']))
  t.after(async () => rm(frontend, { recursive: true, force: true }))

  const app = Fastify()
  await registerWeb(app, frontend)
  t.after(async () => app.close())

  for (const url of ['/', '/?e2e=1', '/about', '/about/', '/about/methodology', '/about/news/', '/about/europe', '/about/europe/cz']) {
    const response = await app.inject(url)
    assert.equal(response.statusCode, 200, url)
    assert.match(response.body, /quiet-map-test/)
  }
  assert.equal((await app.inject('/known.js')).statusCode, 200)

  for (const url of [
    '/.env', '/.git/config', '/.ssh/id_rsa', '/aboutness', '/about/europe/de', '/about//news', '/about/.env',
    '/%2eenv', '/assets/missing.js', '/api/missing', '/about/%6eews', '/%61bout/news', '/about%2fnews',
  ]) {
    const response = await app.inject(url)
    assert.equal(response.statusCode, 404, url)
    assert.doesNotMatch(response.body, /quiet-map-test/)
  }
  assert.equal((await app.inject({ method: 'POST', url: '/' })).statusCode, 404)
})

test('404 is friendly HTML for browsers, plain JSON for API and non-HTML clients', async (t) => {
  const frontend = await mkdtemp(join(tmpdir(), '0db-web-'))
  await writeFile(join(frontend, 'index.html'), '<!doctype html><title>quiet-map-test</title>')
  await writeFile(join(frontend, 'about-pages.json'), '[""]')
  t.after(async () => rm(frontend, { recursive: true, force: true }))

  const app = Fastify()
  await registerWeb(app, frontend)
  t.after(async () => app.close())

  const browser = await app.inject({ url: '/no-such-page', headers: { accept: 'text/html' } })
  assert.equal(browser.statusCode, 404)
  assert.match(browser.headers['content-type'] ?? '', /text\/html/)
  assert.match(browser.body, /Too quiet/) // the friendly page's headline
  assert.match(browser.body, /\/favicon\.svg/) // logo referenced, never embedded
  assert.doesNotMatch(browser.body, /quiet-map-test/)

  for (const response of [
    await app.inject({ url: '/api/missing', headers: { accept: 'text/html' } }),
    await app.inject('/no-such-page'), // curl-like: no Accept header
  ]) {
    assert.equal(response.statusCode, 404)
    assert.match(response.headers['content-type'] ?? '', /application\/json/)
    assert.deepEqual(response.json(), { error: 'Not found' })
  }
})
