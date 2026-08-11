/**
 * Warm the CGAZ country-polygon caches (geoBoundaries GeoJSON conversion +
 * per-polygon bbox cache) ONCE, before parallel fan-out.
 *
 * Border hexes make service-tree shards lazily derive both caches; on a fresh
 * host, 96 concurrent cold-cache derivations would duplicate a 162 MB download
 * and GDAL conversion. Cache publication itself uses collision-free atomic
 * replacement, but osm-to-h3r4.sh still runs this single-process warm-up first
 * to avoid that redundant work.
 *
 * Usage: npx tsx pipeline/prepare-country-polygon-caches.ts
 */

import { allCountryPolygonBboxes, hasCountryPolygon } from './lib/country-polygon.js'

// Any gate query forces the GeoJSON download/conversion; the bbox call builds
// the per-polygon cache from it.
hasCountryPolygon('CZ')
const countries = Object.keys(allCountryPolygonBboxes()).length
console.log(`country-polygon caches ready (${countries} countries)`)
