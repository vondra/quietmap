// The accounts in SQLite (node:sqlite): the sign-in links mailed, the sessions, and each account's
// place names and recent places (frontend/src/lib/place-rules.ts). Tokens are 32 random bytes and
// only their SHA-256 is kept, so the database alone signs nobody in. Every change that must not half
// happen is one transaction.
import { createHash, randomBytes } from 'node:crypto'
import { DatabaseSync } from 'node:sqlite'
import { mergedTitles, withTitle, type PlaceTitle, type RecentPlace } from '../../../frontend/src/lib/place-rules.ts'

/** A link works this long after it is mailed. */
export const LINK_MS = 15 * 60_000
/** Links mailed to one address in an hour. */
export const LINKS_AN_ADDRESS_AN_HOUR = 3
/** A session lasts this long after the day it was last used. */
export const SESSION_MS = 180 * 86_400_000
/** An address that asked for a link is kept this long, to count its links. */
export const HOUR_MS = 3_600_000
const DAY_MS = 86_400_000

const SCHEMA = `
CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS login_links (token_sha256 TEXT PRIMARY KEY, email TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS login_links_email ON login_links (email, created_at);
CREATE TABLE IF NOT EXISTS sessions (token_sha256 TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE, expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS place_titles (user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  position INTEGER NOT NULL, lat REAL NOT NULL, lng REAL NOT NULL, title TEXT NOT NULL,
  updated_at INTEGER NOT NULL, PRIMARY KEY (user_id, position));
CREATE TABLE IF NOT EXISTS recent_places (user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  position INTEGER NOT NULL, lat REAL NOT NULL, lng REAL NOT NULL, place TEXT, sone REAL, lden REAL,
  PRIMARY KEY (user_id, position));
`

const sha256 = (token: string) => createHash('sha256').update(token).digest('hex')
const newToken = () => randomBytes(32).toString('base64url')

export interface Places {
  titles: PlaceTitle[]
  recent: RecentPlace[]
}

export interface SessionUser {
  id: number
  email: string
  /** This use renewed the session (at most daily), so its cookie is sent anew. */
  renewed: boolean
}

export class AccountStore {
  readonly #db: DatabaseSync

  constructor(path: string) {
    this.#db = new DatabaseSync(path)
    this.#db.exec('PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;')
    this.#db.exec(SCHEMA)
  }

  close(): void {
    this.#db.close()
  }

