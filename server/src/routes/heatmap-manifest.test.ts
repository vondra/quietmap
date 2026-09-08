// Contract test for GET /api/tiles-manifest: serves the single-format pin
// (current.dev1.json) projected to {build, zoom, layers, tile_base}.
// Run: cd server && node --import tsx --test src/routes/heatmap-manifest.test.ts

import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { WORLD_BASE_ZOOM } from '../generation-contract.mjs'

// PMTILES_BASE / MANIFEST_FILENAME are captured from the env when
// heatmap-shared loads — point them at the fixture dir BEFORE importing.
const dir = mkdtempSync(join(tmpdir(), 'tiles-manifest-route-test-'))
process.env.TILES_PMTILES_DIR = dir
process.env.TILES_MANIFEST_FILE = 'current.dev1.json'

const { heatmapManifestRoutes } = await import('./heatmap-manifest.js')
const { ALLOWED_LAYERS } = await import('./heatmap-shared.js')
const { default: Fastify } = await import('fastify')

async function buildApp() {
  const app = Fastify()
  await app.register(heatmapManifestRoutes)
  return app
}

const pinPath = join(dir, 'current.dev1.json')
const clearFixture = () => {
  for (const name of readdirSync(dir)) rmSync(join(dir, name), { force: true, recursive: true })
}

function validManifest(build = 'b546'): {
  build: string
  zoom?: number
  layers: Record<string, { file: string; build: string }>
} {
  const layers: Record<string, { file: string; build: string }> = {}
  for (const layer of ALLOWED_LAYERS) layers[layer] = { file: `${layer}.${build}.pmtiles`, build }
  return { build, layers }
}

test('serves the pin with tile_base attached', async () => {
  clearFixture()
  writeFileSync(pinPath, JSON.stringify(validManifest('b546')))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 200)
  assert.equal(res.json().build, 'b546')
  assert.equal(res.json().zoom, WORLD_BASE_ZOOM)
  assert.equal(Object.keys(res.json().layers).length, 8)
  assert.equal(res.json().layers.total.file, 'total.b546.pmtiles')
  assert.equal(res.json().tile_base, '')
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
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 200)
  assert.equal(res.json().build, 'b546')
  assert.equal(res.json().zoom, 13)
  assert.deepEqual(res.json().layers.road, { file: 'road.b545.pmtiles', build: 'b545' })
  await app.close()
})

test('a genuinely fresh checkout (no pin) is a 404, not a 500', async () => {
  clearFixture()
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 404)
  assert.deepEqual(res.json(), { error: 'no build published' })
  await app.close()
})

test('a torn/unparseable pin is a 500', async () => {
  clearFixture()
  writeFileSync(pinPath, '{ not json')
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 500)
  assert.deepEqual(res.json(), { error: 'manifest unreadable' })
  await app.close()
})

test('a pin missing a layer is a 500, never served', async () => {
  clearFixture()
  const manifest = validManifest('b546')
  delete manifest.layers.road
  writeFileSync(pinPath, JSON.stringify(manifest))
  const app = await buildApp()
  const res = await app.inject('/api/tiles-manifest')
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
  const res = await app.inject('/api/tiles-manifest')
  assert.equal(res.statusCode, 500)
  await app.close()
})
