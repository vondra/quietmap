// The optional sign-in's requests (server/src/routes/account.ts): the signed-in visitor's account with
// its places, a sign-in link mailed, the link's account and its use, and the account's changes. A
// change is sent as JSON with the account the page shows, so that the server refuses it (409) once
// another account has signed in on this browser; the browser adds its origin, which the server checks.
import type { PlaceTitle, RecentPlace } from './place-rules.ts'

export interface AccountPlaces {
  titles: PlaceTitle[]
  recent: RecentPlace[]
}

export interface Account extends AccountPlaces {
  email: string
}

/** A request to the sign-in's routes, for `account` when given; null when the map was not reached. */
function send(method: 'POST' | 'PUT' | 'DELETE', url: string, { account, body }: { account?: string, body?: object } = {}): Promise<Response | null> {
  const headers: Record<string, string> = account ? { 'X-Quietmap-Account': account } : {}
  // A body-less request has no content type: the server would refuse JSON with nothing in it.
  if (body) headers['Content-Type'] = 'application/json'
  return fetch(url, { method, headers, body: body && JSON.stringify(body) }).catch(() => null)
}

/** The signed-in visitor's account; null when signed out; 'unavailable' on a server without the
 *  sign-in; undefined when the map could not tell (not reached, or failing). */
export async function fetchAccount(): Promise<Account | null | 'unavailable' | undefined> {
  const response = await fetch('/api/me', { cache: 'no-store' }).catch(() => null)
  if (!response) return undefined
  if (response.status === 401) return null
  const json = response.headers.get('content-type')?.includes('application/json')
  // A page without the API (a static host) answers its own HTML.
  if (response.status === 503 || (response.ok && !json)) return 'unavailable'
  return response.ok ? await response.json().catch(() => undefined) as Account | undefined : undefined
}

/** Mails a sign-in link: null once sent, else what to tell the visitor. */
export async function mailSignInLink(email: string): Promise<string | null> {
  const response = await send('POST', '/api/auth/email', { body: { email } })
  if (response?.ok) return null
  if (response?.status === 429) return 'Too many links asked from here. Try again in an hour.'
  const answer = await response?.json().catch(() => null) as { error?: string } | null
  return answer?.error ?? 'The map could not be reached. Try again in a minute.'
}

/** The address a link signs in; null when the link does not work; undefined when the map was not
 *  reached. */
export async function linkEmail(token: string): Promise<string | null | undefined> {
  const response = await send('POST', '/api/auth/link', { body: { token } })
  if (!response) return undefined
  return response.ok ? ((await response.json()) as { email: string }).email : null
}

/** Signs in with a link, which works once: whether it did; undefined when the map was not reached. */
export async function signInWithLink(token: string): Promise<boolean | undefined> {
  const response = await send('POST', '/api/auth/link/use', { body: { token } })
  return response ? response.ok : undefined
}

export async function signOut(): Promise<boolean> {
  return (await send('POST', '/api/auth/signout'))?.ok ?? false
}

/** Deletes `account` with its places: whether it was. */
export async function deleteAccount(account: string): Promise<boolean> {
  return (await send('DELETE', '/api/me', { account }))?.ok ?? false
}

/** Names the place at a point for `account` (null removes its name). */
export function saveTitle(account: string, title: { lat: number, lng: number, title: string | null, updatedAt: number }): Promise<Response | null> {
  return send('PUT', '/api/places/title', { account, body: title })
}

/** Replaces `account`'s recent places. */
export function saveRecent(account: string, recent: RecentPlace[]): Promise<Response | null> {
  return send('PUT', '/api/places/recent', { account, body: { recent } })
}

/** Adds a browser's places to `account`: whether they were. */
export async function importPlaces(account: string, places: AccountPlaces): Promise<boolean> {
  return (await send('POST', '/api/places/import', { account, body: places }))?.ok ?? false
}
