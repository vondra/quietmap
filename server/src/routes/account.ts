// The optional sign-in by a mailed link, and the signed-in visitor's places (../accounts). The link
// carries its token after `#`, so no request line, log or referrer holds it; the map's /signin page
// shows the account it signs in and signs in only at a press. A request that changes anything must
// come from the map's own origin, and one that changes an account must name it: a page still showing
// another account (signed out and in again in another tab) is refused with 409. Without the accounts'
// configuration every route answers 503.
import type { FastifyInstance, FastifyReply, FastifyRequest } from 'fastify'
import type { SendLink } from '../accounts/mail.ts'
import { parseRecent, parseTitle, parseTitles } from '../accounts/places.ts'
import { HOUR_MS, SESSION_MS, type AccountStore, type SessionUser } from '../accounts/store.ts'

export interface Accounts {
  store: AccountStore
  sendLink: SendLink
  /** The map's own origin as a browser sends it (`https://quietmap.org`): the links point there and
   *  only its pages change anything. */
  origin: string
}

const COOKIE = '__Host-qm_session'
/** The account a change is meant for, as the page shows it. */
const ACCOUNT_HEADER = 'x-quietmap-account'
/** Sign-in mails a day from the whole server: half of the mail account's 100 a day (Resend's free
 *  plan, 2026-10), so that a script asking for links to many addresses leaves the rest of its mail. */
export const MAILS_A_DAY = 50
/** Links one client may ask for, and look up or use, in an hour (an IPv4 address or an IPv6 /64, as
 *  ../rate-limit.ts keys them). */
const LINKS_A_CLIENT_AN_HOUR = { config: { rateLimit: { max: 10, timeWindow: HOUR_MS } } }
const LINK_USES_A_CLIENT_AN_HOUR = { config: { rateLimit: { max: 30, timeWindow: HOUR_MS } } }
const DAY_MS = 86_400_000
/** An address in plain ASCII: a lookalike letter from another script, a direction mark or a
 *  zero-width character would show the confirming page one address and mail another. */
const EMAIL = /^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/
const EMAIL_MAX = 254
/** Ended links and sessions are forgotten this often. */
const FORGET_MS = 10 * 60_000

/** The value of cookie `name` in a Cookie header. */
function cookie(header: string | undefined, name: string): string | null {
  for (const part of (header ?? '').split(';')) {
    const [key, ...value] = part.trim().split('=')
    if (key === name) return value.join('=')
  }
  return null
}

const sessionCookie = (value: string, maxAgeMs: number) =>
  `${COOKIE}=${value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=${Math.floor(maxAgeMs / 1000)}`

const body = (request: FastifyRequest) =>
  (typeof request.body === 'object' && request.body !== null ? request.body : {}) as Record<string, unknown>

/** The accounts, once configured and, for a change, asked from the map's own pages; null after
 *  answering. */
function checked(accounts: Accounts | null, request: FastifyRequest, reply: FastifyReply, needs: { change?: boolean }): Accounts | null {
  if (!accounts) return void reply.status(503).send({ error: 'Sign-in is not available here' }), null
  if (needs.change && request.headers.origin !== accounts.origin) return void reply.status(403).send({ error: 'Not from the map' }), null
  reply.header('Cache-Control', 'no-store')
  return accounts
}

/** As `checked`, and the signed-in visitor (a session renewed sends its cookie anew), who for a
 *  change must be the account the page names. */
export function signedIn(accounts: Accounts | null, request: FastifyRequest, reply: FastifyReply, needs: { change?: boolean }): [Accounts, SessionUser] | null {
  const ready = checked(accounts, request, reply, needs)
  if (!ready) return null
  const session = cookie(request.headers.cookie, COOKIE)
  const user = session ? ready.store.sessionUser(session, Date.now()) : null
  if (!user) return void reply.status(401).send({ error: 'Not signed in' }), null
  if (user.renewed) reply.header('Set-Cookie', sessionCookie(session!, SESSION_MS))
  if (needs.change && request.headers[ACCOUNT_HEADER] !== user.email) {
    return void reply.status(409).send({ error: 'Signed in as another account' }), null
  }
  return [ready, user]
}

