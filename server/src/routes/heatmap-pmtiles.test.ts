// Contract test for the versioned tile route's MISS path — load-bearing for
// CDN caching: a miss must be 200 + EMPTY body (Cloudflare doesn't cache 204s
// by default), a year of immutable cache, CORS + Timing-Allow-Origin, and no
// Content-Encoding (an empty stream is not valid Brotli).
// Run: cd server && npx tsx --test src/routes/heatmap-pmtiles.test.ts

import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtempSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'

/** Minimal valid pmtiles v3 archive with ZERO tile entries — every getZxy
 *  misses. Layout: 127-byte header, gzip root directory, gzip '{}' metadata.
 *  Field offsets mirror the pmtiles JS lib's bytesToHeader. */
function emptyArchive(): Buffer {
  const rootDir = gzipSync(Buffer.from([0])) // varint 0 = no entries
  const metadata = gzipSync(Buffer.from('{}'))
  const header = Buffer.alloc(127)
  header.write('PMTiles', 0, 'ascii')
  header.writeUInt8(3, 7) // spec version
  header.writeBigUInt64LE(127n, 8) // root dir offset
  header.writeBigUInt64LE(BigInt(rootDir.length), 16)
  header.writeBigUInt64LE(BigInt(127 + rootDir.length), 24) // metadata offset
  header.writeBigUInt64LE(BigInt(metadata.length), 32)
  const end = BigInt(127 + rootDir.length + metadata.length)
  header.writeBigUInt64LE(end, 40) // leaf dirs offset (empty)
  header.writeBigUInt64LE(end, 56) // tile data offset (empty)
  header.writeUInt8(2, 97) // internal compression: gzip (route requires)
  header.writeUInt8(3, 98) // tile compression: brotli (route requires)
  header.writeUInt8(12, 101) // max zoom (min zoom byte 100 stays 0)
  return Buffer.concat([header, rootDir, metadata])
}

// PMTILES_BASE is captured from the env when heatmap-shared loads — point it
// at the fixture dir BEFORE importing the route module.
const dir = mkdtempSync(join(tmpdir(), 'pmtiles-route-test-'))
process.env.PMTILES_DIR = dir
writeFileSync(join(dir, 'road.b0.pmtiles'), emptyArchive())

const { heatmapPmtilesRoutes } = await import('./heatmap-pmtiles.js')
const { default: Fastify } = await import('fastify')

async function buildApp() {
  const app = Fastify()
  await app.register(heatmapPmtilesRoutes)
  return app
}

test('miss is a cacheable empty 200, not a 204', async () => {
  const app = await buildApp()
  const res = await app.inject({ url: '/api/tiles/b0/road/6/33/21.bin' })
  assert.equal(res.statusCode, 200)
  assert.equal(res.rawPayload.length, 0)
  assert.equal(res.headers['cache-control'], 'public, max-age=31536000, immutable')
  assert.equal(res.headers['access-control-allow-origin'], '*')
  assert.equal(res.headers['timing-allow-origin'], '*')
  assert.equal(res.headers['content-encoding'], undefined)
  await app.close()
})

test('unpacked layer archive is 404, unknown build id is 404', async () => {
  const app = await buildApp()
  const noArchive = await app.inject({ url: '/api/tiles/b0/rail/6/33/21.bin' })
  assert.equal(noArchive.statusCode, 404)
  const badBuild = await app.inject({ url: '/api/tiles/evil/road/6/33/21.bin' })
  assert.equal(badBuild.statusCode, 404)
  await app.close()
})

test('symlinked archive leaf fails closed before PMTiles open', async () => {
  const link = join(dir, 'road.b1.pmtiles')
  symlinkSync(join(dir, 'road.b0.pmtiles'), link)
  const app = await buildApp()
  const res = await app.inject({ url: '/api/tiles/b1/road/6/33/21.bin' })
  assert.equal(res.statusCode, 500)
  assert.equal(res.body, 'archive open failed')
  assert.equal(res.headers['cache-control'], 'no-store')
  await app.close()
})
