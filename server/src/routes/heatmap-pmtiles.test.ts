// Versioned tile serving: wire responses, safe archive reads, descriptor lifetime and recovery.

import assert from 'node:assert/strict'
import test, { after } from 'node:test'
import { mkdtempSync, readdirSync, readlinkSync, renameSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { brotliCompressSync, gzipSync } from 'node:zlib'
import { zxyToTileId } from 'pmtiles'

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
after(() => rmSync(dir, { recursive: true, force: true }))
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
  assert.equal(res.body, 'archive read failed')
  assert.equal(res.headers['cache-control'], 'no-store')
  await app.close()
})

test('completed concurrent tile requests do not retain a deleted archive on disk', { skip: process.platform !== 'linux' }, async (t) => {
  const path = join(dir, 'road.b2.pmtiles')
  writeFileSync(path, emptyArchive())
  const app = await buildApp()
  t.after(() => app.close())
  const responses = await Promise.all(Array.from({ length: 8 }, () =>
    app.inject({ url: '/api/tiles/b2/road/6/33/21.bin' })))
  for (const response of responses) assert.equal(response.statusCode, 200)
  unlinkSync(path)
  const held = readdirSync('/proc/self/fd').some(fd => {
    try { return readlinkSync(`/proc/self/fd/${fd}`) === `${path} (deleted)` } catch { return false }
  })
  assert.ok(!held, 'the archive cache pins deleted disk space')
})

test('a failed leaf-directory read retries after the archive becomes readable again', async (t) => {
  const tile = brotliCompressSync(Buffer.from('tile bytes'))
  const ids = [zxyToTileId(2, 0, 0), zxyToTileId(2, 0, 1)]
  // These low-zoom IDs and lengths all fit one-byte PMTiles varints. Each root
  // entry points at a separate leaf beyond the initial 16 KiB header read.
  const leaves = ids.map(id => gzipSync(Buffer.from([1, id, 1, tile.length, 1])))
  const root = gzipSync(Buffer.from([2, ids[0], ids[1] - ids[0], 0, 0,
    leaves[0].length, leaves[1].length, 1, leaves[0].length + 1]))
  const header = Buffer.from(emptyArchive().subarray(0, 127))
  header.writeBigUInt64LE(BigInt(root.length), 16)
  header.writeBigUInt64LE(BigInt(127 + root.length), 24)
  header.writeBigUInt64LE(0n, 32)
  header.writeBigUInt64LE(16384n, 40)
  header.writeBigUInt64LE(BigInt(leaves[0].length + leaves[1].length), 48)
  header.writeBigUInt64LE(BigInt(16384 + leaves[0].length + leaves[1].length), 56)
  header.writeBigUInt64LE(BigInt(tile.length), 64)
  const path = join(dir, 'road.b3.pmtiles')
  writeFileSync(path, Buffer.concat([header, root,
    Buffer.alloc(16384 - 127 - root.length), ...leaves, tile]))
  const app = await buildApp()
  t.after(() => app.close())
  const first = await app.inject({ url: '/api/tiles/b3/road/2/0/0.bin' })
  assert.equal(first.statusCode, 200)
  assert.deepEqual(first.rawPayload, tile)
  renameSync(path, `${path}.held`)
  const unavailable = await app.inject({ url: '/api/tiles/b3/road/2/0/1.bin' })
  assert.equal(unavailable.statusCode, 404)
  renameSync(`${path}.held`, path)
  const recovered = await app.inject({ url: '/api/tiles/b3/road/2/0/1.bin' })
  assert.equal(recovered.statusCode, 200)
  assert.deepEqual(recovered.rawPayload, tile)
})
