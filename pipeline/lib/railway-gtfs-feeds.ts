/** Immutable GTFS source registries and source-validity contract. */

import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { resolve } from 'node:path'
import { path7za } from '7zip-bin'
import {
  GTFS_BORDER_MARGIN_DEG, METRO_TYPES, RAIL_TYPES, TRAM_TYPES, parseCsvStream,
  routeFamily, readGtfsFeedWindow,
} from './gtfs-enrich-core.js'
import {
  SOURCE_ID_AE_NATIONAL_RAILWAY, SOURCE_ID_AR_NATIONAL_RAILWAY, SOURCE_ID_AU_NATIONAL_RAILWAY,
  SOURCE_ID_BE_NATIONAL_RAILWAY, SOURCE_ID_CA_NATIONAL_RAILWAY,
  SOURCE_ID_DE_NATIONAL_RAILWAY, SOURCE_ID_DK_NATIONAL_RAILWAY,
  SOURCE_ID_ES_NATIONAL_RAILWAY, SOURCE_ID_FI_NATIONAL_RAILWAY,
  SOURCE_ID_IE_NATIONAL_RAILWAY, SOURCE_ID_IL_NATIONAL_RAILWAY,
  SOURCE_ID_IT_NATIONAL_RAILWAY, SOURCE_ID_MX_NATIONAL_RAILWAY,
  SOURCE_ID_PL_NATIONAL_RAILWAY, SOURCE_ID_PT_NATIONAL_RAILWAY,
  SOURCE_ID_SE_NATIONAL_RAILWAY, SOURCE_ID_TH_NATIONAL_RAILWAY,
  SOURCE_ID_GLOBAL_GTFS_TRANSIT,
} from './source-ids.generated.js'
import type { PreparedBbox } from './prepared-grid.js'

const ALL_RAIL_AND_TRAM = new Set([...RAIL_TYPES, ...TRAM_TYPES, ...METRO_TYPES])
const RAIL_AND_TRAM_WITHOUT_METRO = new Set([...RAIL_TYPES, ...TRAM_TYPES])

export type GtfsRegistry = 'global' | 'national'

export interface GlobalGtfsFeed {
  id: string
  country: string
  name: string
  url: string
  downloadUrls?: readonly string[]
  bbox: PreparedBbox
  routeTypes: ReadonlySet<number>
  sourceId: number
  sourcePath?: string
  metroAsRail?: boolean
  includeRailPairs?: boolean
  requiredSubdirectories?: readonly string[]
  singleNestedDirectory?: boolean
  serviceDay: 'busiest-wednesday' | 'midpoint-wednesday'
  acceptedHistoricalSource?: { gtfsTextSha256: string; lastServiceDate: string; sourceYear: number }
  sourceArchive?: { relativePath: string; sha256: string }
}

const globalFeed = (
  feed: Omit<GlobalGtfsFeed, 'sourceId' | 'serviceDay' | 'downloadUrls'>,
): GlobalGtfsFeed => ({
  ...feed, downloadUrls: [feed.url], sourceId: SOURCE_ID_GLOBAL_GTFS_TRANSIT,
  serviceDay: 'busiest-wednesday',
})

