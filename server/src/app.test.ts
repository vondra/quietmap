import assert from 'node:assert/strict'
import test from 'node:test'
import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { buildApp, clusterRoutesEnabled } from './app.js'

const ready = async () => ({ ready: true as const, failed: [], errors: {} })

// Tests run from src (.ts) against a tree that may (private) or may not (public
// distribution) ship the ops routes; compiled trees carry only .js.
const opsRouteFileShipped = (name: string): boolean =>
  ['ts', 'js'].some((ext) => existsSync(fileURLToPath(new URL(`./routes/${name}.${ext}`, import.meta.url))))

test('cluster dashboard: absent unless enabled, else under the /a/ admin prefix (Caddy basic_auth gates the edge)', async (t) => {
  const publicApp = await buildApp({ readinessCheck: ready, enableClusterRoutes: false })
  t.after(async () => publicApp.close())
  assert.equal(publicApp.hasRoute({ method: 'GET', url: '/a/cluster' }), false)
  assert.equal(publicApp.hasRoute({ method: 'GET', url: '/a/api/cluster/status' }), false)

  const withCluster = await buildApp({ readinessCheck: ready, enableClusterRoutes: true })
  t.after(async () => withCluster.close())
  // Presence follows the distribution: private ships the ops routes, public does not.
  const clusterShipped = opsRouteFileShipped('cluster')
  assert.equal(withCluster.hasRoute({ method: 'GET', url: '/a/cluster' }), clusterShipped)
  assert.equal(withCluster.hasRoute({ method: 'GET', url: '/a/api/cluster/worker-log' }), clusterShipped)
  if (!clusterShipped) return

  // Two-layer access control: Caddy basic_auth at the edge + requireLocalPeer here.
  // A request whose SOCKET peer is loopback (Caddy proxies from localhost; the shell TUI)
  // reaches the route regardless of the forwarded public IP. cachedStatus() warms in the
  // background, so a fresh server answers 200 (warm) or 503 (warming) — both prove REACHED
  // (asserting 200 alone is a warming race).
  const reached = await withCluster.inject({
    method: 'GET',
    url: '/a/api/cluster/status',
    remoteAddress: '127.0.0.1',
    headers: { 'x-forwarded-for': '203.0.113.20' },
  })
  assert.ok(
    reached.statusCode === 200 || reached.statusCode === 503,
    `dashboard route must respond (200 warm / 503 warming), got ${reached.statusCode}`,
  )

  // A DIRECT hit on the public port (non-loopback socket, bypassing Caddy + basic_auth)
  // is 404'd by requireLocalPeer — the raw port can't leak box IPs / costs.
  const direct = await withCluster.inject({ method: 'GET', url: '/a/api/cluster/status', remoteAddress: '203.0.113.20' })
  assert.equal(direct.statusCode, 404)
})

test('cluster dashboard defaults on for a named dev checkout and explicit configuration wins', () => {
  assert.equal(clusterRoutesEnabled({ TILE_ENV: 'dev2' }), true)
  assert.equal(clusterRoutesEnabled({ TILE_ENV: 'prod' }), false)
  assert.equal(clusterRoutesEnabled({ TILE_ENV: 'dev3', ENABLE_CLUSTER_ROUTES: '0' }), false)
  assert.equal(clusterRoutesEnabled({ TILE_ENV: 'prod', ENABLE_CLUSTER_ROUTES: '1' }), true)
})

test('ops route modules absent: startup survives and their routes stay unregistered (public distribution shape)', async (t) => {
  const app = await buildApp({
    readinessCheck: ready,
    enableClusterRoutes: true, // gate ON, files "absent" — must skip, not crash
    importOpsModule: async () => null,
  })
  t.after(async () => app.close())
  const live = await app.inject('/api/live')
  assert.equal(live.statusCode, 200)
  assert.equal(app.hasRoute({ method: 'POST', url: '/api/mail-inbound' }), false)
  assert.equal(app.hasRoute({ method: 'GET', url: '/a/cluster' }), false)
  assert.equal(app.hasRoute({ method: 'GET', url: '/a/api/cluster/status' }), false)
  // An /a/* miss answers like any unknown route, matching the gate-off shape.
  const response = await app.inject({ method: 'GET', url: '/a/cluster', remoteAddress: '127.0.0.1' })
  assert.equal(response.statusCode, 404)
})

test('mail-inbound registration follows its file presence exactly — no env gate', async (t) => {
  const app = await buildApp({ readinessCheck: ready, enableClusterRoutes: false })
  t.after(async () => app.close())
  assert.equal(app.hasRoute({ method: 'POST', url: '/api/mail-inbound' }), opsRouteFileShipped('mail-inbound'))
})

test('noindex is an explicit deployment property', async (t) => {
  const app = await buildApp({ readinessCheck: ready, noIndex: true })
  t.after(async () => app.close())
  const response = await app.inject('/api/live')
  assert.equal(response.statusCode, 200)
  assert.equal(response.headers['x-robots-tag'], 'noindex, nofollow, noarchive')
})

test('responses expose one process-coherence token for long model runs', async (t) => {
  const app = await buildApp({ readinessCheck: ready })
  t.after(async () => app.close())
  const live = await app.inject('/api/live')
  const missing = await app.inject('/does-not-exist')
  assert.match(String(live.headers['x-0db-instance']), /^[0-9a-f-]{36}$/)
  assert.equal(missing.headers['x-0db-instance'], live.headers['x-0db-instance'])
})
