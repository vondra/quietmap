/**
 * Enrich KR industrial.arrow with NACE sector codes inferred from Korean OSM names.
 *
 * The global GEM + GPPD enrichment (`enrich-global-industrial.ts`) caught Korea's
 * power plants and a few steel/cement sites, and the global name heuristic
 * (`enrich-industrial-name-heuristic.ts`, source 9000) matched 21 KR sites — but
 * its keyword list is Latin-script only, so it missed every Korean-named giant:
 * 포항제철소 (POSCO Pohang, 10.1M m² steelworks), 여수국가산업단지 (Yeosu petrochem,
 * 25.7M m²), 현대중공업 / 삼성중공업 / 한화오션 (the world's three largest shipyards),
 * 현대제철 / 동국제강, etc. They sat at source_id=0 → the generic 93 dB factory
 * profile, understating the loudest point sources in the country (steel/cement
 * NACE 24/23 → 100 dB, +7 dB over generic).
 *
 * Korea's signature heavy-industry sites carry unmistakable Hangul name tokens
 * (제철/조선/석유화학/시멘트…), so we match by Korean (+ English) keyword → NACE
 * 4-digit and stamp the engine's sector profile. This is a name heuristic
 * (priority 10): it fills source_id==0 sites and supersedes the Latin-only global
 * heuristic, but never overrides GEM/GPPD measured matches (priority 50).
 *
 * K-PRTR (icis.me.go.kr, ~4,000 facilities with real sectors) would replace this
 * with measured data, but it is geofenced to KR IPs — see docs/about/asia/kr.md.
 *
 * North Korea exclusion: the KR bbox sweeps in Pyongyang/Nampo/Kaesong, whose
 * factories carry the same Hangul tokens, plus Japan's Kyushu (Kitakyushu steel)
 * and Tsushima. A POSITIVE `makeCountryGate('KR')` is unusable here — the CGAZ
 * ADM0 coastline is generalised inland, so it rejects coastal/reclaimed-land
 * sites including the Geoje shipyards (삼성중공업/한화오션, the world's largest).
 * Instead gate NEGATIVELY: keep a site unless it falls inside North Korea or Japan
 * (the only other land in the bbox) — coastal SK sites pass, NK/JP are dropped.
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-kr.ts
 */

import { resolve } from 'node:path'
import { Uint16, vectorFromArray, makeTable } from 'apache-arrow'
import { withArrowWrite, shouldOverwrite } from './lib/provenance.js'
import { iterateCountryHexes } from './lib/roads-arrow.js'
import { makeCountryGate } from './lib/country-polygon.js'
import { SOURCE_ID_KR_INDUSTRIAL_NAMES } from './lib/source-ids.generated.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'

const MY_SOURCE_ID = SOURCE_ID_KR_INDUSTRIAL_NAMES

// KR coarse bbox [minLat,minLon,maxLat,maxLon] — sweeps in NK + a China sliver;
// makeCountryGate('KR') is the real filter, this only avoids a planet scan.
const KR_BBOX: [number, number, number, number] = [33, 124.5, 39, 132]

// Korean (+ English) name keyword → NACE 4-digit. Order matters: first match
// wins, so the loud/specific sectors precede generic ones. The engine reads the
// 2-digit class (nace_4digit / 100) for these — 24 metal 100 dB, 23 cement 100,
// 30 shipbuilding 93, 20 chemical 94, 29 vehicles 93, 17 paper 93, 35 power 97,
// 10 food 90 (see engine/noise-compute/src/emission/industrial.rs::nace_profile).
interface Rule { kw: string[]; nace: number; label: string }
const NAME_RULES: Rule[] = [
  // Iron & steel — NACE 24 (100 dB). POSCO, Hyundai Steel, Dongkuk, SeAH.
  { kw: ['제철', '제강', '철강', '제련', '특수강', 'steel', 'smelter', 'foundry', 'metallurg', 'iron works'], nace: 2410, label: 'steel' },
  // Nonferrous metals — NACE 24 (100 dB). Korea Zinc, Ulsan Aluminum, LS copper.
  { kw: ['알루미늄', '비철금속', '아연', '동제련', '제련소', 'aluminum', 'aluminium', 'copper smelt', 'zinc'], nace: 2442, label: 'nonferrous' },
  // Cement, glass, ceramics — NACE 23 (100 dB). Ssangyong, Hanil, Asia Cement.
  { kw: ['시멘트', '레미콘', '요업', '내화물', '유리공업', 'cement', 'concrete plant'], nace: 2351, label: 'cement' },
  // Shipbuilding & heavy industries — NACE 30 (93 dB). HD Hyundai, Samsung HI, Hanwha Ocean.
  { kw: ['조선', '중공업', 'shipyard', 'shipbuild', 'heavy industr'], nace: 3011, label: 'shipyard' },
  // Petrochemical / refinery / chemical — NACE 20 (94 dB). LG Chem, Lotte, SK, Hanwha.
  { kw: ['석유화학', '정유', '화학', '케미칼', '석유정제', 'petrochem', 'refinery', 'chemical'], nace: 2011, label: 'chemical' },
  // Motor vehicles — NACE 29 (93 dB). Hyundai/Kia/GM Korea/Renault.
  { kw: ['자동차', '모터스', 'automotive', 'motor company', 'car factory'], nace: 2910, label: 'automotive' },
  // Pulp & paper — NACE 17 (93 dB).
  { kw: ['제지', '펄프', 'paper mill', 'pulp'], nace: 1700, label: 'paper' },
  // Power — NACE 3511 (97 dB). Mostly GPPD-covered; this catches GPPD-missed sites.
  { kw: ['발전소', '화력', '원자력', '열병합', 'power plant', 'power station'], nace: 3511, label: 'power' },
  // Food & beverage — NACE 10 (90 dB).
  { kw: ['식품', '제분', '제당', '유가공', '양조', '맥주공장', 'brewery', 'flour mill'], nace: 1000, label: 'food' },
]