export const GLOBAL_GTFS_FEEDS: readonly GlobalGtfsFeed[] = [
  globalFeed({ id: 'de', country: 'DE', name: 'Germany DELFI', url: 'https://data.public-transport.earth/gtfs/de', bbox: [47.2, 5.8, 55.1, 15.1], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'ch', country: 'CH', name: 'Switzerland OpenTransportData', url: 'https://data.public-transport.earth/gtfs/ch', bbox: [45.8, 5.9, 47.9, 10.5], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'at', country: 'AT', name: 'Austria ÖBB', url: 'https://data.oebb.at/de/datensaetze~soll-fahrplan-gtfs~', bbox: [46.3, 9.5, 49, 17.2], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'nl', country: 'NL', name: 'Netherlands', url: 'https://data.public-transport.earth/gtfs/nl', bbox: [50.7, 3.3, 53.6, 7.3], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'se', country: 'SE', name: 'Sweden', url: 'https://data.public-transport.earth/gtfs/se', bbox: [55.3, 10.9, 69.1, 24.2], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'no', country: 'NO', name: 'Norway', url: 'https://data.public-transport.earth/gtfs/no', bbox: [57.9, 4.5, 71.2, 31.2], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'fi', country: 'FI', name: 'Finland Digitraffic', url: 'https://rata.digitraffic.fi/api/v1/trains/gtfs-all.zip', bbox: [59.7, 19.1, 70.1, 31.6], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'be', country: 'BE', name: 'Belgium SNCB', url: 'https://gtfs.irail.be/nmbs/gtfs/latest.zip', bbox: [49.5, 2.5, 51.5, 6.4], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'in', country: 'IN', name: 'Indian Railways unofficial', url: 'https://github.com/Neo2308/indianrailways-gtfs/raw/refs/heads/main/gtfs/gtfs.zip', bbox: [6.7, 68.1, 35.7, 97.4], routeTypes: RAIL_TYPES }),
  globalFeed({ id: 'us', country: 'US', name: 'Amtrak', url: 'https://content.amtrak.com/content/gtfs/GTFS.zip', bbox: [24.5, -125, 49, -66.9], routeTypes: RAIL_TYPES }),
  globalFeed({ id: 'ca', country: 'CA', name: 'VIA Rail', url: 'https://www.viarail.ca/sites/all/files/gtfs/viarail.zip', bbox: [41.7, -141, 60, -52.6], routeTypes: RAIL_TYPES }),
  globalFeed({ id: 'fr', country: 'FR', name: 'SNCF national', url: 'https://eu.ftp.opendatasoft.com/sncf/plandata/Export_OpenData_SNCF_GTFS_NewTripId.zip', bbox: [41.3, -5.2, 51.1, 9.6], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'lu', country: 'LU', name: 'Luxembourg ATP open data', url: 'https://data.public.lu/en/datasets/horaires-et-arrets-des-transport-publics-gtfs/', bbox: [49.4, 5.7, 50.2, 6.6], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'gr', country: 'GR', name: 'Hellenic Train community timetable', url: 'https://jbb.ghsq.de/gtfs/gr-hellenic-train.gtfs.zip', bbox: [34.8, 19.3, 41.8, 29.8], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'lv-pv', country: 'LV', name: 'Latvia Vivi', url: 'https://www.vivi.lv/uploads/GTFS.zip', bbox: [55.6, 20.9, 58.1, 28.2], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'ee', country: 'EE', name: 'Estonia Peatus', url: 'https://files.mobilitydatabase.org/mdb-1095/latest.zip', bbox: [57.5, 21.8, 59.7, 28.2], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'bg-sofia', country: 'BG', name: 'Sofia Traffic', url: 'https://gtfs.sofiatraffic.bg/api/v1/static', bbox: [42.5, 23.1, 42.8, 23.6], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'hr', country: 'HR', name: 'Croatia HŽ', url: 'https://www.hzpp.hr/GTFS_files.zip', bbox: [42.3, 13.4, 46.6, 19.5], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'hu', country: 'HU', name: 'Hungary MÁV', url: 'https://gtfs.menetbrand.com/download/mav/', bbox: [45.7, 16.1, 48.6, 22.9], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'sk', country: 'SK', name: 'Slovakia ŽSR', url: 'https://data.slovensko.sk/download?id=f63ef0f2-c4e7-496b-bdd1-44ba0e9438e9', bbox: [47.7, 16.8, 49.6, 22.6], routeTypes: ALL_RAIL_AND_TRAM }),
  globalFeed({ id: 'au-vic', country: 'AU', name: 'Victoria PTV', url: 'https://data.ptv.vic.gov.au/downloads/gtfs.zip', bbox: [-39.2, 140.9, -33.9, 150], routeTypes: new Set([...RAIL_TYPES, ...METRO_TYPES]), metroAsRail: true, requiredSubdirectories: ['1', '2', '10'] }),
  globalFeed({ id: 'au-qld', country: 'AU', name: 'Queensland TransLink', url: 'https://gtfsrt.api.translink.com.au/GTFS/SEQ_GTFS.zip', bbox: [-29.2, 150.5, -26, 153.6], routeTypes: RAIL_TYPES }),
]

