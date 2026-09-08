/** Authoritative dev1 global GTFS feed registry and source-layout contract. */

import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
import {
  GTFS_BORDER_MARGIN_DEG, METRO_TYPES, RAIL_TYPES, TRAM_TYPES, parseCsvStream,
  routeFamily,
} from './gtfs-enrich-core.js'
import type { PreparedBbox } from './prepared-grid.js'

const ALL_RAIL_AND_TRAM = new Set([...RAIL_TYPES, ...TRAM_TYPES, ...METRO_TYPES])

export interface GlobalGtfsFeed {
  id: string
  country: string
  name: string
  url: string
  bbox: PreparedBbox
  routeTypes: ReadonlySet<number>
  metroAsRail?: boolean
  requiredSubdirectories?: readonly string[]
}

export const GLOBAL_GTFS_FEEDS: readonly GlobalGtfsFeed[] = [
  { id: 'de', country: 'DE', name: 'Germany DELFI', url: 'https://data.public-transport.earth/gtfs/de', bbox: [47.2, 5.8, 55.1, 15.1], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'ch', country: 'CH', name: 'Switzerland OpenTransportData', url: 'https://data.public-transport.earth/gtfs/ch', bbox: [45.8, 5.9, 47.9, 10.5], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'at', country: 'AT', name: 'Austria ÖBB', url: 'https://data.oebb.at/de/datensaetze~soll-fahrplan-gtfs~', bbox: [46.3, 9.5, 49, 17.2], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'nl', country: 'NL', name: 'Netherlands', url: 'https://data.public-transport.earth/gtfs/nl', bbox: [50.7, 3.3, 53.6, 7.3], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'se', country: 'SE', name: 'Sweden', url: 'https://data.public-transport.earth/gtfs/se', bbox: [55.3, 10.9, 69.1, 24.2], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'no', country: 'NO', name: 'Norway', url: 'https://data.public-transport.earth/gtfs/no', bbox: [57.9, 4.5, 71.2, 31.2], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'fi', country: 'FI', name: 'Finland Digitraffic', url: 'https://rata.digitraffic.fi/api/v1/trains/gtfs-all.zip', bbox: [59.7, 19.1, 70.1, 31.6], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'be', country: 'BE', name: 'Belgium SNCB', url: 'https://gtfs.irail.be/nmbs/gtfs/latest.zip', bbox: [49.5, 2.5, 51.5, 6.4], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'in', country: 'IN', name: 'Indian Railways unofficial', url: 'https://github.com/Neo2308/indianrailways-gtfs/raw/refs/heads/main/gtfs/gtfs.zip', bbox: [6.7, 68.1, 35.7, 97.4], routeTypes: RAIL_TYPES },
  { id: 'us', country: 'US', name: 'Amtrak', url: 'https://content.amtrak.com/content/gtfs/GTFS.zip', bbox: [24.5, -125, 49, -66.9], routeTypes: RAIL_TYPES },
  { id: 'ca', country: 'CA', name: 'VIA Rail', url: 'https://www.viarail.ca/sites/all/files/gtfs/viarail.zip', bbox: [41.7, -141, 60, -52.6], routeTypes: RAIL_TYPES },
  { id: 'fr', country: 'FR', name: 'SNCF national', url: 'https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip', bbox: [41.3, -5.2, 51.1, 9.6], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'lu', country: 'LU', name: 'Luxembourg ATP open data', url: 'https://data.public.lu/en/datasets/horaires-et-arrets-des-transport-publics-gtfs/', bbox: [49.4, 5.7, 50.2, 6.6], routeTypes: ALL_RAIL_AND_TRAM },
  {
    id: 'gr',
    country: 'GR',
    name: 'Greece TrainOSE archive',
    url: 'https://files.mobilitydatabase.org/mdb-1161/latest.zip',
    bbox: [34.8, 19.3, 41.8, 29.8],
    routeTypes: ALL_RAIL_AND_TRAM,
  },
  { id: 'lv-pv', country: 'LV', name: 'Latvia Vivi', url: 'https://www.vivi.lv/uploads/GTFS.zip', bbox: [55.6, 20.9, 58.1, 28.2], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'ee', country: 'EE', name: 'Estonia Peatus', url: 'https://files.mobilitydatabase.org/mdb-1095/latest.zip', bbox: [57.5, 21.8, 59.7, 28.2], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'bg-sofia', country: 'BG', name: 'Sofia Traffic', url: 'https://gtfs.sofiatraffic.bg/api/v1/static', bbox: [42.5, 23.1, 42.8, 23.6], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'hr', country: 'HR', name: 'Croatia HŽ', url: 'https://www.hzpp.hr/GTFS_files.zip', bbox: [42.3, 13.4, 46.6, 19.5], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'hu', country: 'HU', name: 'Hungary MÁV', url: 'https://gtfs.menetbrand.com/download/mav/', bbox: [45.7, 16.1, 48.6, 22.9], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'sk', country: 'SK', name: 'Slovakia ŽSR', url: 'https://data.slovensko.sk/download?id=f63ef0f2-c4e7-496b-bdd1-44ba0e9438e9', bbox: [47.7, 16.8, 49.6, 22.6], routeTypes: ALL_RAIL_AND_TRAM },
  { id: 'fr-idf', country: 'FR', name: 'Île-de-France tram only', url: 'https://eu.ftp.opendatasoft.com/sncf/gtfs/transilien-gtfs.zip', bbox: [48.1, 1.4, 49.2, 3.6], routeTypes: TRAM_TYPES },
  { id: 'au-vic', country: 'AU', name: 'Victoria PTV', url: 'https://data.ptv.vic.gov.au/downloads/gtfs.zip', bbox: [-39.2, 140.9, -33.9, 150], routeTypes: new Set([...RAIL_TYPES, ...METRO_TYPES]), metroAsRail: true, requiredSubdirectories: ['1', '2', '10'] },
  { id: 'au-qld', country: 'AU', name: 'Queensland TransLink', url: 'https://gtfsrt.api.translink.com.au/GTFS/SEQ_GTFS.zip', bbox: [-29.2, 150.5, -26, 153.6], routeTypes: RAIL_TYPES },
]

