/**
 * Nightly-style anonymous web-stats aggregation: reads Caddy JSON access
 * logs (root-owned → `sudo -n cat`, user vondra has passwordless sudo) and
 * upserts ANONYMIZED per-(site, UTC day) aggregates into SQLite. The client
 * IP never leaves memory — it feeds only the GeoIP lookup and the HLL
 * visitor sketch (see lib/web-stats-aggregates.ts for the privacy contract).
 *
 * Usage:  npx tsx web-stats.ts [site ...]        (default site: quietmap.org)
 * Logs:   /var/log/caddy/{site}.access.log       (current file only)
 *
 * Incrementality: data/web-stats-state.json keeps {inode, offset} per log;
 * a changed inode (rotation) or shrunk file restarts at 0. Rotated *.gz
 * archives are NOT read yet — when adding them, read archives older than
 * the current file first, then continue at offset 0 of the live file.
 * Only complete lines are consumed: a trailing partial line (Caddy
 * mid-write) is left for the next run.
 *
 * Run ownership: data/web-stats-run.lock excludes overlapping timer and
 * manual invocations from before state loading until every output is closed.
 * SQLite owns the lock, so process death releases it without stale cleanup.
 *
 * Exactly-once: the DB is committed BEFORE the state file is written, so a
 * crash between the two reprocesses bytes and double-counts the additive
 * counters (visitors stay exact — sketch merge is idempotent). Do not
 * delete the state file; a full re-run double-counts by design.
 *
 * Outputs: data/web-stats.sqlite (one table per aggregate, upsert keyed by
 * site+day+dimension), data/web-stats-latest.json (latest-day snapshot),
 * and a human-readable summary on stdout.
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import {
  aggregateAccessRecord,
  DayAccumulator,
  SEARCH_TERM_MIN_COUNT,
  VisitorSketch,
} from './lib/web-stats-aggregates.js'
import { tryAcquireSqliteRunLock } from './lib/sqlite-run-lock.js'

const REPO_ROOT = resolve(import.meta.dirname, '..')
const DATABASE_PATH = resolve(REPO_ROOT, 'data/web-stats.sqlite')
const STATE_PATH = resolve(REPO_ROOT, 'data/web-stats-state.json')
const SNAPSHOT_PATH = resolve(REPO_ROOT, 'data/web-stats-latest.json')
const RUN_LOCK_PATH = resolve(REPO_ROOT, 'data/web-stats-run.lock')
const GEOIP_DATABASE_PATH = resolve(REPO_ROOT, 'data/prepared/geoip/dbip-city-lite.mmdb')
const MAX_SUDO_OUTPUT_BYTES = 1 << 30

interface MmdbReader {
  get(ip: string): { country?: { iso_code?: string } } | null
}

function openGeoipCountryLookup(): (ip: string) => string {
  try {
    // mmdb-lib is not a pipeline dependency; reuse the copy the server
    // already installs rather than adding a second source of truth.
    const serverRequire = createRequire(resolve(REPO_ROOT, 'server/package.json'))
    const { Reader } = serverRequire('mmdb-lib') as {
      Reader: new (buffer: Buffer) => MmdbReader
    }
    const reader = new Reader(readFileSync(GEOIP_DATABASE_PATH))
    return (ip) => {
      try {
        return reader.get(ip)?.country?.iso_code ?? '??'
      } catch {
        return '??'
      }
    }
  } catch (error) {
    console.warn(`geoip unavailable (${(error as Error).message}); every country becomes '??'`)
    return () => '??'
  }
}

interface LogFileState {
  inode: number
  offset: number
}

interface StatsState {
  files: Record<string, LogFileState>
}

function loadState(): StatsState {
  try {
    return JSON.parse(readFileSync(STATE_PATH, 'utf8')) as StatsState
  } catch {
    return { files: {} }
  }
}

function saveJsonAtomically(destination: string, value: unknown): void {
  const temporary = `${destination}.tmp`
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`)
  renameSync(temporary, destination)
}

function saveState(state: StatsState): void {
  saveJsonAtomically(STATE_PATH, state)
}

function sudoRead(argv: string[]): Buffer {
  return execFileSync('sudo', ['-n', ...argv], { maxBuffer: MAX_SUDO_OUTPUT_BYTES })
}

interface NewLogBytes {
  completeLines: Buffer
  newOffset: number
  inode: number
}

/** Returns the not-yet-consumed complete lines, or null when up to date. */
function readNewLogBytes(logPath: string, state: StatsState): NewLogBytes | null {
  const stats = statSync(logPath)
  const prior = state.files[logPath]
  const offset =
    prior && prior.inode === stats.ino && prior.offset <= stats.size ? prior.offset : 0
  if (stats.size === offset) return null
  const chunk =
    offset === 0
      ? sudoRead(['cat', logPath])
      : sudoRead(['tail', '-c', `+${offset + 1}`, logPath]) // tail -c is 1-based
  const lastNewline = chunk.lastIndexOf(0x0a)
  if (lastNewline < 0) return null
  return {
    completeLines: chunk.subarray(0, lastNewline + 1),
    newOffset: offset + lastNewline + 1,
    inode: stats.ino,
  }
}