const NATIONAL_GTFS_DOWNLOAD_URLS: Readonly<Record<string, readonly string[]>> = {
  'rta-dubai': ['https://www.dubaipulse.gov.ae/dataset/73765e8f-e8c4-443c-9687-288072ed9d12/resource/11515bd3-bdba-466f-ab65-f057bd123ab5/download/gtfs.7z'],
  'subte-buenos-aires': ['https://buenosaires.gob.ar/sites/gcaba/files/subte_gtfs.zip'],
  'nsw-sydney': ['https://opendata.transport.nsw.gov.au/data/dataset/d1f68d4f-b778-44df-9823-cf2fa922e47f/resource/67974f14-01bf-47b7-bfa5-c7f2f8a950ca/download/full_greater_sydney_gtfs_static_0.zip'],
  'adelaide-metro': ['https://gtfs.adelaidemetro.com.au/v1/static/latest/google_transit.zip'],
  'transperth-wa': [
    'https://www.transperth.wa.gov.au/TimetablePDFs/GoogleTransit/Production/google_transit.zip',
    'https://files.mobilitydatabase.org/mdb-1086/latest.zip',
  ],
  'stib-brussels': [
    'https://gtfs.flatturtle.cloud/stib-mivb/_latest/stib-mivb-gtfs.zip',
    'https://stibmivb.opendatasoft.com/api/datasets/1.0/gtfs-files-production/alternative_exports/gtfszip/',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/be-bruxelles-capitale-societe-des-transports-intercommunaux-de-bruxellesmaatschappij-voor-het-intercommunaal-vervoer-te-brussel-stibmivb-gtfs-1088.zip?alt=media',
  ],
  'delijn-flanders': [
    'http://gtfs.irail.be/de-lijn/de_lijn-gtfs.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/be-vlaams-gewest-de-lijn-gtfs-684.zip?alt=media',
  ],
  'tec-wallonia': [
    'http://opendata.tec-wl.be/Current%20GTFS/TEC-GTFS.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/be-unknown-societe-regionale-wallonne-du-transport-gtfs-1212.zip?alt=media',
  ],
  'via-rail': [
    'https://www.viarail.ca/sites/all/files/gtfs/viarail.zip',
    'https://files.mobilitydatabase.org/mdb-735/latest.zip',
  ],
  'go-transit': ['https://assets.metrolinx.com/raw/upload/Documents/Metrolinx/Open%20Data/GO-GTFS.zip'],
  'ttc-toronto': ['http://opendata.toronto.ca/toronto.transit.commission/ttc-routes-and-schedules/OpenData_TTC_Schedules.zip'],
  'stm-montreal': [
    'http://www.stm.info/sites/default/files/gtfs/gtfs_stm.zip',
    'https://files.mobilitydatabase.org/mdb-2126/latest.zip',
  ],
  'translink-vancouver': ['https://gtfs-static.translink.ca/gtfs/google_transit.zip'],
  'oc-transpo': ['https://oct-gtfs-emasagcnfmcgeham.z01.azurefd.net/public-access/GTFSExport.zip'],
  'calgary-transit': ['https://data.calgary.ca/download/npk7-z3bj/application%2Fzip'],
  'edmonton-ets': ['https://gtfs.edmonton.ca/TMGTFSRealTimeWebService/GTFS/GTFS.zip'],
  'de-full': ['https://download.gtfs.de/germany/free/latest.zip'],
  'rejseplanen': ['https://www.rejseplanen.info/labs/GTFS.zip'],
  'renfe-av-ld-md': [
    'https://ssl.renfe.com/gtransit/Fichero_AV_LD/google_transit.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/es-renfe-alta-velocidad-larga-distancia-media-distancia-gtfs-2620.zip?alt=media',
  ],
  'renfe-cercanias': [
    'https://ssl.renfe.com/ftransit/Fichero_CER_FOMENTO/fomento_transit.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/es-unknown-renfe-cercanias-gtfs-2653.zip?alt=media',
  ],
  'fgc-catalunya': [
    'https://www.fgc.cat/google/google_transit.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/es-catalunya-ferrocarrils-de-la-generalitat-de-catalunya-gtfs-1856.zip?alt=media',
  ],
  'fintraffic-vr': ['https://rata.digitraffic.fi/api/v1/trains/gtfs-passenger.zip'],
  'hsl-helsinki': ['https://infopalvelut.storage.hsldev.com/gtfs/hsl.zip'],
  'tampere': ['http://data.itsfactory.fi/journeys/files/gtfs/latest/gtfs_tampere.zip'],
  'foli-turku': ['http://data.foli.fi/gtfs/gtfs.zip'],
  'nta-unified': ['https://www.transportforireland.ie/transitData/Data/GTFS_All.zip'],
  'mot-unified': [
    'https://gtfs.mot.gov.il/gtfsfiles/israel-public-transportation.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/il-ministry-of-transport-and-road-safety-gtfs-2519.zip?alt=media',
  ],
  'toscana-trenitalia': [
    'https://dati.toscana.it/dataset/8bb8f8fe-fe7d-41d0-90dc-49f2456180d1/resource/4f85393b-357d-443d-8378-65de4198505f/download/trenitalia.gtfs',
  ],
  'trenord-lombardia': [
    'https://www.dati.lombardia.it/download/3z4k-mxz9/application%2Fzip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/it-lombardia-trenord-gtfs-855.zip?alt=media',
  ],
  'gtt-piemonte': [
    'https://www.gtt.to.it/open_data/gtt_gtfs.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/it-piedmont-turin-gruppo-torinese-trasporti-gtfs-2687.zip?alt=media',
  ],
  'ferrotramviaria-puglia': [
    'http://dati.mit.gov.it/catalog/dataset/1b5195b2-8c2e-409a-b104-78dd5f5e057f/resource/ba72e7dd-d68e-4503-b08f-66f5a707a4d5/download/ferrovienordbarese.gtfs',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/it-puglia-ferrotramviaria-gtfs-1058.zip?alt=media',
  ],
  'trenitalia-sardegna': [
    'https://www.sardegnamobilita.it/opendata/R_SARDEGTRASP_00008_1_dati_trenitalia.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/it-regione-autonoma-della-sardegna-trenitalia-gtfs-2997.zip?alt=media',
  ],
  'cdmx-semovi': [
    'https://datos.cdmx.gob.mx/dataset/75538d96-3ade-4bc5-ae7d-d85595e4522d/resource/32ed1b6b-41cd-49b3-b7f0-b57acb0eb819/download/gtfs-2.zip',
    'https://files.mobilitydatabase.org/mdb-1830/latest.zip',
  ],
  'toluca-movimex': ['https://datos.movimex.gob.mx/gtfs/toluca.gtfs.zip'],
  'polish-trains': ['https://mkuran.pl/gtfs/polish_trains.zip'],
  'warsaw-ztm': ['https://mkuran.pl/gtfs/warsaw.zip'],
  'krakow-tram': ['https://gtfs.ztp.krakow.pl/GTFS_KRK_T.zip'],
  'silesia-gzm': ['https://mkuran.pl/gtfs/gzm.zip'],
  'wkd-warszawa': ['https://mkuran.pl/gtfs/wkd.zip'],
  'cp-comboios': ['https://publico.cp.pt/gtfs/gtfs.zip'],
  'metro-porto': [
    'https://www.metrodoporto.pt/metrodoporto/uploads/document/file/794/google_transit_04_09_2026.zip',
    'https://storage.googleapis.com/storage/v1/b/mdb-latest/o/pt-porto-metro-do-porto-gtfs-2357.zip?alt=media',
  ],
  'metro-sul-tejo': ['https://mts.pt/imt/MTS-20240129.zip'],
  'carris-metropolitana': ['https://api.carrismetropolitana.pt/v2/gtfs'],
  'gtfs-sverige-2': ['https://api.resrobot.se/gtfs/sweden.zip'],
  'namtang': ['https://namtang-api.otp.go.th/download/namtang-gtfs.zip'],
}

