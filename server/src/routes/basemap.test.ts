// The base map's routes: a stored tile sent as it is, a missing one, file names that leave the
// directory, and a server without a base map.

import assert from 'node:assert/strict'
import test, { after } from 'node:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gunzipSync, gzipSync } from 'node:zlib'
import Fastify from 'fastify'
import { zxyToTileId } from 'pmtiles'
import { basemapRoutes } from './basemap.ts'

const TILE = gzipSync(Buffer.from('a vector tile'))

/** A PMTiles v3 archive of one gzip tile at 2/1/1: the header, a gzip root directory, the tile. */
function archiveOfOneTile(): Buffer {
  // One entry: its tile id, a run of one, the tile's length, its offset plus one (all one-byte varints).
  const root = gzipSync(Buffer.from([1, zxyToTileId(2, 1, 1), 1, TILE.length, 1]))
  const header = Buffer.alloc(127)
  header.write('PMTiles', 0, 'ascii')
  header.writeUInt8(3, 7) // spec version
  header.writeBigUInt64LE(127n, 8) // root directory offset
  header.writeBigUInt64LE(BigInt(root.length), 16)
  const end = BigInt(127 + root.length)
  header.writeBigUInt64LE(end, 24) // metadata offset (none)
  header.writeBigUInt64LE(end, 40) // leaf directories offset (none)
  header.writeBigUInt64LE(end, 56) // tile data offset
  header.writeBigUInt64LE(BigInt(TILE.length), 64)
  header.writeUInt8(2, 97) // internal compression: gzip
  header.writeUInt8(2, 98) // tile compression: gzip
  header.writeUInt8(1, 99) // tile type: MVT
  header.writeUInt8(15, 101) // max zoom (min zoom, byte 100, stays 0)
  return Buffer.concat([header, root, TILE])
}

const dir = mkdtempSync(join(tmpdir(), 'basemap-route-test-'))
after(() => rmSync(dir, { recursive: true, force: true }))
writeFileSync(join(dir, 'planet.pmtiles'), archiveOfOneTile())
mkdirSync(join(dir, 'fonts', 'Noto Sans Regular'), { recursive: true })
writeFileSync(join(dir, 'fonts', 'Noto Sans Regular', '0-255.pbf'), 'glyphs')
mkdirSync(join(dir, 'sprites'))
writeFileSync(join(dir, 'sprites', 'light@2x.json'), '{}')
writeFileSync(join(dir, 'secret.json'), '{"outside": "sprites"}')

async function app(t: test.TestContext, basemapDir: string | null = dir) {
  const built = Fastify()
  await built.register(basemapRoutes, { basemapDir })
  t.after(() => built.close())
  return built
}

test('a stored tile is sent as it is, gzip for the browser to inflate, kept a day', async (t) => {
  const response = await (await app(t)).inject('/api/basemap/2/1/1.mvt')
  assert.equal(response.statusCode, 200)
  assert.equal(response.headers['content-type'], 'application/x-protobuf')
  assert.equal(response.headers['content-encoding'], 'gzip')
  assert.equal(response.headers['cache-control'], 'public, max-age=86400')
  assert.deepEqual(response.rawPayload, TILE)
  assert.equal(gunzipSync(response.rawPayload).toString(), 'a vector tile')
})

test('a tile the archive lacks is an empty 204; an address outside the world is refused', async (t) => {
  const server = await app(t)
  for (const missing of ['2/1/2', '16/0/0']) {
    const response = await server.inject(`/api/basemap/${missing}.mvt`)
    assert.equal(response.statusCode, 204, missing)
    assert.equal(response.headers['content-encoding'], undefined)
  }
  for (const wrong of ['2/4/0', '2/0/4', '23/0/0', '2/-1/0', '2/1.5/1', '1e1/0/0', 'fonts/0/0']) {
    assert.equal((await server.inject(`/api/basemap/${wrong}.mvt`)).statusCode, 400, wrong)
  }
})

test('fonts and sprites are served by name; a name that leaves their directories is 404', async (t) => {
  const server = await app(t)
  const glyphs = await server.inject('/api/basemap/fonts/Noto%20Sans%20Regular/0-255.pbf')
  assert.equal(glyphs.statusCode, 200)
  assert.equal(glyphs.headers['content-type'], 'application/x-protobuf')
  assert.equal(glyphs.body, 'glyphs')
  const sprite = await server.inject('/api/basemap/sprites/light@2x.json')
  assert.equal(sprite.statusCode, 200)
  assert.match(String(sprite.headers['content-type']), /^application\/json/)
  assert.equal(sprite.headers['cache-control'], 'public, max-age=86400')
  for (const path of [
    'sprites/..%2Fsecret.json', 'sprites/%2E%2E%2Fsecret.json', 'sprites/../secret.json', 'sprites/secret.json',
    'sprites/dark.png', 'fonts/..%2Fsprites/light@2x.pbf', 'fonts/%2E%2E/0-255.pbf', 'fonts/Noto%20Sans%20Regular/..%2F0-255.pbf',
    'fonts/Noto%20Sans%20Regular/256-511.pbf',
  ]) {
    const response = await server.inject(`/api/basemap/${path}`)
    assert.equal(response.statusCode, 404, path)
    assert.doesNotMatch(response.body, /outside/, path)
  }
})

test('without a base map directory the routes do not exist', async (t) => {
  const server = await app(t, null)
  for (const path of ['2/1/1.mvt', 'fonts/Noto%20Sans%20Regular/0-255.pbf', 'sprites/light@2x.json']) {
    assert.equal((await server.inject(`/api/basemap/${path}`)).statusCode, 404, path)
  }
})
