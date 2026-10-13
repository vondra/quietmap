// Compatible manifest generations, fetch coordinates and retention of retired decoder endpoints.

import assert from 'node:assert/strict'
import test, { after } from 'node:test'
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import Fastify from 'fastify'
import { heatmapManifestRoutes } from './heatmap-manifest.ts'
import { ALLOWED_LAYERS, MANIFEST_FILENAME, WORLD_BASE_ZOOM } from './heatmap-shared.ts'
import { HM3_MANIFEST_URL, HM3_VERSION } from '../../../frontend/src/lib/hm3-decoder.ts'

const dir = mkdtempSync(join(tmpdir(), 'tiles-manifest-route-test-'))
after(() => rmSync(dir, { recursive: true, force: true }))

async function buildApp() {
  const app = Fastify()
  await app.register(heatmapManifestRoutes, { tilesDir: dir })
  return app
}

const pinPath = join(dir, MANIFEST_FILENAME)
const clearFixture = () => {
  for (const name of readdirSync(dir)) rmSync(join(dir, name), { force: true, recursive: true })
}

function validManifest(build = 'b546'): {
  build: string
  hm3_version: number
  zoom?: number
  layers: Record<string, { file: string; build: string }>
} {
  const layers: Record<string, { file: string; build: string }> = {}
  for (const layer of ALLOWED_LAYERS) layers[layer] = { file: `${layer}.${build}.pmtiles`, build }
  return { build, hm3_version: HM3_VERSION, layers }
}

test('serves the manifest projected to the fetch coordinates', async () => {
  clearFixture()
  writeFileSync(pinPath, JSON.stringify(validManifest('b546')))
  const app = await buildApp()
  const res = await app.inject(HM3_MANIFEST_URL)
  assert.equal(res.statusCode, 200)
  assert.equal(res.json().build, 'b546')
  assert.equal(res.json().zoom, WORLD_BASE_ZOOM)
  assert.equal(Object.keys(res.json().layers).length, ALLOWED_LAYERS.size)
  assert.equal(res.json().layers.total.file, 'total.b546.pmtiles')
  assert.deepEqual(Object.keys(res.json()).sort(), ['build', 'layers', 'zoom'])
  assert.equal(res.headers['cache-control'], 'no-cache')
  await app.close()
})

test('projects a mixed per-layer publication, zoom rides the manifest', async () => {
  clearFixture()
  const manifest = validManifest('b546')
  // road was carried forward from b545 with its own older build — an
  // ordinary layer entry to the frontend.
  manifest.layers.road = { file: 'road.b545.pmtiles', build: 'b545' }
  manifest.zoom = 13
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const res = await app.inject(HM3_MANIFEST_URL)
  assert.equal(res.statusCode, 200)
  assert.equal(res.json().build, 'b546')
  assert.equal(res.json().zoom, 13)
  assert.deepEqual(res.json().layers.road, { file: 'road.b545.pmtiles', build: 'b545' })
  await app.close()
})

test('a directory without a manifest is a 404, not a 500', async () => {
  clearFixture()
  const app = await buildApp()
  const res = await app.inject(HM3_MANIFEST_URL)
  assert.equal(res.statusCode, 404)
  assert.deepEqual(res.json(), { error: 'no build published' })
  await app.close()
})

test('a torn/unparseable manifest is a 500', async () => {
  clearFixture()
  writeFileSync(pinPath, '{ not json')
  const app = await buildApp()
  const res = await app.inject(HM3_MANIFEST_URL)
  assert.equal(res.statusCode, 500)
  assert.deepEqual(res.json(), { error: 'manifest unreadable' })
  await app.close()
})

test('a manifest missing a layer is a 500, never served', async () => {
  clearFixture()
  const manifest = validManifest('b546')
  delete manifest.layers.road
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const res = await app.inject(HM3_MANIFEST_URL)
  assert.equal(res.statusCode, 500)
  assert.deepEqual(res.json(), { error: 'manifest unreadable' })
  await app.close()
})

test('a layer pointing at another layer archive is a 500', async () => {
  clearFixture()
  const manifest = validManifest('b546')
  manifest.layers.road = { file: 'rail.b546.pmtiles', build: 'b546' }
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const res = await app.inject(HM3_MANIFEST_URL)
  assert.equal(res.statusCode, 500)
  await app.close()
})

test('a retired decoder endpoint stays absent and the current one refuses incompatible archives', async () => {
  clearFixture()
  writeFileSync(pinPath, JSON.stringify(validManifest()))
  const app = await buildApp()
  try {
    assert.equal((await app.inject('/api/tiles-manifest')).statusCode, 404)
    const previous = validManifest()
    previous.hm3_version = HM3_VERSION - 1
    writeFileSync(pinPath, JSON.stringify(previous))
    const response = await app.inject(HM3_MANIFEST_URL)
    assert.equal(response.statusCode, 500)
    assert.deepEqual(response.json(), { error: 'manifest unreadable' })
  } finally {
    await app.close()
  }
})
