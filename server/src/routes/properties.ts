import path from 'node:path'
import { existsSync, readFileSync, mkdirSync } from 'node:fs'
import type { FastifyInstance } from 'fastify'
import fastifyStatic from '@fastify/static'
import { DATA_YEAR as YEAR } from '../data-year.js'

const PROPERTIES_DIR = path.resolve(import.meta.dirname, '..', '..', '..', 'data', 'prepared', YEAR, 'properties')
const PROPERTIES_JSON = path.join(PROPERTIES_DIR, 'properties.json')
const PHOTOS_DIR = path.join(PROPERTIES_DIR, 'photos')

export async function propertiesRoutes(app: FastifyInstance): Promise<void> {
  // The full CZ listing set (a few thousand); the client filters by viewport +
  // type + noise. Returns [] until an import has run, so the overlay degrades
  // to empty rather than erroring.
  app.get('/api/properties', async (_request, reply) => {
    reply.header('Cache-Control', 'no-store')
    reply.type('application/json; charset=utf-8')
    return reply.send(existsSync(PROPERTIES_JSON) ? readFileSync(PROPERTIES_JSON, 'utf8') : '[]')
  })

  // @fastify/static throws if `root` is missing — ensure the dir exists (empty
  // is fine; a missing photo just 404s).
  mkdirSync(PHOTOS_DIR, { recursive: true })
  app.register(fastifyStatic, {
    root: PHOTOS_DIR,
    prefix: '/api/properties/photos/',
    decorateReply: false,
    setHeaders(res) {
      res.setHeader('Cache-Control', 'public, max-age=86400')
      res.setHeader('Access-Control-Allow-Origin', '*')
    },
  })
}