const nationalFeed = (
  country: string, sourceId: number, bbox: PreparedBbox, id: string, name: string,
  options: Partial<Pick<GlobalGtfsFeed, 'routeTypes' | 'includeRailPairs' | 'singleNestedDirectory' | 'acceptedHistoricalSource' | 'sourceArchive'>> = {},
): GlobalGtfsFeed => ({
  id, country, sourceId, bbox, name,
  url: NATIONAL_GTFS_DOWNLOAD_URLS[id]?.[0] ?? '',
  downloadUrls: NATIONAL_GTFS_DOWNLOAD_URLS[id],
  sourcePath: `${country.toLowerCase()}/gtfs-${id}`,
  routeTypes: ALL_RAIL_AND_TRAM, serviceDay: 'midpoint-wednesday', ...options,
})

/** Dev1 national timetable coverage, expressed as data over the shared parser/writer. */
export const NATIONAL_GTFS_FEEDS: readonly GlobalGtfsFeed[] = [
  nationalFeed('AE', SOURCE_ID_AE_NATIONAL_RAILWAY, [22.3, 51, 26.3, 56.7], 'rta-dubai', 'Dubai RTA 2025 archive', { routeTypes: RAIL_AND_TRAM_WITHOUT_METRO, singleNestedDirectory: true, acceptedHistoricalSource: { gtfsTextSha256: 'eda30cc37cd7de95b4c6c03bddadb54c7c40e45c22b2cff12444fa83550a25cf', lastServiceDate: '20251231', sourceYear: 2025 } }),
  nationalFeed('AR', SOURCE_ID_AR_NATIONAL_RAILWAY, [-35, -59, -34, -58], 'subte-buenos-aires', 'Buenos Aires Subte 2019 archive', { sourceArchive: { relativePath: 'ar/subte-gtfs/subte_gtfs', sha256: '8b3f5f4e6583288fc95f59b2f673c158db63f2230fd758ed6ebe75e63305bd68' }, acceptedHistoricalSource: { gtfsTextSha256: '145c3999717cdd9783e5f85f737beffe80e587c34a1d12c606db23d1157c57a8', lastServiceDate: '20191231', sourceYear: 2019 } }),
  nationalFeed('AU', SOURCE_ID_AU_NATIONAL_RAILWAY, [-44, 113, -10, 154], 'nsw-sydney', 'NSW Greater Sydney'),
  nationalFeed('AU', SOURCE_ID_AU_NATIONAL_RAILWAY, [-44, 113, -10, 154], 'adelaide-metro', 'Adelaide Metro'),
  nationalFeed('AU', SOURCE_ID_AU_NATIONAL_RAILWAY, [-44, 113, -10, 154], 'transperth-wa', 'Transperth WA'),
  nationalFeed('BE', SOURCE_ID_BE_NATIONAL_RAILWAY, [49.4, 2.4, 51.6, 6.5], 'stib-brussels', 'STIB/MIVB Brussels'),
  nationalFeed('BE', SOURCE_ID_BE_NATIONAL_RAILWAY, [49.4, 2.4, 51.6, 6.5], 'delijn-flanders', 'De Lijn Flanders'),
  nationalFeed('BE', SOURCE_ID_BE_NATIONAL_RAILWAY, [49.4, 2.4, 51.6, 6.5], 'tec-wallonia', 'TEC Wallonia'),
  ...['via-rail', 'go-transit', 'ttc-toronto', 'stm-montreal', 'translink-vancouver', 'oc-transpo', 'calgary-transit', 'edmonton-ets'].map(id => nationalFeed('CA', SOURCE_ID_CA_NATIONAL_RAILWAY, [41.5, -141, 84, -52], id, id)),
  nationalFeed('DE', SOURCE_ID_DE_NATIONAL_RAILWAY, [47.3, 5.9, 55.1, 15], 'de-full', 'DELFI national'),
  nationalFeed('DK', SOURCE_ID_DK_NATIONAL_RAILWAY, [54.5, 8, 57.8, 13], 'rejseplanen', 'Rejseplanen unified'),
  ...['renfe-av-ld-md', 'renfe-cercanias', 'fgc-catalunya'].map(id => nationalFeed('ES', SOURCE_ID_ES_NATIONAL_RAILWAY, [35.5, -10, 44, 5], id, id)),
  ...['fintraffic-vr', 'hsl-helsinki', 'tampere', 'foli-turku'].map(id => nationalFeed('FI', SOURCE_ID_FI_NATIONAL_RAILWAY, [59.7, 19.1, 70.1, 31.6], id, id)),
  nationalFeed('IE', SOURCE_ID_IE_NATIONAL_RAILWAY, [51.4, -10.5, 55.4, -5.4], 'nta-unified', 'NTA Transport for Ireland'),
  nationalFeed('IL', SOURCE_ID_IL_NATIONAL_RAILWAY, [29.5, 34.2, 33.4, 35.9], 'mot-unified', 'Israel MoT unified'),
  ...['toscana-trenitalia', 'trenord-lombardia', 'gtt-piemonte'].map(id => nationalFeed('IT', SOURCE_ID_IT_NATIONAL_RAILWAY, [35.5, 6.6, 47.1, 18.6], id, id)),
  nationalFeed('IT', SOURCE_ID_IT_NATIONAL_RAILWAY, [35.5, 6.6, 47.1, 18.6], 'ferrotramviaria-puglia', 'Ferrotramviaria 2025 archive', { acceptedHistoricalSource: { gtfsTextSha256: 'cb24dbf16f8f5f41d50b17f575d064d6713efc7c41fa867e3e8d71b834fc2c6b', lastServiceDate: '20251231', sourceYear: 2025 } }),
  nationalFeed('IT', SOURCE_ID_IT_NATIONAL_RAILWAY, [35.5, 6.6, 47.1, 18.6], 'trenitalia-sardegna', 'Trenitalia Sardegna'),
  ...['cdmx-semovi', 'toluca-movimex'].map(id => nationalFeed('MX', SOURCE_ID_MX_NATIONAL_RAILWAY, [14.5, -118.4, 32.7, -86.7], id, id)),
  nationalFeed('PL', SOURCE_ID_PL_NATIONAL_RAILWAY, [49, 14, 55, 24.5], 'polish-trains', 'Polish Trains'),
  nationalFeed('PL', SOURCE_ID_PL_NATIONAL_RAILWAY, [49, 14, 55, 24.5], 'warsaw-ztm', 'Warsaw ZTM', { includeRailPairs: false }),
  ...['krakow-tram', 'silesia-gzm', 'wkd-warszawa'].map(id => nationalFeed('PL', SOURCE_ID_PL_NATIONAL_RAILWAY, [49, 14, 55, 24.5], id, id)),
  ...['cp-comboios', 'metro-porto', 'metro-sul-tejo', 'carris-metropolitana'].map(id => nationalFeed('PT', SOURCE_ID_PT_NATIONAL_RAILWAY, [36.5, -10, 42.5, -6], id, id)),
  nationalFeed('SE', SOURCE_ID_SE_NATIONAL_RAILWAY, [55.3, 10.9, 69.1, 24.2], 'gtfs-sverige-2', 'GTFS Sverige 2'),
  nationalFeed('TH', SOURCE_ID_TH_NATIONAL_RAILWAY, [5.5, 97.3, 20.5, 105.7], 'namtang', 'Namtang Thailand', { routeTypes: RAIL_AND_TRAM_WITHOUT_METRO }),
]