const DIMENSION_TABLES = [
  { table: 'country_stats', dimension: 'country', value: 'requests', dimType: 'TEXT' },
  { table: 'browser_stats', dimension: 'browser', value: 'requests', dimType: 'TEXT' },
  { table: 'os_stats', dimension: 'os', value: 'requests', dimType: 'TEXT' },
  { table: 'device_stats', dimension: 'device', value: 'requests', dimType: 'TEXT' },
  { table: 'language_stats', dimension: 'language', value: 'requests', dimType: 'TEXT' },
  { table: 'hour_stats', dimension: 'hour', value: 'requests', dimType: 'INTEGER' },
  { table: 'referer_stats', dimension: 'referer', value: 'visits', dimType: 'TEXT' },
  { table: 'api_usage_stats', dimension: 'endpoint', value: 'requests', dimType: 'TEXT' },
  { table: 'search_term_stats', dimension: 'term', value: 'searches', dimType: 'TEXT' },
  { table: 'zoom_stats', dimension: 'zoom', value: 'requests', dimType: 'INTEGER' },
] as const

function createSchema(db: DatabaseSync): void {
  db.exec(`
    CREATE TABLE IF NOT EXISTS daily_stats (
      site TEXT NOT NULL, day TEXT NOT NULL,
      visitors INTEGER NOT NULL, requests INTEGER NOT NULL,
      bot_requests INTEGER NOT NULL, page_loads INTEGER NOT NULL,
      PRIMARY KEY (site, day)
    );
    CREATE TABLE IF NOT EXISTS popup_cell_stats (
      site TEXT NOT NULL, day TEXT NOT NULL,
      lat_cell REAL NOT NULL, lng_cell REAL NOT NULL, opens INTEGER NOT NULL,
      PRIMARY KEY (site, day, lat_cell, lng_cell)
    );
    CREATE TABLE IF NOT EXISTS visitor_sketches (
      site TEXT NOT NULL, day TEXT NOT NULL, sketch BLOB NOT NULL,
      PRIMARY KEY (site, day)
    );
  `)
  for (const { table, dimension, value, dimType } of DIMENSION_TABLES) {
    db.exec(`CREATE TABLE IF NOT EXISTS ${table} (
      site TEXT NOT NULL, day TEXT NOT NULL,
      ${dimension} ${dimType} NOT NULL, ${value} INTEGER NOT NULL,
      PRIMARY KEY (site, day, ${dimension})
    )`)
  }
}

