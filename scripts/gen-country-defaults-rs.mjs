#!/usr/bin/env node
// Generate engine/noise-compute/src/country_defaults_generated.rs from
// World Bank country data (scripts/fetch-wb-country-data.mjs) + Wikipedia
// vehicles/roads data (scripts/fetch-wiki-roads-fleet.mjs).
//
// Per-country motorway AADT scale factor (plan v5 §I.3 refined):
//
//   (1) If wiki-roads-fleet.json has vehicles_per_km for the country:
//         scale = clamp(0.7, 1.3,
//                       1.0 + 0.3 * tanh(log2(vpk / DE_vpk)))
//       where DE_vpk = 63.5 vehicles/km (Wikipedia, Germany 2024).
//       This is the direct signal: fleet / paved_km (or total_km when
//       paved unavailable) — exactly the AADT denominator we want.
//
//   (2) Else (ISO absent from wiki, or wiki vehicles_per_km is null):
//         scale = clamp(0.7, 1.3,
//                       1.0 + 0.3 * tanh(log2(density / 60)))
//       density from World Bank pop_density. Density is a weak proxy —
//       wealthier + denser → more motorization + concentrated fleet —
//       but it's all we have when Wikipedia doesn't report both signals.
//
// tanh smooths log-ratio so extreme values don't explode. ±30% band is
// conservative — Wikipedia road_paved_km is noisy (Brazil only reports
// federal/state paved, not local; Costa Rica reports 0 paved; some
// countries' fleet counts exclude 2-wheelers). Tight clamp prevents any
// single bad number from dominating.
//
// Applied to classes 0 (motorway), 1 (trunk), 2 (primary) and the ramp
// classes 10-12. Local classes 3-9 stay at WORLD — local AADT depends
// on informal transport, pedestrian share, and bike-path availability,
// all of which decorrelate from motorway AADT.
//
// Continent defaults are population-weighted averages of their countries'
// scale factors, for hexes whose country arm isn't populated (micro-
// states, ocean-side hexes near dual boundaries).
//
// Usage:
//   cd <runtime code checkout> && node scripts/gen-country-defaults-rs.mjs
//
// Commit wiki-roads-fleet.json + the generated .rs file — the World Bank dump is gitignored.

