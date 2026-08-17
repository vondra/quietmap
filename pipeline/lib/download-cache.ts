/**
 * Where an already-downloaded upstream dataset is found, before anything
 * reaches the network.
 *
 * WHY a search path and not one directory: an upstream release can vanish.
 * geoBoundaries deleted the pinned v6.0.0 release assets (observed 2026-08-17,
 * HTTP 404) that both boundary gates stand on, so the copies already on disk
 * became the ONLY surviving source. `scripts/cache/` is the per-checkout
 * working cache a fresh download is published into; `data/enrichment/` is the
 * shared enrichment cache every checkout symlinks into the one data node, so a
 * file downloaded once by any checkout — or preserved there by hand after its
 * upstream disappeared — is visible to all of them.
 *
 * Both ends are repo-relative: a checkout without a data node simply misses
 * that half of the path and falls back to its own cache. No host-specific path
 * is encoded here.
 *
 * A miss returns null. Callers must then run their own acquisition step or fail
 * loud — NEVER substitute different data for a missing dataset: a boundary gate
 * fed the wrong geometry writes one country's traffic counts onto another
 * country's roads, which is the exact bug these gates exist to prevent.
 */
import { existsSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

const REPO_ROOT = resolve(import.meta.dirname, '..', '..')

/** The per-checkout working cache — where a fresh download is published. */
export const DOWNLOAD_CACHE_DIR = resolve(REPO_ROOT, 'scripts', 'cache')

/** The shared enrichment cache (`data/enrichment` → the data node). */
const SHARED_ENRICHMENT_CACHE_DIR = resolve(REPO_ROOT, 'data', 'enrichment')

/**
 * Directories consulted for an already-downloaded file, in order: the working
 * cache first, then the shared enrichment cache AND each of its immediate
 * children. The shared cache is organised per dataset family (`global/`,
 * `shared/`, per-year, a preserved-copies directory), so a flat lookup would
 * miss nearly everything in it. One level only — deep-walking a multi-terabyte
 * data node per lookup is not a search, it is an outage.
 */
function downloadSearchDirectories(): string[] {
  const dirs = [DOWNLOAD_CACHE_DIR, SHARED_ENRICHMENT_CACHE_DIR]
  if (existsSync(SHARED_ENRICHMENT_CACHE_DIR)) {
    for (const entry of readdirSync(SHARED_ENRICHMENT_CACHE_DIR)) {
      dirs.push(resolve(SHARED_ENRICHMENT_CACHE_DIR, entry))
    }
  }
  return dirs
}

/** Absolute path of `fileName` in the first search directory that holds it,
 *  else null. */
export function findDownloadedFile(fileName: string): string | null {
  for (const dir of downloadSearchDirectories()) {
    const candidate = resolve(dir, fileName)
    if (existsSync(candidate)) return candidate
  }
  return null
}

/** The search path as a human string — for the fail-loud message a caller
 *  raises when its dataset is missing everywhere. */
export function describeDownloadSearchPath(): string {
  return downloadSearchDirectories().join(', ')
}