export async function accountRoutes(app: FastifyInstance, { accounts }: { accounts: Accounts | null }) {
  // The sign-in mails sent this day: [the day's start, how many].
  let mailed = [0, 0]
  if (accounts) {
    const forget = setInterval(() => accounts.store.forgetEnded(Date.now()), FORGET_MS)
    forget.unref()
    app.addHook('onClose', async () => clearInterval(forget))
  }

  // The same answer whether the address has an account or not, and whether its three links of the
  // hour ran out (../accounts/store.ts).
  app.post('/api/auth/email', LINKS_A_CLIENT_AN_HOUR, async (request, reply) => {
    const ready = checked(accounts, request, reply, { change: true })
    if (!ready) return
    const email = String(body(request).email ?? '').trim().toLowerCase()
    if (email.length > EMAIL_MAX || !EMAIL.test(email)) return reply.status(400).send({ error: 'That is not an e-mail address' })
    const now = Date.now()
    if (now - mailed[0] >= DAY_MS) mailed = [now, 0]
    if (mailed[1] >= MAILS_A_DAY) {
      return reply.status(503).header('Retry-After', Math.ceil((mailed[0] + DAY_MS - now) / 1000))
        .send({ error: 'Too many sign-in mails today. Try again tomorrow.' })
    }
    const token = ready.store.requestLink(email, now)
    if (token == null) return reply.status(204).send()
    try {
      await ready.sendLink(email, `${ready.origin}/signin#${token}`)
    } catch (error) {
      // A link that was never mailed neither works nor counts against the address.
      ready.store.forgetLink(token)
      request.log.error({ err: error }, 'sign-in mail not sent')
      return reply.status(502).send({ error: 'The mail could not be sent. Try again in a minute.' })
    }
    mailed[1]++
    return reply.status(204).send()
  })

  app.post('/api/auth/link', LINK_USES_A_CLIENT_AN_HOUR, async (request, reply) => {
    const ready = checked(accounts, request, reply, { change: true })
    if (!ready) return
    const email = ready.store.linkEmail(String(body(request).token ?? ''), Date.now())
    return email ? { email } : reply.status(404).send({ error: 'This link has expired or was used' })
  })

  app.post('/api/auth/link/use', LINK_USES_A_CLIENT_AN_HOUR, async (request, reply) => {
    const ready = checked(accounts, request, reply, { change: true })
    if (!ready) return
    const session = ready.store.useLink(String(body(request).token ?? ''), Date.now())
    if (!session) return reply.status(404).send({ error: 'This link has expired or was used' })
    return reply.header('Set-Cookie', sessionCookie(session, SESSION_MS)).status(204).send()
  })

  app.get('/api/me', async (request, reply) => {
    const ready = signedIn(accounts, request, reply, {})
    if (!ready) return
    const [{ store }, user] = ready
    return { email: user.email, ...store.places(user.id) }
  })

  app.post('/api/auth/signout', async (request, reply) => {
    const ready = checked(accounts, request, reply, { change: true })
    if (!ready) return
    const session = cookie(request.headers.cookie, COOKIE)
    if (session) ready.store.signOut(session)
    return reply.header('Set-Cookie', sessionCookie('', 0)).status(204).send()
  })

  app.delete('/api/me', async (request, reply) => {
    const ready = signedIn(accounts, request, reply, { change: true })
    if (!ready) return
    const [{ store }, user] = ready
    store.deleteUser(user.id)
    return reply.header('Set-Cookie', sessionCookie('', 0)).status(204).send()
  })

  app.put('/api/places/title', async (request, reply) => {
    const ready = signedIn(accounts, request, reply, { change: true })
    if (!ready) return
    const [{ store }, user] = ready
    const parsed = parseTitle(request.body, Date.now())
    if (typeof parsed === 'string') return reply.status(400).send({ error: parsed })
    return { titles: store.setTitle(user.id, parsed.at, parsed.title, parsed.updatedAt) }
  })

  app.put('/api/places/recent', async (request, reply) => {
    const ready = signedIn(accounts, request, reply, { change: true })
    if (!ready) return
    const [{ store }, user] = ready
    const recent = parseRecent(body(request).recent)
    if (typeof recent === 'string') return reply.status(400).send({ error: recent })
    store.setRecent(user.id, recent)
    return reply.status(204).send()
  })

  app.post('/api/places/import', async (request, reply) => {
    const ready = signedIn(accounts, request, reply, { change: true })
    if (!ready) return
    const [{ store }, user] = ready
    const titles = parseTitles(body(request).titles, Date.now())
    const recent = parseRecent(body(request).recent)
    if (typeof titles === 'string' || typeof recent === 'string') {
      return reply.status(400).send({ error: typeof titles === 'string' ? titles : recent })
    }
    return store.importPlaces(user.id, { titles, recent })
  })
}