function additiveUpsert(
  db: DatabaseSync,
  table: string,
  dimension: string,
  value: string,
) {
  return db.prepare(`INSERT INTO ${table} (site, day, ${dimension}, ${value})
    VALUES (?, ?, ?, ?)
    ON CONFLICT (site, day, ${dimension})
    DO UPDATE SET ${value} = ${value} + excluded.${value}`)
}

function writeDayToDatabase(
  db: DatabaseSync,
  site: string,
  day: string,
  accum: DayAccumulator,
): void {
  const stored = db
    .prepare('SELECT sketch FROM visitor_sketches WHERE site = ? AND day = ?')
    .get(site, day) as { sketch: Uint8Array } | undefined
  if (stored) accum.sketch.mergeFrom(stored.sketch)
  db.prepare(
    'INSERT OR REPLACE INTO visitor_sketches (site, day, sketch) VALUES (?, ?, ?)',
  ).run(site, day, accum.sketch.serialize())

  // Requests/bots/page loads are additive deltas; visitors is an absolute
  // estimate recomputed from the merged sketch (idempotent under replays).
  db.prepare(`INSERT INTO daily_stats (site, day, visitors, requests, bot_requests, page_loads)
    VALUES (?, ?, ?, ?, ?, ?)
    ON CONFLICT (site, day) DO UPDATE SET
      visitors = excluded.visitors,
      requests = requests + excluded.requests,
      bot_requests = bot_requests + excluded.bot_requests,
      page_loads = page_loads + excluded.page_loads`,
  ).run(site, day, accum.sketch.estimate(), accum.requests, accum.botRequests, accum.pageLoads)

  const dimensions: {
    spec: (typeof DIMENSION_TABLES)[number]
    entries: Iterable<[string | number, number]>
  }[] = [
    { spec: DIMENSION_TABLES[0], entries: accum.countries },
    { spec: DIMENSION_TABLES[1], entries: accum.browsers },
    { spec: DIMENSION_TABLES[2], entries: accum.oses },
    { spec: DIMENSION_TABLES[3], entries: accum.devices },
    { spec: DIMENSION_TABLES[4], entries: accum.languages },
    { spec: DIMENSION_TABLES[5], entries: accum.hours },
    { spec: DIMENSION_TABLES[6], entries: accum.referers },
    { spec: DIMENSION_TABLES[7], entries: accum.apiUsage },
    {
      spec: DIMENSION_TABLES[8],
      entries: [...accum.searchTerms].filter(([, n]) => n >= SEARCH_TERM_MIN_COUNT),
    },
    { spec: DIMENSION_TABLES[9], entries: accum.zooms },
  ]
  for (const { spec, entries } of dimensions) {
    const upsert = additiveUpsert(db, spec.table, spec.dimension, spec.value)
    for (const [dim, n] of entries) upsert.run(site, day, dim, n)
  }

  const upsertCell = db.prepare(`INSERT INTO popup_cell_stats
    (site, day, lat_cell, lng_cell, opens) VALUES (?, ?, ?, ?, ?)
    ON CONFLICT (site, day, lat_cell, lng_cell)
    DO UPDATE SET opens = opens + excluded.opens`)
  for (const [cellKey, opens] of accum.popupCells) {
    const [lat, lng] = cellKey.split(',').map(Number)
    upsertCell.run(site, day, lat, lng, opens)
  }
}

function topRows(
  db: DatabaseSync,
  table: string,
  dimension: string,
  value: string,
  site: string,
  day: string,
  limit = 10,
): { dim: string | number; n: number }[] {
  const rows = db
    .prepare(`SELECT ${dimension} AS dim, ${value} AS n FROM ${table}
      WHERE site = ? AND day = ? ORDER BY n DESC LIMIT ?`)
    .all(site, day, limit) as { dim: string | number; n: number }[]
  return rows
}

function formatTop(rows: { dim: string | number; n: number }[]): string {
  if (rows.length === 0) return '(none yet)'
  return rows.map(({ dim, n }) => `${dim} ${n}`).join(' · ')
}

