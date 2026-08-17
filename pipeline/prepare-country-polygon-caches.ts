/**
 * Acquire the CGAZ ADM0 boundary dataset ONCE, before parallel fan-out — the
 * ONE place in the pipeline that downloads it.
 *
 * WHY acquisition lives here and not in lib/country-polygon.ts: the gate is
 * reached from `writeRoadAadt` (national ownership is derived automatically
 * from the stamped source_id), so a lazy download on the read path put a
 * 162 MB fetch behind an ordinary unit test — which is how the data-free CI
 * gate went red when geoBoundaries deleted this release (404 since
 * 2026-08-17). The library now only ever READS a prepared dataset and fails
 * loud without one; nothing imports this file, so no test can pull the network
 * in transitively.
 *
 * Border hexes also make service-tree shards touch the gate; on a fresh host,
 * 96 concurrent cold starts would each want the same download and GDAL
 * conversion, so osm-to-h3r4.sh runs this single-process warm-up first.
 *
 * Usage: npx tsx pipeline/prepare-country-polygon-caches.ts
 */

import { mkdirSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { replaceCacheFileAtomically } from './lib/atomic-cache.js'
import { DOWNLOAD_CACHE_DIR, findDownloadedFile } from './lib/download-cache.js'
import {
  CGAZ_GPKG_FILE,
  allCountryPolygonBboxes,
  hasCountryPolygon,
  preparedCountryPolygonFile,
} from './lib/country-polygon.js'

// Pinned release tag, not `main`: boundaries silently moving under a data gate
// would make enrichment runs irreproducible. Upstream deleted the v6.0.0
// release assets (HTTP 404 since 2026-08-17) — on our hosts the surviving copy
// lives in the shared enrichment cache and download-cache.ts finds it there, so
// this fetch is only reached on a host that has neither file anywhere.
const CGAZ_URL = `https://github.com/wmgeolab/geoBoundaries/raw/v6.0.0/releaseData/CGAZ/${CGAZ_GPKG_FILE}`

if (!preparedCountryPolygonFile() && !findDownloadedFile(CGAZ_GPKG_FILE)) {
  console.log(`downloading ${CGAZ_URL}`)
  mkdirSync(DOWNLOAD_CACHE_DIR, { recursive: true })
  // Atomic replace so an interrupted or concurrent download cannot leave a
  // truncated file or steal another process's temporary path.
  replaceCacheFileAtomically(resolve(DOWNLOAD_CACHE_DIR, CGAZ_GPKG_FILE), temporaryPath => {
    execFileSync('curl', ['-fsSL', '--max-time', '600', CGAZ_URL, '-o', temporaryPath])
  })
}

// Forces the GeoPackage → GeoJSON conversion when only the GeoPackage is on
// disk, and fails loud when neither could be acquired.
hasCountryPolygon('CZ')
const countries = Object.keys(allCountryPolygonBboxes()).length
console.log(`country-polygon ready (${countries} countries in the committed bbox table)`)
