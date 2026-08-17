//! Authenticated loopback IPC endpoints for PREPARE/ACK/COMMIT publication.

import type { FastifyInstance, FastifyReply, FastifyRequest } from 'fastify'
import { requireLocalPeer } from './internal-access.js'
import { MAX_POPUP_PUBLISH_MANIFEST_TEXT_BYTES } from './published-line-model-contract.js'
import type { PublishedLineModel } from './published-line-model.js'

export const POPUP_PUBLISH_TOKEN_HEADER = 'x-qm-popup-publish-token'

async function requireToken(
  manager: PublishedLineModel,
  request: FastifyRequest,
  reply: FastifyReply,
): Promise<void> {
  const value = request.headers[POPUP_PUBLISH_TOKEN_HEADER]
  if (!manager.authenticate(Array.isArray(value) ? null : value)) {
    await reply.code(403).send({ error: 'popup publication authentication failed' })
  }
}

export async function registerPopupPublishIpc(
  app: FastifyInstance,
  manager: PublishedLineModel,
): Promise<void> {
  if (!manager.enabled) return
  const preHandler = [
    requireLocalPeer,
    async (request: FastifyRequest, reply: FastifyReply) => requireToken(manager, request, reply),
  ]
  app.post('/api/internal/popup-publish/prepare', {
    preHandler,
    bodyLimit: MAX_POPUP_PUBLISH_MANIFEST_TEXT_BYTES,
  },
    async (request, reply) => {
      try {
        return await manager.prepare(request.body)
      } catch (error) {
        app.log.warn({ err: error }, 'popup publication PREPARE rejected')
        return reply.code(409).send({ error: 'popup publication PREPARE rejected' })
      }
    })
  app.post('/api/internal/popup-publish/commit', { preHandler }, async (request, reply) => {
    try {
      return await manager.commit(request.body)
    } catch (error) {
      app.log.warn({ err: error }, 'popup publication COMMIT rejected')
      return reply.code(409).send({ error: 'popup publication COMMIT rejected' })
    }
  })
}