function printAndSnapshotSite(db: DatabaseSync, site: string): object | null {
  const latest = db
    .prepare('SELECT day FROM daily_stats WHERE site = ? ORDER BY day DESC LIMIT 1')
    .get(site) as { day: string } | undefined
  if (!latest) return null
  const day = latest.day
  const daily = db
    .prepare('SELECT * FROM daily_stats WHERE site = ? AND day = ?')
    .get(site, day) as Record<string, number | string>

  const sections = Object.fromEntries(
    DIMENSION_TABLES.map(({ table, dimension, value }) => [
      table,
      topRows(db, table, dimension, value, site, day, table === 'zoom_stats' ? 100 : 10),
    ]),
  )
  const cells = db
    .prepare(`SELECT lat_cell, lng_cell, opens FROM popup_cell_stats
      WHERE site = ? AND day = ? ORDER BY opens DESC LIMIT 10`)
    .all(site, day) as { lat_cell: number; lng_cell: number; opens: number }[]

  console.log(`\nlatest day ${day} (${site}):`)
  console.log(
    `  visitors ${daily.visitors} · requests ${daily.requests} · bots ${daily.bot_requests} · page loads ${daily.page_loads}`,
  )
  console.log(`  top countries:  ${formatTop(sections.country_stats)}`)
  console.log(`  top browsers:   ${formatTop(sections.browser_stats)}`)
  console.log(`  os families:    ${formatTop(sections.os_stats)}`)
  console.log(`  device classes: ${formatTop(sections.device_stats)}`)
  console.log(`  top languages:  ${formatTop(sections.language_stats)}`)
  console.log(`  top referers:   ${formatTop(sections.referer_stats)}`)
  console.log(`  api usage:      ${formatTop(sections.api_usage_stats)}`)
  console.log(`  search terms (≥${SEARCH_TERM_MIN_COUNT}): ${formatTop(sections.search_term_stats)}`)
  console.log(`  zoom histogram: ${formatTop(sections.zoom_stats)}`)
  const cellText =
    cells.length === 0
      ? '(none yet)'
      : cells.map((c) => `${c.lat_cell.toFixed(2)},${c.lng_cell.toFixed(2)} ×${c.opens}`).join(' · ')
  console.log(`  top popup cells: ${cellText}`)

  const allRows = (table: string, dimension: string, value: string) =>
    Object.fromEntries(
      (db
        .prepare(`SELECT ${dimension} AS dim, ${value} AS n FROM ${table}
          WHERE site = ? AND day = ? ORDER BY n DESC`)
        .all(site, day) as { dim: string | number; n: number }[]).map((r) => [r.dim, r.n]),
    )
  return {
    day,
    visitors: daily.visitors,
    requests: daily.requests,
    bot_requests: daily.bot_requests,
    page_loads: daily.page_loads,
    countries: allRows('country_stats', 'country', 'requests'),
    browsers: allRows('browser_stats', 'browser', 'requests'),
    os_families: allRows('os_stats', 'os', 'requests'),
    device_classes: allRows('device_stats', 'device', 'requests'),
    languages: allRows('language_stats', 'language', 'requests'),
    hours: allRows('hour_stats', 'hour', 'requests'),
    referers: allRows('referer_stats', 'referer', 'visits'),
    api_usage: allRows('api_usage_stats', 'endpoint', 'requests'),
    search_terms: allRows('search_term_stats', 'term', 'searches'),
    zoom_histogram: allRows('zoom_stats', 'zoom', 'requests'),
    popup_cells: cells,
  }
}

