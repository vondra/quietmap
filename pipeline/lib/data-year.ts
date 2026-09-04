import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

// Dataset year — ONE source of truth: the DATA_YEAR env var overrides, else
// the committed scripts/dataset-year.json. Enrichment scripts import this
// instead of hardcoding a year fallback (a bare `npx tsx pipeline/…` then
// always tracks the committed year).
export const DATA_YEAR: string =
  process.env.DATA_YEAR ||
  JSON.parse(readFileSync(resolve(import.meta.dirname, '..', '..', 'scripts', 'dataset-year.json'), 'utf-8')).current_year

/** The H3-res-4 extract tree of DATA_YEAR — every enricher reads and patches it in place. */
export const H3R4_DIR: string = resolve(import.meta.dirname, '..', '..', 'data', 'prepared', DATA_YEAR, 'h3r4')

/** The OSM extract tree of DATA_YEAR: `<cell>/buildings.arrow` + `<cell>/barriers.arrow`,
 *  written by osm-to-h3r4 and refined by the buildings enrichers. A SOURCE tree, not a
 *  prepared one — no painter reads it; it is the structures builder's OSM input, which
 *  freezes it into each prepared cell's structures.arrow. Keeping it out of the cell keeps
 *  a prepared cell exactly what the painters read, and nothing that ships a cell carries it. */
export const OSM_EXTRACT_DIR: string = resolve(import.meta.dirname, '..', '..', 'data', 'source', 'osm-extract', DATA_YEAR, 'h3r4')
