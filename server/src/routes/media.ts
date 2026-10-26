// A signed-in visitor's recordings and photos of places (../accounts/store.ts keeps them,
// ../media/check.ts reads what an upload is): uploaded under an id the page chose, so that a retry
// after a lost answer keeps one copy; listed within 30 m of a point; read and deleted by their
// account alone. An upload is refused before its body is read when the sign-in's checks fail
// (./account.ts), and checked again once it is in, as the account may have been deleted meanwhile.
import send from '@fastify/send'
import type { FastifyInstance } from 'fastify'
import { RECORDING_SAMPLES_MAX } from '../../../frontend/src/lib/wav.ts'
import { point } from '../accounts/places.ts'
import { HOUR_MS } from '../accounts/store.ts'
import { MEDIA_KINDS, photoOf, soundOf, type MediaKind } from '../media/check.ts'
import { signedIn, type Accounts } from './account.ts'

/** An item's id: 128 random bits in hex. */
const ID = /^[0-9a-f]{32}$/
/** An upload's point, time and details: URI-encoded JSON, as a header carries Latin-1 only. */
const ABOUT_HEADER = 'x-quietmap-media'
const DETAILS_MAX = 4096
/** A photo, which the phone draws at most 4096 pixels a side, takes a few MB. */
const PHOTO_BYTES_MAX = 16 * 1024 ** 2
const UPLOADS_A_CLIENT_AN_HOUR = { config: { rateLimit: { max: 60, timeWindow: HOUR_MS } } }
const DAY_MS = 86_400_000

/** An upload's point, time and details as its header sends them, or what is wrong with them. */
function parseAbout(header: unknown, now: number): { at: { lat: number, lng: number }, takenAt: number, details: string } | string {
  let sent: unknown
  try {
    sent = JSON.parse(decodeURIComponent(String(header ?? '')))
  } catch {
    return `${ABOUT_HEADER} must be URI-encoded JSON`
  }
  const about = (typeof sent === 'object' && sent !== null ? sent : {}) as Record<string, unknown>
  const at = point(about)
  if (!at) return 'lat must be within ±90 and lng a number'
  const { takenAt } = about
  if (!Number.isInteger(takenAt) || (takenAt as number) < 0 || (takenAt as number) > now + DAY_MS) return 'takenAt must be a time (ms)'
  const details = JSON.stringify(about.details)
  if (typeof about.details !== 'object' || about.details === null || details.length > DETAILS_MAX) {
    return `details must be an object of at most ${DETAILS_MAX} characters`
  }
  return { at, takenAt: takenAt as number, details }
}

export async function mediaRoutes(app: FastifyInstance, { accounts }: { accounts: Accounts | null }) {
  // An upload's body as it came, with the kind its type names; no other type (JSON, text) is read.
  app.removeAllContentTypeParsers()
  app.addContentTypeParser('audio/wav', { parseAs: 'buffer', bodyLimit: 44 + RECORDING_SAMPLES_MAX * 2 },
    (_request, body, done) => done(null, { kind: 'sound', body }))
  app.addContentTypeParser('image/jpeg', { parseAs: 'buffer', bodyLimit: PHOTO_BYTES_MAX },
    (_request, body, done) => done(null, { kind: 'photo', body }))

  app.put<{ Params: { id: string } }>('/api/media/:id', {
    ...UPLOADS_A_CLIENT_AN_HOUR,
    onRequest: async (request, reply) => {
      if (!signedIn(accounts, request, reply, { change: true })) return reply
    },
  }, async (request, reply) => {
    const ready = signedIn(accounts, request, reply, { change: true })
    if (!ready) return
    const [{ store }, user] = ready
    const about = parseAbout(request.headers[ABOUT_HEADER], Date.now())
    if (typeof about === 'string') return reply.status(400).send({ error: about })
    if (!ID.test(request.params.id)) return reply.status(400).send({ error: 'The id must be 32 hex digits' })
    const { kind, body } = request.body as { kind: MediaKind, body: Buffer }
    const sound = kind === 'sound' ? soundOf(body) : null
    const file = kind === 'photo' ? photoOf(body) : body
    if (typeof sound === 'string' || typeof file === 'string') return reply.status(400).send({ error: sound ?? file })
    const item = store.addMedia(user.id, {
      id: request.params.id, kind, ...about.at, takenAt: about.takenAt, durationS: sound?.durationS ?? null,
    }, about.details, file as Buffer)
    if (item === 'taken') return reply.status(409).send({ error: 'Another recording or photo has this id' })
    if (item === 'full') return reply.status(507).send({ error: 'An account keeps at most 200 recordings and photos, 2 GiB' })
    if (item === 'disk full') return reply.status(507).send({ error: 'The map has no room for it now. Try again later.' })
    return item
  })

  app.get<{ Querystring: { lat?: string, lng?: string } }>('/api/media', async (request, reply) => {
    const ready = signedIn(accounts, request, reply, {})
    if (!ready) return
    const [{ store }, user] = ready
    const at = point({ lat: Number(request.query.lat), lng: Number(request.query.lng) })
    if (!at) return reply.status(400).send({ error: 'lat must be within ±90 and lng a number' })
    return { items: store.mediaNear(user.id, at) }
  })

  // Read by an <audio> or <img>, which send the cookie alone; a player asks for ranges (iOS's for
  // bytes 0-1 first), so the bytes go uncompressed.
  app.get<{ Params: { id: string } }>('/api/media/:id', { compress: false }, async (request, reply) => {
    const ready = signedIn(accounts, request, reply, {})
    if (!ready) return
    const [{ store }, user] = ready
    const file = ID.test(request.params.id) ? store.mediaFile(user.id, request.params.id) : null
    if (!file) return reply.status(404).send({ error: 'No such recording or photo' })
    const sent = await send(request.raw, `/${file.name}`, { root: file.directory, cacheControl: false, etag: false, lastModified: false })
    if (sent.type !== 'file') {
      // An error's own headers describe its HTML page; a range past the end keeps its Content-Range.
      if (sent.statusCode === 416) return reply.status(416).header('Content-Range', sent.headers['Content-Range']).send({ error: 'No such range' })
      return reply.status(sent.statusCode).send({ error: 'The file could not be read' })
    }
    return reply.status(sent.statusCode).headers(sent.headers).header('Content-Type', MEDIA_KINDS[file.kind].mime)
      .header('X-Content-Type-Options', 'nosniff').header('Content-Disposition', `inline; filename="${file.name}"`).send(sent.stream)
  })

  app.delete<{ Params: { id: string } }>('/api/media/:id', async (request, reply) => {
    const ready = signedIn(accounts, request, reply, { change: true })
    if (!ready) return
    const [{ store }, user] = ready
    return ID.test(request.params.id) && store.deleteMedia(user.id, request.params.id)
      ? reply.status(204).send()
      : reply.status(404).send({ error: 'No such recording or photo' })
  })
}
