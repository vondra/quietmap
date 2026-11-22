// An industrial source's mapped boundary, read only when its row opens, apart from the noise stream.
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import type { FastifyInstance } from 'fastify'

export async function sourceOutlineRoutes(
  app: FastifyInstance,
  { preparedDir, years }: { preparedDir: string; years: readonly string[] },
): Promise<void> {
  const databases = new Map(years.flatMap(year => {
    const path = join(preparedDir, year, 'source-outlines.sqlite')
    return existsSync(path) ? [[year, new DatabaseSync(path, { readOnly: true })] as const] : []
  }))
  app.addHook('onClose', async () => { for (const database of databases.values()) database.close() })
  const queries = new Map([...databases].map(([year, database]) =>
    [year, database.prepare('SELECT geometry FROM outlines WHERE id = ? ORDER BY geometry')]))

  app.get<{ Params: { id: string }; Querystring: { year?: string } }>('/api/source-outline/:id', async (request, reply) => {
    const { id } = request.params
    const year = request.query.year ?? years[0]
    if (!/^[0-9a-f]{16}$/.test(id) || !years.includes(year)) {
      return reply.code(400).send({ error: 'A source id and a served year are required.' })
    }
    const query = queries.get(year)
    if (!query) return reply.code(503).send({ error: 'Source outlines are unavailable.' })
    const features = query.all(id).map(row => ({
      type: 'Feature', properties: {}, geometry: JSON.parse(row.geometry as string) as unknown,
    }))
    return reply.header('Cache-Control', 'no-cache').send({ type: 'FeatureCollection', features })
  })
}