export function gtfsDownloadUrls(feed: GlobalGtfsFeed): readonly string[] {
  const urls = feed.downloadUrls ?? (feed.url ? [feed.url] : [])
  if (urls.length === 0) throw new Error(`GTFS feed ${feed.id} has no download URL`)
  return urls
}

export function feedsForRegistry(registry: GtfsRegistry): readonly GlobalGtfsFeed[] {
  return registry === 'global' ? GLOBAL_GTFS_FEEDS : NATIONAL_GTFS_FEEDS
}

const REQUIRED_FILES = ['stops.txt', 'stop_times.txt', 'trips.txt', 'routes.txt'] as const
const hasRequiredFiles = (directory: string): boolean =>
  REQUIRED_FILES.every(name => existsSync(resolve(directory, name)))

function resolvedGtfsDirectory(directory: string): string | null {
  if (hasRequiredFiles(directory)) return directory
  const extracted = resolve(directory, 'extracted')
  return hasRequiredFiles(extracted) ? extracted : null
}

function directoryGtfsSources(directory: string, feed: GlobalGtfsFeed): string[] {
  if (feed.singleNestedDirectory) {
    const direct = resolvedGtfsDirectory(directory)
    if (direct) return [direct]
    if (!existsSync(directory)) return []
    const found = readdirSync(directory, { withFileTypes: true })
      .filter(entry => entry.isDirectory())
      .map(entry => resolvedGtfsDirectory(resolve(directory, entry.name)))
      .filter((candidate): candidate is string => candidate !== null)
    return found.length === 1 ? found : []
  }
  if (!feed.requiredSubdirectories) {
    const direct = resolvedGtfsDirectory(directory)
    return direct ? [direct] : []
  }
  const found = feed.requiredSubdirectories.map(name => resolvedGtfsDirectory(resolve(directory, name)))
  return found.every((candidate): candidate is string => candidate !== null) ? found : []
}

