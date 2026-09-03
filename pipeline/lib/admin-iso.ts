/** Reader for the prepared per-cell admin records — res-4 hex → ISO2 country
 *  (the engine's own receiver-country approximation), shared by the
 *  discontinuity auditor, the enrichment-status report and the service-tree
 *  writer. */

import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

/** One 13-byte record per cell at `{h3r4Dir}/{cell}/admin.bin`:
 *  [u64 hex LE, u8 continent, 2×u8 ISO chars, u16 city LE] — mirrors
 *  engine admin.rs and scripts/build-h3-admin.ts. The record's hex id repeats
 *  the directory name, so a record copied into another cell is caught here. */
const ADMIN_FILE_NAME = 'admin.bin'
const RECORD_BYTES = 13

/** Walk the h3r4 tree and collect every cell that resolved to a country.
 *  A directory without an admin record is simply not a prepared cell (the
 *  tree also holds a handful of non-cell entries), so it is skipped; a record
 *  that exists but does not read is a fault and throws. */
function readRecords(h3r4Dir: string): Map<string, string> {
  const out = new Map<string, string>()
  for (const cell of readdirSync(h3r4Dir)) {
    let record: Buffer
    try {
      record = readFileSync(join(h3r4Dir, cell, ADMIN_FILE_NAME))
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code
      if (code === 'ENOENT' || code === 'ENOTDIR') continue
      throw error
    }
    const path = join(h3r4Dir, cell, ADMIN_FILE_NAME)
    if (record.length !== RECORD_BYTES) {
      throw new Error(`${path} holds ${record.length} bytes, expected exactly ${RECORD_BYTES}`)
    }
    const stored = record.readBigUInt64LE(0).toString(16)
    if (stored !== cell) throw new Error(`${path} holds cell ${stored}, not ${cell}`)
    const c1 = record[9]
    const c2 = record[10]
    if (c1 === 0) continue
    out.set(cell, String.fromCharCode(c1, c2))
  }
  return out
}

/** Lenient reader for diagnostics: hex → ISO2, empty map when the tree is
 *  missing. Enrichment writers must use requireAdminIso instead. */
export function readAdminIso(h3r4Dir: string): Map<string, string> {
  if (!existsSync(h3r4Dir)) return new Map()
  return readRecords(h3r4Dir)
}

/** Strict variant for enrichers whose OUTPUT depends on the country lookup
 *  (service-tree national vehicle mix / trip rates). readAdminIso's silent
 *  empty-map fallback is fine for diagnostics, but an enricher running with
 *  it would stamp WORLD defaults over the whole planet without any error —
 *  exactly what happened when the extract built the admin records after the
 *  road passes (/gg Codex CRITICAL). Throws with the regeneration command. */
export function requireAdminIso(h3r4Dir: string): Map<string, string> {
  const regen = `Regenerate them: cd scripts && DATA_YEAR=<year> npm run build:h3-admin`
  if (!existsSync(h3r4Dir)) {
    throw new Error(`h3r4 tree missing at ${h3r4Dir} — country-dependent enrichment must not silently fall back to WORLD defaults. ${regen}`)
  }
  const map = readRecords(h3r4Dir)
  if (map.size === 0) {
    throw new Error(`no cell under ${h3r4Dir} carries an ${ADMIN_FILE_NAME} country record. ${regen}`)
  }
  return map
}