  /** `run` in one transaction, rolled back if it throws. */
  #transaction<T>(run: () => T): T {
    this.#db.exec('BEGIN IMMEDIATE')
    try {
      const result = run()
      this.#db.exec('COMMIT')
      return result
    } catch (error) {
      this.#db.exec('ROLLBACK')
      throw error
    }
  }

  /** A new link's token for `email`, or null when the address had its links this hour. */
  requestLink(email: string, now: number): string | null {
    return this.#transaction(() => {
      const { count } = this.#db.prepare('SELECT count(*) AS count FROM login_links WHERE email = ? AND created_at > ?')
        .get(email, now - HOUR_MS) as { count: number }
      if (count >= LINKS_AN_ADDRESS_AN_HOUR) return null
      const token = newToken()
      this.#db.prepare('INSERT INTO login_links (token_sha256, email, created_at, expires_at) VALUES (?, ?, ?, ?)')
        .run(sha256(token), email, now, now + LINK_MS)
      return token
    })
  }

  /** A link whose mail could not be sent: it neither works nor counts. */
  forgetLink(token: string): void {
    this.#db.prepare('DELETE FROM login_links WHERE token_sha256 = ?').run(sha256(token))
  }

  /** Forgets the addresses that asked for a link over an hour ago and the sessions that ended: a
   *  link never used leaves nothing behind. */
  forgetEnded(now: number): void {
    this.#db.prepare('DELETE FROM login_links WHERE created_at <= ?').run(now - HOUR_MS)
    this.#db.prepare('DELETE FROM sessions WHERE expires_at <= ?').run(now)
  }

  /** The address a link signs in, while it works. */
  linkEmail(token: string, now: number): string | null {
    const row = this.#db.prepare('SELECT email FROM login_links WHERE token_sha256 = ? AND expires_at > ?')
      .get(sha256(token), now) as { email: string } | undefined
    return row?.email ?? null
  }

  /** Uses a link once: a new session's token for its address's account, made at its first link, or
   *  null. A token that names no link takes no write lock. */
  useLink(token: string, now: number): string | null {
    if (this.linkEmail(token, now) == null) return null
    return this.#transaction(() => {
      const row = this.#db.prepare('DELETE FROM login_links WHERE token_sha256 = ? AND expires_at > ? RETURNING email')
        .get(sha256(token), now) as { email: string } | undefined
      if (!row) return null
      this.#db.prepare('INSERT INTO users (email, created_at) VALUES (?, ?) ON CONFLICT (email) DO NOTHING').run(row.email, now)
      const { id } = this.#db.prepare('SELECT id FROM users WHERE email = ?').get(row.email) as { id: number }
      const session = newToken()
      this.#db.prepare('INSERT INTO sessions (token_sha256, user_id, expires_at) VALUES (?, ?, ?)')
        .run(sha256(session), id, now + SESSION_MS)
      return session
    })
  }

  /** The account a session signs in, while it lasts, renewed for its whole length at most daily. */
  sessionUser(session: string, now: number): SessionUser | null {
    const hash = sha256(session)
    const row = this.#db.prepare(`SELECT users.id AS id, users.email AS email, sessions.expires_at AS expiresAt
      FROM sessions JOIN users ON users.id = sessions.user_id WHERE sessions.token_sha256 = ? AND sessions.expires_at > ?`)
      .get(hash, now) as { id: number, email: string, expiresAt: number } | undefined
    if (!row) return null
    const renewed = row.expiresAt <= now + SESSION_MS - DAY_MS
    if (renewed) this.#db.prepare('UPDATE sessions SET expires_at = ? WHERE token_sha256 = ?').run(now + SESSION_MS, hash)
    return { id: row.id, email: row.email, renewed }
  }

  signOut(session: string): void {
    this.#db.prepare('DELETE FROM sessions WHERE token_sha256 = ?').run(sha256(session))
  }

  /** The account and everything it keeps: its sessions, places and links not yet used. */
  deleteUser(id: number): void {
    this.#transaction(() => {
      this.#db.prepare('DELETE FROM login_links WHERE email = (SELECT email FROM users WHERE id = ?)').run(id)
      this.#db.prepare('DELETE FROM users WHERE id = ?').run(id)
    })
  }

  #titles(id: number): PlaceTitle[] {
    return this.#db.prepare('SELECT lat, lng, title, updated_at AS updatedAt FROM place_titles WHERE user_id = ? ORDER BY position')
      .all(id) as unknown as PlaceTitle[]
  }

  places(id: number): Places {
    const recent = this.#db.prepare('SELECT lat, lng, place, sone, lden FROM recent_places WHERE user_id = ? ORDER BY position')
      .all(id) as unknown as RecentPlace[]
    return { titles: this.#titles(id), recent }
  }

  #writeTitles(id: number, titles: PlaceTitle[]): void {
    this.#db.prepare('DELETE FROM place_titles WHERE user_id = ?').run(id)
    const insert = this.#db.prepare('INSERT INTO place_titles (user_id, position, lat, lng, title, updated_at) VALUES (?, ?, ?, ?, ?, ?)')
    titles.forEach((title, k) => insert.run(id, k, title.lat, title.lng, title.title, title.updatedAt))
  }

  #writeRecent(id: number, recent: RecentPlace[]): void {
    this.#db.prepare('DELETE FROM recent_places WHERE user_id = ?').run(id)
    const insert = this.#db.prepare('INSERT INTO recent_places (user_id, position, lat, lng, place, sone, lden) VALUES (?, ?, ?, ?, ?, ?, ?)')
    recent.forEach((place, k) => insert.run(id, k, place.lat, place.lng, place.place, place.sone, place.lden))
  }

  /** Names the place at `at` (null removes its name) as `withTitle` does; the account's names. */
  setTitle(id: number, at: { lat: number, lng: number }, title: string | null, updatedAt: number): PlaceTitle[] {
    return this.#transaction(() => {
      const before = this.#titles(id)
      const titles = withTitle(before, at, title, updatedAt)
      if (titles !== before) this.#writeTitles(id, titles)
      return titles
    })
  }

  setRecent(id: number, recent: RecentPlace[]): void {
    this.#transaction(() => this.#writeRecent(id, recent))
  }

  /** A browser's places added to the account at the visitor's word: the names merged
   *  (`mergedTitles`), the recent places taken only when the account has none. */
  importPlaces(id: number, browser: Places): Places {
    return this.#transaction(() => {
      const account = this.places(id)
      const places = {
        titles: mergedTitles(account.titles, browser.titles),
        recent: account.recent.length > 0 ? account.recent : browser.recent,
      }
      this.#writeTitles(id, places.titles)
      this.#writeRecent(id, places.recent)
      return places
    })
  }
}
