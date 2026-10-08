// The data layers' route against a fake `qm-raster`: the arguments it is given, the tiles it
// refuses, a failed drawing answered without its details, and a tile whose visitor leaves never
// drawn.
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { request as httpRequest } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { buildApp } from '../app.ts'
import { parseRasterTile, RASTER_CONCURRENCY } from './raster.ts'

const FAKE_RASTER = join(import.meta.dirname, '..', 'test-fixtures', 'fake-qm-raster.ts')

async function app(t: test.TestContext) {
  const built = await buildApp({
    popupBin: '/nonexistent/qm-popup',
    rasterBin: FAKE_RASTER,
    preparedDir: '/prepared-release',
    years: ['2026', '2025'],
    popupConcurrency: 1,
    tilesDir: '/nonexistent/tiles',
    noIndex: false,
    photonUrl: 'http://127.0.0.1:9',
  })
  t.after(() => built.close())
  return built
}

test('a tile is drawn from the default year and kept an hour', async (t) => {
  const response = await (await app(t)).inject('/api/raster/elevation/12/2212/1387.png')
  assert.equal(response.statusCode, 200)
  assert.equal(response.headers['content-type'], 'image/png')
  assert.equal(response.headers['cache-control'], 'public, max-age=3600')
  assert.deepEqual([...response.rawPayload.subarray(0, 4)], [0x89, 0x50, 0x4e, 0x47])
  assert.deepEqual(JSON.parse(response.rawPayload.subarray(4).toString()), [
    '--prepared', '/prepared-release', '--year', '2026', '--layer', 'elevation',
    '--z', '12', '--x', '2212', '--y', '1387',
  ])
})

test('layers, zooms and positions without a tile are refused before drawing', async (t) => {
  const server = await app(t)
  for (const path of [
    'roads/14/0/0', 'constructor/14/0/0', 'buildings/12/0/0', 'elevation/9/0/0', 'elevation/17/0/0',
    'hard/10/1024/0', 'forest/10/0/1024', 'forest/10/-1/0', 'forest/1e1/0/0', 'forest/10/0.5/0',
  ]) {
    const response = await server.inject(`/api/raster/${path}.png`)
    assert.equal(response.statusCode, 400, path)
  }
  assert.deepEqual(parseRasterTile({ layer: 'barriers', z: '16', x: '65535', y: '0' }), [16, 65535, 0])
})

test('a failed drawing answers 500 in words for the visitor, uncached', async (t) => {
  const response = await (await app(t)).inject('/api/raster/forest/12/2212/1387.png')
  assert.equal(response.statusCode, 500)
  assert.deepEqual(response.json(), { error: 'The map layer could not be drawn.' })
  assert.equal(response.headers['cache-control'], undefined)
})

test('a tile whose visitor leaves is not drawn: dropped from the queue, or its drawing killed', async (t) => {
  const scratch = await mkdtemp(join(tmpdir(), 'raster-route-test-'))
  t.after(() => rm(scratch, { recursive: true, force: true }))
  process.env.FAKE_RASTER_SPAWN_LOG = join(scratch, 'spawned')
  t.after(() => { delete process.env.FAKE_RASTER_SPAWN_LOG })
  const server = await app(t)
  const address = await server.listen({ port: 0, host: '127.0.0.1' })
  const get = (path: string, signal?: AbortSignal) => new Promise<number>((resolve, reject) => {
    const request = httpRequest(`${address}/api/raster/${path}.png`, { agent: false, signal }, (response) => {
      response.resume()
      response.on('end', () => resolve(response.statusCode ?? 0))
    })
    request.on('error', reject)
    request.end()
  })
  // Every slot draws a slow tile; the next one waits and its visitor leaves.
  const slow = Array.from({ length: RASTER_CONCURRENCY }, (_, x) => get(`barriers/13/${x}/0`))
  const leaving = new AbortController()
  const waiting = get('elevation/13/100/0', leaving.signal).catch(() => 'left')
  await new Promise(resolve => setTimeout(resolve, 100))
  leaving.abort()
  assert.deepEqual(await Promise.all(slow), Array(RASTER_CONCURRENCY).fill(200))
  assert.equal(await waiting, 'left')
  assert.equal(await get('elevation/13/101/0'), 200)
  // A drawing whose visitor leaves is killed.
  const leavingDrawn = new AbortController()
  const drawing = get('barriers/13/200/0', leavingDrawn.signal).catch(() => 'left')
  await new Promise(resolve => setTimeout(resolve, 100))
  leavingDrawn.abort()
  assert.equal(await drawing, 'left')
  await new Promise(resolve => setTimeout(resolve, 600))
  const spawned = (await readFile(join(scratch, 'spawned'), 'utf8')).trim().split('\n')
  assert.equal(spawned.filter(line => !line.endsWith('drawn')).length, RASTER_CONCURRENCY + 2)
  assert.ok(!spawned.includes('100') && spawned.includes('101'))
  assert.ok(spawned.includes('200') && !spawned.includes('200 drawn'))
})
