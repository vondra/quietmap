// Internal admin routes (the /a/* area) must never be served to a DIRECT hit on the
// public app port — that would bypass Caddy's basic_auth. Gate on the real TCP peer
// (request.socket.remoteAddress): it is loopback for Caddy (reverse_proxy localhost:8520)
// and the local shell TUI, and the real remote for a direct public :8520 request. This is
// deliberately NOT request.ip — with trustProxy set, request.ip is the X-Forwarded-For
// client, so an AUTHED Caddy-proxied request would carry the public client IP and be
// wrongly rejected. So: Caddy(+basic_auth) and loopback pass; the raw-port bypass 404s.
import type { FastifyReply, FastifyRequest } from 'fastify'

function isLoopbackIp(ip: string): boolean {
  return ip === '127.0.0.1' || ip === '::1' || ip === '::ffff:127.0.0.1'
}

export async function requireLocalPeer(request: FastifyRequest, reply: FastifyReply): Promise<void> {
  if (!isLoopbackIp(request.socket.remoteAddress ?? '')) {
    await reply.code(404).send({ error: 'Not found' })
  }
}
