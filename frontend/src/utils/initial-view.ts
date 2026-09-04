// First-visit map view (no #hash in the URL): approximate the visitor's
// COUNTRY WITHOUT any permission prompt — Google-style layering:
//   1. shared link (#hash) wins — handled by useUrlState, never reaches here
//   2. server IP COUNTRY guess (`fetchIpCountry` → /api/initial-view, DB-IP
//      lite on the server) — bounded, raced before the map mounts
//   3. browser language REGION subtag (cs-CZ, en-GB, de-AT…) → country view
//   4. unambiguous single-country languages (cs, pl, hu…) → country view
//   5. fallback: whole Europe
// We guess COUNTRY, not city: free GeoIP places consumer/mobile ISP pools at
// the provider's regional registration, not the subscriber (measured
// 2026-07-19 — an O2 CZ line in Kytín geolocated 35–230 km wrong across four
// DBs, all of which still agreed on the country). A wrong city zoom is worse
// than a right country view. Precise location is a separate, user-triggered
// gesture (the locate button) — auto-prompting for GPS on load is hostile UX
// and browsers deprioritize it.

export interface InitialView {
  lat: number
  lng: number
  zoom: number
}

export const EUROPE_VIEW: InitialView = { lat: 50.5, lng: 9.5, zoom: 4 }

// Country centroids at a "whole country on screen" zoom. Microstates get
// z10–z12, small z7–z9, mid z6, large z4–z5 — coarse is fine, this is a first
// guess, not a fix. Coverage is Europe-complete plus US/CA — the product's
// priority regions (CLAUDE.md) and where the noise data is richest. An IP or
// language country with NO entry here intentionally falls through to
// EUROPE_VIEW rather than guessing (or dropping a first-time visitor onto a
// blank non-covered map); curate a new country's first view by adding a row.
const COUNTRY_VIEW: Record<string, InitialView> = {
  CZ: { lat: 49.8, lng: 15.5, zoom: 7 },
  SK: { lat: 48.7, lng: 19.7, zoom: 7 },
  DE: { lat: 51.2, lng: 10.4, zoom: 6 },
  AT: { lat: 47.6, lng: 14.1, zoom: 7 },
  CH: { lat: 46.8, lng: 8.2, zoom: 7 },
  PL: { lat: 52.1, lng: 19.4, zoom: 6 },
  FR: { lat: 46.6, lng: 2.5, zoom: 6 },
  ES: { lat: 40.2, lng: -3.6, zoom: 6 },
  PT: { lat: 39.6, lng: -8.0, zoom: 7 },
  IT: { lat: 42.8, lng: 12.5, zoom: 6 },
  GB: { lat: 54.0, lng: -2.5, zoom: 6 },
  IE: { lat: 53.4, lng: -8.0, zoom: 7 },
  NL: { lat: 52.2, lng: 5.3, zoom: 7 },
  BE: { lat: 50.6, lng: 4.7, zoom: 7 },
  LU: { lat: 49.8, lng: 6.1, zoom: 9 },
  DK: { lat: 56.0, lng: 10.0, zoom: 7 },
  NO: { lat: 62.0, lng: 9.5, zoom: 5 },
  SE: { lat: 62.5, lng: 16.0, zoom: 5 },
  FI: { lat: 63.0, lng: 26.0, zoom: 5 },
  EE: { lat: 58.7, lng: 25.5, zoom: 7 },
  LV: { lat: 56.9, lng: 24.9, zoom: 7 },
  LT: { lat: 55.3, lng: 23.9, zoom: 7 },
  HU: { lat: 47.2, lng: 19.4, zoom: 7 },
  SI: { lat: 46.1, lng: 14.8, zoom: 8 },
  HR: { lat: 45.0, lng: 16.4, zoom: 7 },
  RO: { lat: 45.9, lng: 25.0, zoom: 6 },
  BG: { lat: 42.7, lng: 25.3, zoom: 7 },
  GR: { lat: 39.0, lng: 22.9, zoom: 6 },
  // Rest of Europe (incl. transcontinental neighbours + Caucasus), so no
  // European visitor's IP/language country silently drops to the Europe view.
  UA: { lat: 48.8, lng: 31.3, zoom: 5 },
  BY: { lat: 53.6, lng: 28.0, zoom: 6 },
  MD: { lat: 47.1, lng: 28.5, zoom: 7 },
  RU: { lat: 55.5, lng: 38.0, zoom: 4 },
  TR: { lat: 39.0, lng: 35.2, zoom: 6 },
  IS: { lat: 64.9, lng: -18.8, zoom: 6 },
  RS: { lat: 44.0, lng: 20.9, zoom: 7 },
  BA: { lat: 44.0, lng: 17.8, zoom: 7 },
  MK: { lat: 41.6, lng: 21.7, zoom: 8 },
  AL: { lat: 41.2, lng: 20.0, zoom: 8 },
  ME: { lat: 42.8, lng: 19.3, zoom: 8 },
  XK: { lat: 42.6, lng: 20.9, zoom: 8 },
  GE: { lat: 42.0, lng: 43.5, zoom: 7 },
  AM: { lat: 40.2, lng: 45.0, zoom: 7 },
  AZ: { lat: 40.3, lng: 47.7, zoom: 7 },
  CY: { lat: 35.0, lng: 33.2, zoom: 9 },
  MT: { lat: 35.9, lng: 14.4, zoom: 10 },
  AD: { lat: 42.5, lng: 1.5, zoom: 10 },
  LI: { lat: 47.15, lng: 9.55, zoom: 11 },
  MC: { lat: 43.74, lng: 7.42, zoom: 12 },
  SM: { lat: 43.94, lng: 12.46, zoom: 11 },
  US: { lat: 39.8, lng: -98.6, zoom: 4 },
  CA: { lat: 56.0, lng: -96.0, zoom: 4 },
}

