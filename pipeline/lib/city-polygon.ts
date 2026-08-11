/**
 * Municipal point-in-polygon gate from geoBoundaries gbOpen ADM2, pinned at
 * the same v6.0.0 tag as the ADM0 country gate (boundaries silently moving
 * under a data gate would make enrichment runs irreproducible).
 *
 * City enrichers stamp `city-measured` (rank above national) — the gate is
 * what keeps that rank honest: only rows whose midpoint lies inside the
 * city's real municipal boundary are eligible. Rectangle bboxes are NOT
 * acceptable here (a bbox + rank-above-national would overwrite neighbouring
 * towns' valid national data — city-enrichment-plan §2.2, Gemini /gg
 * CRITICAL).
 *
 * Deliberately polygon-only — no ADM0/`makeCountryGate` dependency — so
 * city-state and territory adapters (Singapore, Hong Kong) can gate on their
 * own boundary even though they have no CGAZ ADM0 feature (Codex /gg).
 *
 * Per-country ADM2 GeoJSONs are a few MB (CZE 1.7 MB) vs the 162 MB global
 * ADM0 GeoPackage — downloaded on demand, cached under `scripts/cache/`,
 * unique tmp+rename so interrupted or concurrent downloads cannot leave a
 * truncated file or steal one another's temporary path.
 */
import { readFileSync, existsSync, mkdirSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { replaceCacheFileAtomically } from './atomic-cache.js'
import { pointInRing } from './spatial.js'

const CACHE_DIR = resolve(import.meta.dirname, '..', '..', 'scripts', 'cache')

type Ring = ReadonlyArray<readonly [number, number]>
type Adm2Feature = {
  properties: { shapeName?: string }
  geometry: { type: 'Polygon' | 'MultiPolygon'; coordinates: unknown }
}

function adm2Path(iso3: string): string {
  return resolve(CACHE_DIR, `geoBoundaries-${iso3}-ADM2.geojson`)
}

function loadAdm2(iso3: string): ReadonlyArray<Adm2Feature> {
  const path = adm2Path(iso3)
  if (!existsSync(path)) {
    mkdirSync(CACHE_DIR, { recursive: true })
    const url = `https://github.com/wmgeolab/geoBoundaries/raw/v6.0.0/releaseData/gbOpen/${iso3}/ADM2/geoBoundaries-${iso3}-ADM2.geojson`
    replaceCacheFileAtomically(path, temporaryPath => {
      execFileSync('curl', ['-fsSL', '--max-time', '600', url, '-o', temporaryPath])
    })
  }
  return JSON.parse(readFileSync(path, 'utf8')).features
}

/** Diacritics/case-insensitive name compare ("Hlavní město Praha" ≡ "hlavni mesto praha"). */
function normName(s: string): string {
  return s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase().trim()
}

/**
 * A load-once point-in-municipality tester: `true` iff `(lat, lon)` lies
 * inside any outer ring of the named gbOpen ADM2 feature (holes ignored —
 * fine for a traffic gate). Throws when the ADM2 file lacks the name —
 * fail loud, a silent always-false gate would drop every city match.
 */
export function makeCityGate(iso3: string, adm2Name: string): (lat: number, lon: number) => boolean {
  const want = normName(adm2Name)
  const feat = loadAdm2(iso3).find((f) => normName(f.properties.shapeName ?? '') === want)
  if (!feat) {
    const names = loadAdm2(iso3).map((f) => f.properties.shapeName).filter(Boolean)
    throw new Error(
      `city-polygon: ADM2 "${adm2Name}" not found in gbOpen ${iso3} (${names.length} features; ` +
      `closest names: ${names.filter((n) => normName(n!).includes(want.split(' ').pop()!)).slice(0, 5).join(', ') || '—'})`,
    )
  }
  const rings: Ring[] =
    feat.geometry.type === 'Polygon'
      ? [(feat.geometry.coordinates as Ring[])[0]]
      : (feat.geometry.coordinates as Ring[][]).map((poly) => poly[0])
  return (lat, lon) => rings.some((ring) => pointInRing(lon, lat, ring))
}