function findGtfsRoots(directory: string, depth = 0): string[] {
  const roots = hasRequiredFiles(directory) ? [directory] : []
  if (depth >= 4) return roots
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) roots.push(...findGtfsRoots(resolve(directory, entry.name), depth + 1))
  }
  return roots
}

function extractedArchiveSource(sourceRoot: string, cacheDirectory: string, feed: GlobalGtfsFeed): string {
  const archiveSpec = feed.sourceArchive!
  const archive = resolve(sourceRoot, archiveSpec.relativePath)
  const before = lstatSync(archive, { bigint: true })
  if (!before.isFile() || before.isSymbolicLink()) throw new Error(`GTFS archive is not a plain file: ${archive}`)
  const archiveSha256 = createHash('sha256').update(readFileSync(archive)).digest('hex')
  const after = lstatSync(archive, { bigint: true })
  if (before.dev !== after.dev || before.ino !== after.ino || before.size !== after.size ||
      before.mtimeNs !== after.mtimeNs || archiveSha256 !== archiveSpec.sha256) {
    throw new Error(`GTFS archive identity mismatch for ${feed.id}: ${archiveSha256}`)
  }
  const parent = resolve(cacheDirectory, 'source-snapshots', feed.id)
  const target = resolve(parent, archiveSpec.sha256)
  const existing = existsSync(target) ? findGtfsRoots(target) : []
  if (existing.length === 1) return existing[0]
  if (existsSync(target)) throw new Error(`invalid extracted GTFS archive cache for ${feed.id}`)
  mkdirSync(parent, { recursive: true })
  const staging = mkdtempSync(resolve(parent, '.incoming-'))
  try {
    const extraction = spawnSync(path7za, ['x', '-y', `-o${staging}`, archive], { encoding: 'utf8' })
    if (extraction.status !== 0) {
      throw new Error(`7za exited ${extraction.status}: ${extraction.stderr.trim()}`)
    }
    const roots = findGtfsRoots(staging)
    if (roots.length !== 1) throw new Error(`GTFS archive ${feed.id} contains ${roots.length} feed roots`)
    if (roots[0] === staging) renameSync(staging, target)
    else renameSync(roots[0], target)
  } finally {
    if (existsSync(staging)) rmSync(staging, { recursive: true, force: true })
  }
  const extracted = findGtfsRoots(target)
  if (extracted.length !== 1) throw new Error(`invalid extracted GTFS archive cache for ${feed.id}`)
  return extracted[0]
}