// Languages spoken overwhelmingly in ONE country — safe without a region
// subtag. Multi-country languages (en, de, fr, es, pt, nl, sv…) stay out:
// guessing DE for plain `de` would misplace Austrians and Swiss.
const UNAMBIGUOUS_LANGUAGE_COUNTRY: Record<string, string> = {
  cs: 'CZ', sk: 'SK', pl: 'PL', hu: 'HU', sl: 'SI', hr: 'HR', ro: 'RO',
  bg: 'BG', el: 'GR', da: 'DK', nb: 'NO', nn: 'NO', fi: 'FI', et: 'EE',
  lv: 'LV', lt: 'LT', it: 'IT', uk: 'UA', tr: 'TR', is: 'IS', mk: 'MK',
  be: 'BY', ka: 'GE', hy: 'AM', az: 'AZ',
}

/**
 * Server-side COUNTRY guess from the visitor's IP (/api/initial-view, DB-IP
 * City Lite looked up in memory on our origin). Returns an ISO 3166-1 alpha-2
 * code (e.g. `CZ`) or null. Bounded by `timeoutMs` so a slow origin can never
 * hold up the first map paint; any failure, timeout or `source:'none'` → null
 * → the language fallback in resolveInitialView.
 */
export async function fetchIpCountry(timeoutMs: number): Promise<string | null> {
  try {
    const res = await fetch('/api/initial-view', { signal: AbortSignal.timeout(timeoutMs) })
    if (!res.ok) return null
    const body = (await res.json()) as { source?: unknown; country?: unknown }
    if (body.source !== 'ip-country' || typeof body.country !== 'string') return null
    const country = body.country.toUpperCase()
    return /^[A-Z]{2}$/.test(country) ? country : null
  } catch {
    return null
  }
}

/**
 * Resolve the first-visit view from (optionally) the IP-guessed country and
 * the browser languages. `ipCountry` — an ISO alpha-2 code or null — is the
 * most reliable geo signal and wins when we have a country view for it; it is
 * omitted for the synchronous language-only fallback (useUrlState) that runs
 * before the IP fetch resolves.
 */
export function resolveInitialView(
  languages: readonly string[] = navigator.languages ?? [],
  ipCountry: string | null = null,
): InitialView {
  if (ipCountry && COUNTRY_VIEW[ipCountry]) return COUNTRY_VIEW[ipCountry]
  for (const tag of languages) {
    const region = tag.split('-')[1]?.toUpperCase()
    if (region && COUNTRY_VIEW[region]) return COUNTRY_VIEW[region]
  }
  for (const tag of languages) {
    const lang = tag.split('-')[0].toLowerCase()
    const country = UNAMBIGUOUS_LANGUAGE_COUNTRY[lang]
    if (country && COUNTRY_VIEW[country]) return COUNTRY_VIEW[country]
  }
  return EUROPE_VIEW
}
