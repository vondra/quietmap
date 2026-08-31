// Routes for the /a/stats web-analytics dashboard. Registered inside the /a admin scope (Caddy
// basic_auth + requireLocalPeer), so it inherits both access layers.
// `compress:false` mirrors cluster.ts: @fastify/compress streaming hit a
// "premature close" that delivered EMPTY bodies to gzip clients.
import type { FastifyInstance } from 'fastify'
import {
  emptyStatsSummary,
  openStatsDb,
  readStatsSummary,
  WEB_STATS_SITE,
  type StatsSummary,
} from './web-stats-data.js'
import { liveScan, windowScan, type CountrySlice } from './web-stats-live.js'
import { buildInsights, type InsightInputs } from './web-stats-insights.js'
import { COUNTRY_NAME } from './web-stats-land-grid.js'
import { statsPage } from './web-stats-page.js'

const EMPTY_SLICE: CountrySlice = {
  visitors: 0,
  requests: 0,
  pageLoads: 0,
  popupOpens: 0,
  searches: 0,
  hours: Array(24).fill(0) as number[],
  referers: {},
  devices: {},
  devicePopups: {},
  deviceSearches: {},
  cells: [],
}

function readSummarySafe(day?: string): StatsSummary {
  const db = openStatsDb()
  if (!db) return emptyStatsSummary(WEB_STATS_SITE)
  try {
    return readStatsSummary(db, WEB_STATS_SITE, day)
  } catch {
    return emptyStatsSummary(WEB_STATS_SITE)
  } finally {
    db.close()
  }
}

export async function webStatsRoutes(app: FastifyInstance) {
  app.get('/stats', { compress: false }, async (_req, reply) => {
    return reply.type('text/html; charset=utf-8').send(statsPage())
  })

  app.get('/api/stats/summary', { compress: false }, async (req, reply) => {
    const query = req.query as { day?: string; country?: string }
    const day = query.day && /^\d{4}-\d{2}-\d{2}$/.test(query.day) ? query.day : undefined
    const country = query.country && /^[A-Z]{2}$/.test(query.country) ? query.country : null

    const summary = readSummarySafe(day)
    // One window scan serves BOTH the country slice and the insight device
    // rates; it is memoized for 60 s, so the 60 s poller shares one sudo read.
    const window = await windowScan()
    const insightInputs: InsightInputs = {
      today: summary.day,
      previousDay: summary.previousDay,
      selfDomain: WEB_STATS_SITE,
      countryFirstSeen: summary.countryFirstSeen,
      countryNames: COUNTRY_NAME,
      referersToday: summary.referers.map((r) => ({ domain: r.domain, visits: r.visits })),
      referersPrevious: summary.referersPrevious,
      refererFirstSeen: summary.refererFirstSeen,
      topPopupCell: summary.popupCells[0] ?? null,
      deviceRates: window.deviceRates,
    }
    return reply.send({
      ...summary,
      filter: { country },
      // Slice semantics differ from the DB aggregates (live window, today so
      // far, distinct-IP counts) — the page labels it as such wherever shown.
      slice: country ? (window.byCountry[country] ?? EMPTY_SLICE) : null,
      window: { ok: window.ok, fromTs: window.fromTs, toTs: window.toTs, lines: window.lines },
      insights: buildInsights(insightInputs),
    })
  })

  app.get('/api/stats/live', { compress: false }, async (_req, reply) => {
    const scan = await liveScan()
    return reply.send({
      ok: scan.ok,
      error: scan.error ?? null,
      onlineNow: scan.onlineNow,
      events: scan.events,
      generatedAt: new Date().toISOString(),
    })
  })
}