function aggregateSitesWithDatabase(
  db: DatabaseSync,
  state: StatsState,
  geoipCountry: (ip: string) => string,
  siteList: string[],
): void {
  createSchema(db)

  const snapshots: Record<string, object> = {}
  for (const site of siteList) {
    const logPath = `/var/log/caddy/${site}.access.log`
    let fresh: NewLogBytes | null
    try {
      fresh = readNewLogBytes(logPath, state)
    } catch (error) {
      console.error(`${site}: cannot read ${logPath}: ${(error as Error).message.split('\n')[0]}`)
      continue
    }
    const days = new Map<string, DayAccumulator>()
    let lines = 0
    let aborted = 0
    let malformed = 0
    if (fresh) {
      for (const line of fresh.completeLines.toString('utf8').split('\n')) {
        if (!line) continue
        lines += 1
        let parsed: unknown
        try {
          parsed = JSON.parse(line)
        } catch {
          malformed += 1
          continue
        }
        const ts = (parsed as { ts?: number }).ts
        const dayKey =
          typeof ts === 'number' ? new Date(ts * 1000).toISOString().slice(0, 10) : 'unknown'
        let accum = days.get(dayKey)
        if (!accum) {
          accum = new DayAccumulator()
          days.set(dayKey, accum)
        }
        const outcome = aggregateAccessRecord(parsed, accum, geoipCountry)
        if (outcome === 'aborted') aborted += 1
        else if (outcome === 'malformed') malformed += 1
      }

      db.exec('BEGIN')
      try {
        for (const [day, accum] of [...days].sort(([a], [b]) => a.localeCompare(b))) {
          writeDayToDatabase(db, site, day, accum)
        }
        db.exec('COMMIT')
      } catch (error) {
        db.exec('ROLLBACK')
        throw error
      }
      state.files[logPath] = { inode: fresh.inode, offset: fresh.newOffset }
    }

    console.log(
      `\n== ${site}: ${lines} new lines (${aborted} client-aborted, ${malformed} malformed) ==`,
    )
    console.log('day          visitors  requests    bots  page-ld  popups  search  isochron  reverse')
    for (const [day, accum] of [...days].sort(([a], [b]) => a.localeCompare(b))) {
      const api = (name: string) => String(accum.apiUsage.get(name) ?? 0).padStart(8)
      console.log(
        `${day}  ${String(accum.sketch.estimate()).padStart(8)}  ${String(accum.requests).padStart(8)}` +
          `  ${String(accum.botRequests).padStart(7)}  ${String(accum.pageLoads).padStart(7)}` +
          `${api('popup_open')}${api('search')}${api('isochron')}${api('reverse')}`,
      )
    }
    if (days.size === 0) console.log('(no new complete lines — already up to date)')

    const snapshot = printAndSnapshotSite(db, site)
    if (snapshot) snapshots[site] = snapshot
  }

  // State moves forward only after the DB commit succeeded (see header).
  saveState(state)
  saveJsonAtomically(SNAPSHOT_PATH, {
    generated_at: new Date().toISOString(),
    sites: snapshots,
  })
  console.log(`\ndb ${DATABASE_PATH} · state ${STATE_PATH} · snapshot ${SNAPSHOT_PATH}`)
}

function aggregateSites(siteList: string[]): void {
  const geoipCountry = openGeoipCountryLookup()
  const state = loadState()
  const db = new DatabaseSync(DATABASE_PATH)
  try {
    aggregateSitesWithDatabase(db, state, geoipCountry, siteList)
  } finally {
    db.close()
  }
}

function main(): void {
  const sites = process.argv.slice(2)
  const siteList = sites.length > 0 ? sites : ['quietmap.org']
  for (const site of siteList) {
    if (!/^[a-z0-9.-]+$/i.test(site)) {
      console.error(`invalid site name '${site}' — refusing to build a log path from it`)
      process.exitCode = 1
      return
    }
  }

  mkdirSync(resolve(REPO_ROOT, 'data'), { recursive: true })
  const runLock = tryAcquireSqliteRunLock(RUN_LOCK_PATH)
  if (!runLock) {
    console.log('web-stats aggregation is already running; leaving logs and state untouched')
    return
  }
  try {
    aggregateSites(siteList)
  } finally {
    runLock.release()
  }
}

main()
