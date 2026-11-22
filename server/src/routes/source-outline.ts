// An industrial source's mapped boundary, read only when its row opens, apart from the noise stream.
import { createHash } from 'node:crypto'
import { existsSync, readFileSync, realpathSync } from 'node:fs'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import type { FastifyInstance } from 'fastify'

const COMPLETION_LAYOUT = 'qmsq1'

/** Verify a completed year root's `complete` marker and `year.json` digest match `expectedYear`
 *  (mirrors `tiles/src/completion.rs`) and return its canonical pinned path. */
function verifyCompletedYearRoot(yearRoot: string, expectedYear: string): string {
  const pinned = realpathSync(yearRoot)
  const completeText = readFileSync(join(pinned, 'complete'), 'utf8')
  const newline = completeText.indexOf('\n')
  if (newline < 0 || completeText.slice(0, newline) !== COMPLETION_LAYOUT) {
    throw new Error(`${pinned} is not laid out as ${COMPLETION_LAYOUT}`)
  }
  const marker = JSON.parse(completeText.slice(newline + 1)) as { manifest_sha256?: unknown; as_of?: unknown }
  const manifestBytes = readFileSync(join(pinned, 'year.json'))
  const manifest = JSON.parse(manifestBytes.toString('utf8')) as { year?: unknown; as_of?: unknown }
  const digest = createHash('sha256').update(manifestBytes).digest('hex')
  if (marker.manifest_sha256 !== digest || typeof manifest.as_of !== 'string' || marker.as_of !== manifest.as_of) {
    throw new Error(`${pinned}: completion layout, manifest hash or as_of mismatch`)
  }
  if (manifest.year !== expectedYear) {
    throw new Error(`${pinned}: completion manifest is for year ${String(manifest.year)}, not ${expectedYear}`)
  }
  return pinned
}

export async function sourceOutlineRoutes(
  app: FastifyInstance,
  { preparedDir, years }: { preparedDir: string; years: readonly string[] },
): Promise<void> {
  const databases = new Map(years.flatMap(year => {
    const yearRoot = join(preparedDir, year)
    if (!existsSync(join(yearRoot, 'source-outlines.sqlite'))) return []
    const pinned = verifyCompletedYearRoot(yearRoot, year)
    const database = new DatabaseSync(join(pinned, 'source-outlines.sqlite'), { readOnly: true })
    const hasMeta = database.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'").get()
    if (hasMeta) {
      const row = database.prepare("SELECT value FROM meta WHERE key = 'year'").get() as { value?: unknown } | undefined
      if (row?.value !== year) {
        database.close()
        throw new Error(`${pinned}/source-outlines.sqlite is for year ${String(row?.value)}, not ${year}`)
      }
    }
    return [[year, database] as const]
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
