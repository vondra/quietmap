// The built frontend, served statically, its About pages, and real 404s for everything else.
// Unknown paths must remain 404s: returning index.html for scanner paths hides mistakes and makes
// sensitive-looking URLs appear to exist. Browsers get the friendly HTML page, API and scanner
// clients get JSON.
import fastifyStatic from '@fastify/static'
import type { FastifyInstance } from 'fastify'
import { NOT_FOUND_PAGE_HTML } from './not-found-page.ts'

/** The About pages the frontend renders (frontend/src/about). */
const ABOUT_PAGES = /^\/about(?:\/(?:methodology|credits|news))?\/?$/

export async function registerWeb(app: FastifyInstance, frontendDist: string): Promise<void> {
  // preCompressed: the frontend build writes sibling .br files, served with
  // zero per-request compression CPU.
  await app.register(fastifyStatic, { root: frontendDist, preCompressed: true })

  app.setNotFoundHandler(async (request, reply) => {
    // As the browser reads it (frontend/src/main.tsx), percent-encoding kept: an encoded About path
    // is no page.
    let pathname: string
    try {
      pathname = new URL(request.raw.url ?? '/', 'http://localhost').pathname
    } catch {
      return reply.status(404).send({ error: 'Not found' })
    }
    if ((request.method === 'GET' || request.method === 'HEAD') && ABOUT_PAGES.test(pathname)) {
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
