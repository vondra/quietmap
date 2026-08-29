#!/usr/bin/env node
// Fetch per-country economic + population signals from the World Bank API.
// Used by scripts/gen-country-defaults-rs.mjs to produce the per-country
// scaling table for the engine defaults cascade (plan v5 §I.3).
//
// Output: scripts/wb-country-<year>.json — gitignored local dump. The
// committed truth is engine/noise-compute/src/country_defaults_generated.rs.
//
// Usage:
//   node scripts/fetch-wb-country-data.mjs         # year 2022 (default)
//   node scripts/fetch-wb-country-data.mjs 2021

import { writeFileSync, mkdirSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const YEAR = process.argv[2] || '2022'
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const OUT = resolve(ROOT, 'scripts', `wb-country-${YEAR}.json`)

const INDICATORS = {
  gdp_per_capita_ppp: 'NY.GDP.PCAP.PP.CD',
  population: 'SP.POP.TOTL',
  pop_density: 'EN.POP.DNST',
}

async function fetchIndicator(code) {
  // Paginated — per_page=400 covers all 260+ countries + aggregates in one hit.
  // `date=<year>` picks the single-year window; WB returns null for missing.
  const url = `https://api.worldbank.org/v2/country/all/indicator/${code}?format=json&date=${YEAR}&per_page=400`
  const res = await fetch(url)
  if (!res.ok) throw new Error(`WB ${code}: HTTP ${res.status}`)
  const body = await res.json()
  const rows = body[1] || []
  const out = {}
  for (const r of rows) {
    if (r.value == null) continue
    // r.country.id is ISO-3166 alpha-2; skip WB-defined aggregates (XM, XQ, etc.)
    const iso2 = r.country.id
    if (!/^[A-Z]{2}$/.test(iso2)) continue
    out[iso2] = r.value
  }
  return out
}

async function main() {
  console.log(`Fetching World Bank data for ${YEAR}…`)
  const results = {}
  for (const [name, code] of Object.entries(INDICATORS)) {
    const data = await fetchIndicator(code)
    console.log(`  ${code} (${name}): ${Object.keys(data).length} countries`)
    results[name] = data
  }

  // Union of all ISO codes that had ≥ 1 indicator populated
  const allIso = new Set()
  for (const dict of Object.values(results)) {
    for (const k of Object.keys(dict)) allIso.add(k)
  }

  const payload = {
    source: 'World Bank Open Data',
    api_root: 'https://api.worldbank.org/v2',
    year: Number(YEAR),
    fetched_at: new Date().toISOString(),
    indicators: INDICATORS,
    countries: {},
  }

  for (const iso of [...allIso].sort()) {
    payload.countries[iso] = {
      gdp_per_capita_ppp: results.gdp_per_capita_ppp[iso] ?? null,
      population: results.population[iso] ?? null,
      pop_density: results.pop_density[iso] ?? null,
    }
  }

  mkdirSync(dirname(OUT), { recursive: true })
  writeFileSync(OUT, JSON.stringify(payload, null, 2))
  console.log(`Wrote ${OUT} — ${allIso.size} countries`)
}

main().catch(e => { console.error(e); process.exit(1) })
