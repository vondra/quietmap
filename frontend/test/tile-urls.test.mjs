import assert from 'node:assert/strict'
import test from 'node:test'

import { buildKey, tileUrl } from '../src/lib/tile-urls.ts'

const builds = { latest: 'b16', byLayer: { road: 'b15', total: 'b16' }, base: '', zoom: 12 }

test('a tile URL carries the SOURCE\'s own build, not the manifest build', () => {
  assert.equal(tileUrl(builds, 'road', 12, 2212, 1387), '/api/tiles/b15/road/12/2212/1387.bin')
  assert.equal(tileUrl(builds, 'total', 12, 2212, 1387), '/api/tiles/b16/total/12/2212/1387.bin')
  assert.equal(tileUrl({ ...builds, base: 'https://tiles.example' }, 'road', 2, 1, 1),
    'https://tiles.example/api/tiles/b15/road/2/1/1.bin')
})

test('the cache key re-keys on a partial republish AND on a world-zoom change', () => {
  // A deck layer id / composite signature is built from this. If the world is repainted at
  // a finer base zoom, every cached tile of the old world must be dropped, not reused.
  assert.equal(buildKey(builds, ['road', 'total']), 'road:b15|total:b16|z12')
  assert.notEqual(buildKey({ ...builds, zoom: 13 }, ['road']), buildKey(builds, ['road']))
  assert.notEqual(buildKey({ ...builds, byLayer: { road: 'b17' } }, ['road']),
    buildKey(builds, ['road']))
})
