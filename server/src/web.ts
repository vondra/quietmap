// Frontend static serving and the deliberately small SPA route surface.
// Unknown paths must remain real 404s: returning index.html for scanner paths
// hides mistakes and makes sensitive-looking URLs appear to exist. Browsers
// get the friendly HTML page, API/scanner clients get JSON.
import fastifyStatic from '@fastify/static'
import type { FastifyInstance } from 'fastify'
import { NOT_FOUND_PAGE_HTML } from './not-found-page.js'

function isSpaPath(pathname: string): boolean {
  return pathname === '/' || /^\/about(?:\/[a-z0-9-]+)*\/?$/.test(pathname)
}

export async function registerWeb(app: FastifyInstance, frontendDist: string): Promise<void> {
  // preCompressed: the frontend build writes sibling .br files, served with
  // zero per-request compression CPU.
  await app.register(fastifyStatic, { root: frontendDist, preCompressed: true })

  app.setNotFoundHandler(async (request, reply) => {
    let pathname: string
    try {
      pathname = decodeURIComponent(new URL(request.raw.url ?? '/', 'http://localhost').pathname)
    } catch {
      return reply.status(404).send({ error: 'Not found' })
    }
    if ((request.method === 'GET' || request.method === 'HEAD') && isSpaPath(pathname)) {
      return reply.sendFile('index.html')
    }
    const wantsHtml =
      (request.method === 'GET' || request.method === 'HEAD') &&
      !pathname.startsWith('/api/') &&
      (request.headers.accept ?? '').includes('text/html')
    if (wantsHtml) {
      return reply.status(404).type('text/html; charset=utf-8').send(NOT_FOUND_PAGE_HTML)
    }
    return reply.status(404).send({ error: 'Not found' })
  })
}