const REQUIRED_FILES = ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt'] as const
const hasRequiredFiles = (directory: string): boolean =>
  REQUIRED_FILES.every(name => existsSync(resolve(directory, name)))

function resolvedGtfsDirectory(directory: string): string | null {
  if (hasRequiredFiles(directory)) return directory
  const extracted = resolve(directory, 'extracted')
  return hasRequiredFiles(extracted) ? extracted : null
}

export function railFamilyFor(
  routeType: number,
  feed: GlobalGtfsFeed,
): 'rail' | 'tram' | null {
  if (!feed.routeTypes.has(routeType)) return null
  if (feed.metroAsRail && METRO_TYPES.has(routeType)) return 'rail'
  return routeFamily(routeType)
}

/** Resolve archived flat or Victoria mode-split source directories without writing them. */
export function gtfsSourceDirectories(sourceRoot: string, feed: GlobalGtfsFeed): string[] {
  const directory = resolve(sourceRoot, feed.id)
  if (!feed.requiredSubdirectories) {
    const resolvedDirectory = resolvedGtfsDirectory(directory)
    return resolvedDirectory ? [resolvedDirectory] : []
  }
  const found = feed.requiredSubdirectories.map(name =>
    resolvedGtfsDirectory(resolve(directory, name)))
  return found.every((candidate): candidate is string => candidate !== null) ? found : []
}

export function countryGtfsBbox(country: string): PreparedBbox {
  const feeds = GLOBAL_GTFS_FEEDS.filter(feed => feed.country === country)
  if (feeds.length === 0) throw new Error(`no global GTFS feed for country '${country}'`)
  const margin = GTFS_BORDER_MARGIN_DEG
  return [
    Math.max(-90, Math.min(...feeds.map(feed => feed.bbox[0])) - margin),
    Math.max(-180, Math.min(...feeds.map(feed => feed.bbox[1])) - margin),
    Math.min(90, Math.max(...feeds.map(feed => feed.bbox[2])) + margin),
    Math.min(180, Math.max(...feeds.map(feed => feed.bbox[3])) + margin),
  ]
}

export interface GtfsSourceFreshness {
  lastServiceDate: string
}

const validGtfsDate = (value: string): boolean => /^\d{8}$/.test(value)

function latestDate(
  rows: readonly Record<string, string>[],
  column: string,
  current = '',
  include: (row: Readonly<Record<string, string>>) => boolean = () => true,
): string {
  let latest = current
  for (const row of rows) {
    if (!include(row)) continue
    const value = row[column] || ''
    if (validGtfsDate(value) && value > latest) latest = value
  }
  return latest
}

/**
 * Validate the feed-declared service horizon, preferring authoritative
 * feed_info dates over broad recurring calendars.
 */
export async function validateGtfsSourceFreshness(
  feed: GlobalGtfsFeed,
  directory: string,
  asOfDate: string,
): Promise<GtfsSourceFreshness> {
  if (!validGtfsDate(asOfDate)) throw new Error(`invalid GTFS as-of date '${asOfDate}'`)
  const feedInfoPath = resolve(directory, 'feed_info.txt')
  let lastServiceDate = ''
  if (existsSync(feedInfoPath)) {
    lastServiceDate = latestDate(await parseCsvStream(feedInfoPath), 'feed_end_date')
  }
  if (!lastServiceDate) {
    const calendarPath = resolve(directory, 'calendar.txt')
    const calendarDatesPath = resolve(directory, 'calendar_dates.txt')
    if (existsSync(calendarPath)) {
      lastServiceDate = latestDate(
        await parseCsvStream(calendarPath),
        'end_date',
        lastServiceDate,
      )
    }
    if (existsSync(calendarDatesPath)) {
      lastServiceDate = latestDate(
        await parseCsvStream(calendarDatesPath),
        'date',
        lastServiceDate,
        row => row['exception_type'] === '1',
      )
    }
  }
  if (!lastServiceDate) {
    throw new Error(`GTFS feed ${feed.id} has no service-validity date in ${directory}`)
  }
  if (lastServiceDate < asOfDate) {
    throw new Error(
      `GTFS feed ${feed.id} expired ${lastServiceDate}; refresh it before ${asOfDate} enrichment`,
    )
  }
  return { lastServiceDate }
}