import { readFileSync, writeFileSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
// ISO alpha-2 → continent comes from the ONE shared table in
// scripts/iso-continent.mjs (also consumed by pipeline/lib/admin-at.ts).
// Must match engine/noise-compute/src/admin.rs::Continent and
// scripts/h3-admin-metros.json continent ids.
import { isoContinent } from './iso-continent.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const WB_INPUT = resolve(ROOT, 'scripts', 'wb-country-2022.json')
const WIKI_INPUT = resolve(ROOT, 'scripts', 'wiki-roads-fleet.json')
const OUTPUT = resolve(ROOT, 'engine', 'noise-compute', 'src', 'country_defaults_generated.rs')

const DE_VPK = 63.5                  // Germany vehicles/paved_km, Wikipedia 2024 — reference
const WORLD_MEDIAN_DENSITY = 60.0    // people/km², ~median of populated countries
const SCALE_MIN = 0.7                // ±30% band
const SCALE_MAX = 1.3
const SCALE_SPAN = (SCALE_MAX - SCALE_MIN) / 2  // 0.3 — tanh amplitude

const wb = JSON.parse(readFileSync(WB_INPUT, 'utf8'))
const wiki = JSON.parse(readFileSync(WIKI_INPUT, 'utf8'))
const wbCountries = wb.countries
const wikiCountries = wiki.countries

// ── Scaling formulas ────────────────────────────────────────────────────
// Both tanh-based, clamped to [SCALE_MIN, SCALE_MAX]. Rounded to 3 d.p.
// for stable diffs in the generated file.

function tanhScale(ratio) {
  const raw = 1.0 + SCALE_SPAN * Math.tanh(Math.log2(ratio))
  const clamped = Math.max(SCALE_MIN, Math.min(SCALE_MAX, raw))
  return Math.round(clamped * 1000) / 1000
}

function wikiScale(vpk) {
  if (vpk == null || vpk <= 0) return null
  return tanhScale(vpk / DE_VPK)
}

function densityScale(density) {
  if (density == null || density <= 0) return null
  return tanhScale(density / WORLD_MEDIAN_DENSITY)
}

// ── Resolve per-country scale (wiki preferred, density fallback) ────────
const countryInfo = {}  // iso → { scale, source: 'wiki'|'density', vpk?, density? }
for (const iso of Object.keys(wbCountries)) {
  const w = wikiCountries[iso]
  const vpk = w?.vehicles_per_km
  if (vpk != null && vpk > 0) {
    countryInfo[iso] = {
      scale: wikiScale(vpk),
      source: 'wiki',
      vpk,
      sourceOverride: w.source_override || null,
    }
    continue
  }
  const dens = wbCountries[iso].pop_density
  const s = densityScale(dens)
  if (s != null) {
    countryInfo[iso] = { scale: s, source: 'density', density: dens }
  }
  // else: unscaled — no entry emitted, lookup returns None
}

// Also add wiki-only countries that are missing from WB (e.g. small
// territories): these won't have admin hits in most cases but keeping
// symmetry is clean.
for (const iso of Object.keys(wikiCountries)) {
  if (countryInfo[iso]) continue
  const w = wikiCountries[iso]
  if (w.vehicles_per_km != null && w.vehicles_per_km > 0) {
    countryInfo[iso] = {
      scale: wikiScale(w.vehicles_per_km),
      source: 'wiki',
      vpk: w.vehicles_per_km,
      sourceOverride: w.source_override || null,
    }
  }
}

const sortedCountries = Object.keys(countryInfo).sort()
const wikiCount = sortedCountries.filter(iso => countryInfo[iso].source === 'wiki').length
const densityCount = sortedCountries.filter(iso => countryInfo[iso].source === 'density').length

// ── Continent averages (population-weighted) ─────────────────────────────
const contAccum = {}
for (const [iso, info] of Object.entries(countryInfo)) {
  const cont = isoContinent(iso)
  if (!cont) continue
  if (!contAccum[cont]) contAccum[cont] = { weighted: 0, weight: 0, scaledN: 0 }
  const pop = wbCountries[iso]?.population
  if (pop == null) continue
  contAccum[cont].weighted += info.scale * pop
  contAccum[cont].weight += pop
  contAccum[cont].scaledN += 1
}
const continentScale = {}
for (const [cont, a] of Object.entries(contAccum)) {
  if (a.weight > 0) {
    continentScale[cont] = Math.round((a.weighted / a.weight) * 1000) / 1000
  }
}

// ── Scale summary stats for log ────────────────────────────────────────
const scales = sortedCountries.map(iso => countryInfo[iso].scale).sort((a, b) => a - b)
const minScale = scales[0]
const maxScale = scales[scales.length - 1]
const medianScale = scales[Math.floor(scales.length / 2)]

// ── Emit Rust ────────────────────────────────────────────────────────────
const out = []
out.push(`//! Per-country and per-continent AADT scale factors relative to
//! WORLD_DEFAULT. Generated from \`scripts/wb-country-${wb.year}.json\`
//! and \`scripts/wiki-roads-fleet.json\` by
//! \`scripts/gen-country-defaults-rs.mjs\`.
//!
//! Sources:
//!   - Wikipedia *List of countries and territories by motor vehicles per
//!     capita* + *List of countries by road network size* → direct
//!     fleet / paved_km ratio (\`vehicles_per_km\`, ~160 countries).
//!   - World Bank \`EN.POP.DNST\` (people per km², ${wb.year}) — fallback
//!     for countries Wikipedia misses (~60 more).
//!
//! Method (plan v5 §I.3 refined):
//!   If wiki has vehicles_per_km:
//!     scale = clamp(${SCALE_MIN.toFixed(1)}, ${SCALE_MAX.toFixed(1)},
//!                   1.0 + ${SCALE_SPAN.toFixed(1)} × tanh(log₂(vpk / ${DE_VPK})))
//!   Else (density fallback):
//!     scale = clamp(${SCALE_MIN.toFixed(1)}, ${SCALE_MAX.toFixed(1)},
//!                   1.0 + ${SCALE_SPAN.toFixed(1)} × tanh(log₂(density / ${WORLD_MEDIAN_DENSITY.toFixed(0)})))
//!
//! Why vehicles_per_km (plan v5 §I.3 refined, was density-only):
//!   AADT on an existing road is vehicle fleet / road km. Wikipedia gives
//!   both signals directly for ~160 countries. Density was a proxy when
//!   only WB API was available (WB discontinued road_km + fleet
//!   indicators in 2011). With Wikipedia the proxy becomes direct.
//!
//!   ±30% clamp is intentional — Wikipedia \`road_paved_km\` is noisy:
//!   BR reports only federal/state paved (under-counts ×10), some
//!   countries report zero paved (CR, TH, NO) forcing fallback to total,
//!   and fleet columns in developing countries often exclude 2-wheelers
//!   (IN, VN) — we override these per \`source_override\` annotation in
//!   wiki-roads-fleet.json where a better value is known.
//!
//! Applied to motorway/trunk/primary/link classes (0/1/2/10/11/12) in
//! \`defaults.rs::country_default\`. Local road classes (3-9) stay at
//! WORLD_DEFAULT — motorway-scale doesn't predict residential AADT.
//!
//! Continent scales are population-weighted averages of their countries'
//! factors (reaches only hexes whose country assignment fell outside every
//! CGAZ polygon, e.g. disputed areas, micro-ocean cells).
//!
//! To refresh:
//!   node scripts/fetch-wb-country-data.mjs ${wb.year}
//!   node scripts/fetch-wiki-roads-fleet.mjs
//!   node scripts/gen-country-defaults-rs.mjs
//!   # commit wiki-roads-fleet.json + this file (WB dump is gitignored)

use crate::admin::Continent;

/// Per-country scale factor vs WORLD_DEFAULT motorway. ${sortedCountries.length} entries
/// (${wikiCount} from Wikipedia vehicles_per_km, ${densityCount} from WB density fallback).
pub const COUNTRY_SCALES: &[(&[u8; 2], f64)] = &[`)

for (const iso of sortedCountries) {
  const info = countryInfo[iso]
  let comment
  if (info.source === 'wiki') {
    const tag = info.sourceOverride ? ` [${info.sourceOverride}]` : ''
    comment = `wiki vpk=${info.vpk.toFixed(1)}${tag}`
  } else {
    const dens = info.density?.toFixed(1) ?? '?'
    comment = `density ${dens}/km²`
  }
  out.push(`    (b"${iso}", ${info.scale.toFixed(3)}), // ${comment}`)
}

out.push('];')
out.push('')
out.push('/// Binary-search lookup. O(log N), N = ' + sortedCountries.length + '.')
out.push('pub fn country_scale(iso: &[u8; 2]) -> Option<f64> {')
out.push('    COUNTRY_SCALES')
out.push('        .binary_search_by_key(iso, |(k, _)| **k)')
out.push('        .ok()')
out.push('        .map(|idx| COUNTRY_SCALES[idx].1)')
out.push('}')
out.push('')
out.push('/// Continent scale factor (population-weighted mean of member countries).')
out.push('/// Returned when a hex has no country arm (e.g. oceanic cells near coasts).')
out.push('pub fn continent_scale(continent: Continent) -> Option<f64> {')
out.push('    match continent {')
for (const [cont, scale] of Object.entries(continentScale).sort()) {
  out.push(`        Continent::${cont} => Some(${scale.toFixed(3)}),`)
}
out.push('        Continent::Unknown => None,')
out.push('    }')
out.push('}')
// rustfmt-clean: join('\n') + trailing '\n' = exactly one final newline
// (a trailing out.push('') here made the gate's rustfmt check fail 2026-07-28).

writeFileSync(OUTPUT, out.join('\n') + '\n')
console.log(`Wrote ${OUTPUT}`)
console.log(`  countries scaled total:   ${sortedCountries.length}`)
console.log(`  wiki-sourced (vpk):       ${wikiCount}`)
console.log(`  density-sourced fallback: ${densityCount}`)
console.log(`  scale min/median/max:     ${minScale}/${medianScale}/${maxScale}`)
console.log('\n  Continent scales (pop-weighted):')
for (const [c, s] of Object.entries(continentScale).sort()) {
  const a = contAccum[c]
  console.log(`    ${c.padEnd(14)} ${s.toFixed(3)}  (${a.scaledN} countries, ${(a.weight / 1e6).toFixed(0)}M pop)`)
}
// Spot-check reference countries
console.log('\n  Reference countries:')
for (const iso of ['DE', 'US', 'NO', 'SG', 'IN', 'NG', 'BR', 'DZ', 'MN', 'AU', 'CN', 'FR', 'GB', 'IT']) {
  const info = countryInfo[iso]
  if (!info) { console.log(`    ${iso}: [missing]`); continue }
  const src = info.source === 'wiki' ? `vpk=${info.vpk}` : `dens=${info.density?.toFixed(1)}`
  console.log(`    ${iso}: ${info.scale.toFixed(3)}  (${info.source}, ${src})`)
}
