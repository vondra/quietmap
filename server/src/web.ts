// The built frontend, served statically, its About pages, and real 404s for everything else.
// Unknown paths must remain 404s: returning index.html for scanner paths hides mistakes and makes
// sensitive-looking URLs appear to exist. Browsers get the friendly HTML page, API and scanner
// clients get JSON.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import fastifyStatic from '@fastify/static'
import type { FastifyInstance } from 'fastify'
import { NOT_FOUND_PAGE_HTML } from './not-found-page.ts'

/** A path under /about (one trailing slash allowed) and the page it names, as the frontend reads it
 *  (frontend/src/lib/about-paths.ts). */
const ABOUT_PATH = /^\/about(?:\/(.*?))?\/?$/

export async function registerWeb(app: FastifyInstance, frontendDist: string): Promise<void> {
  // The About pages the build found (frontend/src/about): only these paths open a page.
  const aboutPages = new Set(JSON.parse(readFileSync(join(frontendDist, 'about-pages.json'), 'utf8')) as string[])
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
    const about = pathname.match(ABOUT_PATH)
    if ((request.method === 'GET' || request.method === 'HEAD') && about && aboutPages.has(about[1] ?? '')) {
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