/** Resolve immutable directories, or extract one identity-pinned historical archive into derived cache. */
export function gtfsSourceDirectories(sourceRoot: string, feed: GlobalGtfsFeed, cacheDirectory?: string): string[] {
  const direct = directoryGtfsSources(resolve(sourceRoot, feed.sourcePath ?? feed.id), feed)
  if (direct.length > 0 || !feed.sourceArchive) return direct
  if (!cacheDirectory) throw new Error(`GTFS feed ${feed.id} requires --cache-dir for its pinned source archive`)
  return [extractedArchiveSource(sourceRoot, cacheDirectory, feed)]
}

export function railFamilyFor(routeType: number, feed: GlobalGtfsFeed): 'rail' | 'tram' | null {
  if (!feed.routeTypes.has(routeType)) return null
  if (feed.metroAsRail && METRO_TYPES.has(routeType)) return 'rail'
  return routeFamily(routeType)
}

export function countryGtfsBbox(country: string, feeds = GLOBAL_GTFS_FEEDS): PreparedBbox {
  const selected = feeds.filter(feed => feed.country === country)
  if (selected.length === 0) {
    const registryLabel = feeds === GLOBAL_GTFS_FEEDS ? 'global ' : ''
    throw new Error(`no ${registryLabel}GTFS feed for country '${country}'`)
  }
  const margin = GTFS_BORDER_MARGIN_DEG
  return [
    Math.max(-90, Math.min(...selected.map(feed => feed.bbox[0])) - margin),
    Math.max(-180, Math.min(...selected.map(feed => feed.bbox[1])) - margin),
    Math.min(90, Math.max(...selected.map(feed => feed.bbox[2])) + margin),
    Math.min(180, Math.max(...selected.map(feed => feed.bbox[3])) + margin),
  ]
}

