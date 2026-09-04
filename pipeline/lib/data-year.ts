import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

// Dataset year — ONE source of truth: the DATA_YEAR env var overrides, else
// the committed scripts/dataset-year.json. Enrichment scripts import this
// instead of hardcoding a year fallback (a bare `npx tsx pipeline/…` then
// always tracks the committed year).
export const DATA_YEAR: string =
  process.env.DATA_YEAR ||
  JSON.parse(readFileSync(resolve(import.meta.dirname, '..', '..', 'scripts', 'dataset-year.json'), 'utf-8')).current_year

/** The ONE data root both trees hang from — the same directory
 *  scripts/osm-to-h3r4.sh anchors its two outputs on. Neither side takes a root
 *  override: a second root would let a fresh extract write where the enrich
 *  chain does not read, and the chain would certify the stale tree it found. */
const DATA_DIR: string = resolve(import.meta.dirname, '..', '..', 'data')

/** The H3-res-4 extract tree of DATA_YEAR — every enricher reads and patches it in place. */
export const H3R4_DIR: string = resolve(DATA_DIR, 'prepared', DATA_YEAR, 'h3r4')

/** The OSM extract tree of DATA_YEAR: `<cell>/buildings.arrow` + `<cell>/barriers.arrow`,
 *  written by osm-to-h3r4 and refined by the buildings enrichers. A SOURCE tree, not a
 *  prepared one — no painter reads it; it is the structures builder's OSM input, which
 *  freezes it into each prepared cell's structures.arrow. Keeping it out of the cell keeps
 *  a prepared cell exactly what the painters read, and nothing that ships a cell carries it. */
export const OSM_EXTRACT_DIR: string = resolve(DATA_DIR, 'source', 'osm-extract', DATA_YEAR, 'h3r4')

/** An H3 res-4 cell directory name. Every R4 index ends in `ffffffff`, so this
 *  rejects a stray file or a scratch directory that would otherwise pass for a cell. */
const R4_CELL_DIR = /^[0-9a-f]{7}ffffffff$/

/**
 * The ONE readiness rule for the OSM extract tree, shared by every reader
 * (enrich-structures, the buildings enrichers, service-tree).
 *
 * Absence is not emptiness. `build-structures.py` and the enrichers select cells
 * by the presence of `buildings.arrow`, so a missing or bare tree reads as "no
 * OSM buildings anywhere" and the run reports success over an unmounted disk —
 * the structure builder would rewrite every table Overture-only and erase the
 * emission stock cell by cell. `root` is explicit so the tests can prove the rule
 * without the repository's own data tree; every caller uses the default.
 */
export function requireOsmExtractTree(root: string = OSM_EXTRACT_DIR): void {
  const hasCell = existsSync(root) && readdirSync(root).some((d) => R4_CELL_DIR.test(d))
  if (!hasCell) {
    throw new Error(
      `OSM extract tree missing or holds no R4 cell (${root}) — every structures.arrow would ` +
        `lose its OSM buildings and every buildings enricher would report 0 hexes over a world ` +
        `that has them; run scripts/osm-to-h3r4.sh`,
    )
  }
}

/**
 * Every prepared cell in scope carries BOTH OSM tables. `requireOsmExtractTree`
 * catches an unmounted disk; this catches a HALF-migrated or half-written tree,
 * where the readers that discover their work by the PRESENCE of buildings.arrow
 * — the invariant auditor, the country buildings enrichers — would silently skip
 * exactly the cells that are missing and report a clean run over them.
 */
export function requireOsmExtractCells(
  cells: readonly string[],
  root: string = OSM_EXTRACT_DIR,
): void {
  const missing: string[] = []
  for (const cell of cells) {
    for (const name of ['buildings.arrow', 'barriers.arrow']) {
      if (!existsSync(resolve(root, cell, name))) missing.push(`${cell}/${name}`)
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `${missing.length} OSM extract table(s) missing under ${root} for prepared cells in ` +
        `scope (first: ${missing.slice(0, 5).join(', ')}) — every prepared cell carries both, ` +
        `0-row where nothing stands; re-run scripts/osm-to-h3r4.sh rather than skip them`,
    )
  }
}
