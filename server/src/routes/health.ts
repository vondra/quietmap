// Process liveness and dependency readiness. Public responses name only the
// failed component; absolute paths and native errors stay in server logs.
import type { FastifyInstance, FastifyReply } from 'fastify'
import { READINESS_COMPONENTS, type ReadinessCheck, type ReadinessResult } from '../runtime-readiness.js'

export async function healthRoutes(app: FastifyInstance, readiness: ReadinessCheck): Promise<void> {
  let lastLoggedFailure = ''

  app.get('/api/live', async (_request, reply) => {
    reply.header('Cache-Control', 'no-store')
    return { status: 'ok' }
  })

  const ready = async (reply: FastifyReply) => {
    reply.header('Cache-Control', 'no-store')
    let result: ReadinessResult
    try {
      result = await readiness()
    } catch (error) {
      app.log.error(error, 'runtime readiness check crashed')
      return reply.code(503).send({ status: 'not_ready', failed: [...READINESS_COMPONENTS] })
    }
    if (result.ready) {
      lastLoggedFailure = ''
      return reply.send({ status: 'ok' })
    }
    const failureKey = JSON.stringify(result.errors)
    if (failureKey !== lastLoggedFailure) {
      app.log.warn({ readiness_errors: result.errors }, 'runtime readiness failed')
      lastLoggedFailure = failureKey
    }
    return reply.code(503).send({ status: 'not_ready', failed: result.failed })
  }

  app.get('/api/ready', async (_request, reply) => ready(reply))
  // Compatibility for existing monitors and validation scripts.
  app.get('/api/health', async (_request, reply) => ready(reply))
}
