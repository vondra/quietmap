/**
 * Per-client rate limiting for public request-heavy endpoints (owner
 * directive 2026-07-15). Tiles (/api/tiles*) and static assets must NEVER be
 * limited — the map fetches dozens of cheap immutable tiles per pan, so the
 * plugin registers with `global: false` and only routes that opt in via a
 * route-level `config.rateLimit` are throttled.
 */

/** 5 req/s per client bucket — route-level opt-in config for @fastify/rate-limit. */
export const EXPENSIVE_ROUTE_RATE_LIMIT = {
  max: 5,
  timeWindow: 1000,
}

/**
 * The building-height hover lookup is containment-only and cache-backed, but
 * a moving pointer can issue several requests per second. Keep it protected
 * while allowing normal hover interaction to stay smooth.
 */
export const BUILDING_LOOKUP_RATE_LIMIT = {
  max: 20,
  timeWindow: 1000,
}

/**
 * Local, unproxied callers bypass the limit entirely: the benchmark/parity
 * skills (check-popup fires 115 popup queries at 4-concurrent, check-world
 * similar) and smoke tests hit http://localhost:PORT directly. This cannot be
 * abused from outside: public visitors arrive through Caddy which always sets
 * X-Forwarded-For, so their request.ip is the public client address, and a
 * remote direct hit on the public port keeps its own socket address —
 * trustProxy honours forwarding headers from loopback peers only.
 */
export function isLoopbackClient(ip: string): boolean {
  return ip === '127.0.0.1' || ip === '::1' || ip === '::ffff:127.0.0.1'
}

/**
 * Rate-limit bucket key: IPv4 = the full address; IPv6 = the /64 prefix.
 * ISPs hand out at least a /64 per subscriber, so a single abuser rotating
 * interface IDs inside their prefix still shares one bucket, while two
 * different IPv4 customers behind different addresses never collide.
 */
export function rateLimitClientKey(ip: string): string {
  const bare = ip.split('%')[0] // strip any link-local zone index
  if (!bare.includes(':')) return bare // IPv4

  // IPv4-mapped IPv6 (Node reports dual-stack sockets as ::ffff:a.b.c.d):
  // key on the embedded IPv4 so v4 clients get per-address buckets either way.
  const mapped = bare.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/i)
  if (mapped) return mapped[1]

  // Expand a possibly `::`-abbreviated address to 8 hextets, keep the first 4.
  const [head, tail = ''] = bare.split('::')
  const headParts = head ? head.split(':') : []
  const tailParts = tail ? tail.split(':') : []
  const zeros = Array(8 - headParts.length - tailParts.length).fill('0')
  const hextets = [...headParts, ...zeros, ...tailParts]
  return `${hextets.slice(0, 4).join(':')}::/64`
}