export interface GtfsSourceFreshness { lastServiceDate: string; historical?: true }
const validGtfsDate = (value: string): boolean => /^\d{8}$/.test(value)

export function gtfsTextSourceSha256(directory: string): string {
  const hash = createHash('sha256')
  const files = readdirSync(directory, { withFileTypes: true })
    .filter(entry => entry.isFile() && entry.name.endsWith('.txt'))
    .map(entry => entry.name).sort()
  for (const name of files) {
    hash.update(name); hash.update('\0'); hash.update(readFileSync(resolve(directory, name))); hash.update('\0')
  }
  return hash.digest('hex')
}

function latestDate(rows: readonly Record<string, string>[], column: string, current = '', include: (row: Readonly<Record<string, string>>) => boolean = () => true): string {
  let latest = current
  for (const row of rows) {
    if (!include(row)) continue
    const value = row[column] || ''
    if (validGtfsDate(value) && value > latest) latest = value
  }
  return latest
}

export async function validateGtfsSourceFreshness(feed: GlobalGtfsFeed, directory: string, asOfDate: string): Promise<GtfsSourceFreshness> {
  if (!validGtfsDate(asOfDate)) throw new Error(`invalid GTFS as-of date '${asOfDate}'`)
  const window = await readGtfsFeedWindow(directory)
  if (window.firstServiceDate && window.firstServiceDate > asOfDate) {
    throw new Error(`GTFS feed ${feed.id} starts ${window.firstServiceDate}; not valid for ${asOfDate} enrichment`)
  }
  let lastServiceDate = window.lastServiceDate
  if (!lastServiceDate) {
    const calendarPath = resolve(directory, 'calendar.txt')
    const calendarDatesPath = resolve(directory, 'calendar_dates.txt')
    if (existsSync(calendarPath)) lastServiceDate = latestDate(await parseCsvStream(calendarPath), 'end_date', lastServiceDate)
    if (existsSync(calendarDatesPath)) {
      lastServiceDate = latestDate(await parseCsvStream(calendarDatesPath), 'date', lastServiceDate, row => row['exception_type'] === '1')
    }
  }
  if (!lastServiceDate) throw new Error(`GTFS feed ${feed.id} has no service-validity date in ${directory}`)
  if (lastServiceDate < asOfDate) {
    const accepted = feed.acceptedHistoricalSource
    if (!accepted || accepted.lastServiceDate !== lastServiceDate) {
      throw new Error(`GTFS feed ${feed.id} expired ${lastServiceDate}; refresh it before ${asOfDate} enrichment`)
    }
    const identity = gtfsTextSourceSha256(directory)
    if (identity !== accepted.gtfsTextSha256) {
      throw new Error(`GTFS feed ${feed.id} historical source identity ${identity} does not match pinned ${accepted.gtfsTextSha256}`)
    }
    return { lastServiceDate, historical: true }
  }
  return { lastServiceDate }
}