// Petrochemical mega-complexes tagged generically as 산업단지 (no sector token).
// Too big to leave at the generic profile (Yeosu alone is 25.7M m²); their
// dominant tenant industry is petrochemical/refining → NACE 20.
const PETROCHEM_COMPLEXES = ['여수국가산업단지', '여수산업단지', '울산미포', '온산국가산업단지', '대산석유화학', '대산산업단지']

function matchName(name: string): Rule | null {
  const lower = name.toLowerCase()
  for (const c of PETROCHEM_COMPLEXES) if (name.includes(c)) return { kw: [], nace: 2011, label: 'chemical' }
  for (const rule of NAME_RULES) for (const kw of rule.kw) if (lower.includes(kw.toLowerCase())) return rule
  return null
}

async function main() {
  console.log(`=== KR Industrial Enrichment — Korean-name NACE heuristic (${YEAR}) ===\n`)
  // Negative gate: the only land in KR_BBOX besides South Korea is North Korea
  // (Pyongyang/Nampo/Kaesong) and Japan (Kyushu/Tsushima). Exclude those; a
  // positive KR polygon would clip the SK coast (drops the Geoje shipyards).
  const inNK = makeCountryGate('KP')
  const inJP = makeCountryGate('JP')
  const inKR = (lat: number, lon: number) => !inNK(lat, lon) && !inJP(lat, lon)
  const hexes = iterateCountryHexes(H3R4_DIR, KR_BBOX, 'industrial.arrow')
  console.log(`  KR-bbox hexes with industrial.arrow: ${hexes.length}\n`)

  let totalSites = 0, withName = 0, matched = 0, hexesUpdated = 0, outsideKR = 0
  const labelCounts: Record<string, number> = {}
  const startTime = Date.now()

  for (let hi = 0; hi < hexes.length; hi++) {
    const arrowPath = resolve(H3R4_DIR, hexes[hi], 'industrial.arrow')
    await withArrowWrite(arrowPath, table => {
      const n = table.numRows
      if (n === 0) return table
      totalSites += n
      const nameCol = table.getChild('name')
      const latCol = table.getChild('centroid_lat')
      const lonCol = table.getChild('centroid_lon')
      const naceCol = table.getChild('nace_4digit')
      const srcCol = table.getChild('source_id')
      if (!nameCol || !latCol || !lonCol) return table

      const nace = new Uint16Array(n)
      const src = new Uint16Array(n)
      for (let i = 0; i < n; i++) {
        nace[i] = (naceCol?.get(i) as number) ?? 0
        src[i] = (srcCol?.get(i) as number) ?? 0
      }

      let any = false
      for (let i = 0; i < n; i++) {
        const rawName = nameCol.get(i) as string | null
        if (!rawName || rawName.trim() === '') continue
        withName++
        if (!shouldOverwrite(src[i], MY_SOURCE_ID)) continue
        const rule = matchName(rawName)
        if (!rule) continue
        if (!inKR(latCol.get(i) as number, lonCol.get(i) as number)) { outsideKR++; continue }
        nace[i] = rule.nace
        src[i] = MY_SOURCE_ID
        matched++
        labelCounts[rule.label] = (labelCounts[rule.label] ?? 0) + 1
        any = true
      }
      if (!any) return table
      hexesUpdated++

      const columns: Record<string, any> = {}
      for (const f of table.schema.fields) {
        if (f.name === 'nace_4digit' || f.name === 'source_id') continue
        columns[f.name] = table.getChild(f.name)!
      }
      columns['nace_4digit'] = vectorFromArray(Array.from(nace), new Uint16())
      columns['source_id'] = vectorFromArray(Array.from(src), new Uint16())
      return makeTable(columns)
    })
    if (hi % 40 === 0 || hi === hexes.length - 1) {
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(0)
      console.log(`  [${elapsed}s] ${hi + 1}/${hexes.length} hexes, ${hexesUpdated} updated, ${matched} matched`)
    }
  }

  console.log(`\n=== Results ===`)
  console.log(`  Industrial sites scanned: ${totalSites.toLocaleString()}`)
  console.log(`  Sites with a name:        ${withName.toLocaleString()}`)
  console.log(`  Matched (stamped NACE):   ${matched.toLocaleString()}`)
  console.log(`  Skipped (outside KR / NK): ${outsideKR.toLocaleString()}`)
  console.log(`  Hexes updated:            ${hexesUpdated}/${hexes.length}`)
  console.log(`\n  Sector distribution:`)
  for (const [label, c] of Object.entries(labelCounts).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${label.padEnd(14)} ${c}`)
  }
  console.log(`\n=== Done ===`)
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
