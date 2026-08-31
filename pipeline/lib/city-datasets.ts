/**
 * City traffic-enrichment registry.
 *
 * One entry per city adapter. The DRIVER (`pipeline/enrich-cities-roads.ts`)
 * iterates enabled cities, calls the adapter for normalized per-street
 * records, and stamps rows via `writeRoadAadt` — gated by the city's real
 * municipal polygon (`makeCityGate`, gbOpen ADM2; never a bbox — a bbox plus
 * the city-measured rank would overwrite neighbouring towns' national data).
 *
 * `hexBbox` is ONLY the hex-iteration envelope (which R4 cells to open);
 * the polygon gates every row inside them.
 */
import type { CityRecord } from './city-adapters/types.js'

export interface CityDataset {
  /** Registry slug — also the cache dir name `city-<slug>`. */
  slug: string
  /** ISO 3166-1 alpha-2 of the country (cache layout + docs). */
  iso2: string
  /** gbOpen ADM2 lookup: ISO alpha-3 + shapeName (labels are English-ish —
   *  "Prague", not "Hlavní město Praha"; verify per city when adding). */
  adm2: { iso3: string; shapeName: string }
  /** source_id from enrichment-datasets.ts (priority 90 → city-measured). */
  sourceId: number
  /** Hex-iteration envelope [S, W, N, E] — generous is fine. */
  hexBbox: readonly [number, number, number, number]
  /** Road classes this city's values may stamp (writeRoadAadt gate).
   *  Without it, arterial counts land on same-named service/track
   *  fragments (Praha live run: 147 class-7 rows — Codex /gg). */
  coverage: ReadonlySet<number>
  /** Max metres between a row and a `CityRecord.line` for a proximity
   *  match (default 40 — covers the far carriageway of a divided street
   *  while staying inside one block, same spirit as the EU-cities 50 m
   *  cap). Irrelevant for name-matched datasets like Praha. */
  geoMatchCapM?: number
  /** Loads + parses the dataset into normalized records (cached download). */
  load: (opts: { enrichOnly: boolean; forceDownload: boolean }) => Promise<CityRecord[]>
  enabled: boolean
}

import {
  SOURCE_ID_CITY_PRAHA_TSK,
  SOURCE_ID_CITY_WIEN_DAUERZAEHLSTELLEN,
  SOURCE_ID_CITY_BRNO_DETECTORS,
} from './source-ids.generated.js'
import { loadPraha } from './city-adapters/praha.js'
import { loadWien } from './city-adapters/wien.js'
import { loadBrno } from './city-adapters/brno.js'

export const CITY_DATASETS: readonly CityDataset[] = [
  {
    slug: 'praha',
    iso2: 'cz',
    adm2: { iso3: 'CZE', shapeName: 'Prague' },
    sourceId: SOURCE_ID_CITY_PRAHA_TSK,
    // Generous: iterateCountryHexes tests hex CENTERS — edge cells whose
    // center lies outside a tight bbox would drop the city's border streets.
    hexBbox: [49.7, 13.9, 50.4, 15.0],
    // TSK monitors the arterial network incl. major residential streets;
    // living_street/service/track/path (6-9) carry street names too but
    // must keep their own class defaults. Motorways (0 + their links 10)
    // are EXCLUDED: D-road city sections carry the ŘSD national census
    // (counted) — the city working-day proxy must not outrank it.
    coverage: new Set([1, 2, 3, 4, 5, 11, 12]),
    load: loadPraha,
    enabled: true,
  },
  {
    slug: 'wien',
    iso2: 'at',
    // gbOpen labels the city-state "Wien(Stadt)" — "Wien" alone is absent.
    adm2: { iso3: 'AUT', shapeName: 'Wien(Stadt)' },
    sourceId: SOURCE_ID_CITY_WIEN_DAUERZAEHLSTELLEN,
    hexBbox: [47.9, 15.9, 48.5, 17.0],
    // MA46 counters stand on Hauptstraßen B + Gemeindestraßen (STRTYP is
    // only B/G) — Vienna's motorways are ASFINAG's network and absent from
    // the dataset, so even though Austria has no national road enricher,
    // classes 0/10 stay excluded: a 40 m proximity match near an
    // interchange must not put a city-street DTV on an autobahn ramp.
    coverage: new Set([1, 2, 3, 4, 5, 11, 12]),
    geoMatchCapM: 40,
    load: loadWien,
    enabled: true,
  },
  {
    slug: 'brno',
    iso2: 'cz',
    // Okres Brno-město = exactly the municipality.
    adm2: { iso3: 'CZE', shapeName: 'Brno-City' },
    sourceId: SOURCE_ID_CITY_BRNO_DETECTORS,
    hexBbox: [48.95, 16.25, 49.45, 17.0],
    // The BKOM pentlogram covers the whole main network including D1/D2,
    // but motorways (0 + links 10) keep the ŘSD national census — its
    // counted per-section AADT beats the pentlogram's working-day values
    // quantized to whole thousands (same reasoning as Praha's D-roads).
    // Class 9 (unclassified) IS covered: BKOM measures real minor
    // through-roads mapped as unclassified (sections 388/504/123/289,
    // 5-15k veh/day) — without it the primary-street rule voided them
    // (Codex /gg 2026-06-12).
    coverage: new Set([1, 2, 3, 4, 5, 9, 11, 12]),
    geoMatchCapM: 40,
    load: loadBrno,
    enabled: true,
  },
]
