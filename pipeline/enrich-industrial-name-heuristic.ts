/**
 * OSM industrial name heuristic — global NACE code assignment.
 *
 * Assigns NACE codes to OSM industrial sites that have a `name` field but no
 * existing higher-priority entry. Uses keyword matching on the lowercased
 * name, first match wins (rules ordered from specific to generic).
 *
 * Targets the ~461k sites that have names but no NACE code assigned yet.
 * Sites with neither name nor NACE (2.6M) stay at the pipeline default (93 dB).
 *
 * Strategy:
 *   - Scan ALL h3r4 hexes globally that contain industrial.arrow
 *   - For each OSM site, check the name field
 *   - Apply keyword rules in order — first match wins
 *   - Write nace_4digit + source_id directly to the arrow via
 *     withArrowWrite(). The dataset priority check preserves higher-authority
 *     entries (national registries, E-PRTR, GEM).
 *
 * Usage:
 *   DATA_YEAR=2026 npx tsx pipeline/enrich-industrial-name-heuristic.ts
 */

import { readdirSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { makeTable, makeVector } from 'apache-arrow'
import { SOURCES_BY_KEY } from './lib/sources.js'
import { shouldOverwrite, withArrowWrite } from './lib/provenance.js'
import { SOURCE_ID_INDUSTRIAL_NAME_HEURISTIC } from './lib/source-ids.generated.js'
import { DATA_YEAR as YEAR, H3R4_DIR } from './lib/data-year.js'


const PROGRESS_INTERVAL_MS = 10_000

const MY_SOURCE_ID = SOURCE_ID_INDUSTRIAL_NAME_HEURISTIC

// ── Keyword → NACE rules ──
// Order matters: first match wins.  More specific keywords before generic ones.
// nace: 6-digit string, label: used in source tag and summary.

interface NameRule {
  keywords: string[]
  nace: string
  label: string
}

const NAME_RULES: NameRule[] = [
  // Solar (NACE 3599 synthetic) — 55 dB — before "power" to avoid generic match
  {
    keywords: ['solar', 'photovoltaic', 'pv park', 'solární', 'fotovoltaico'],
    nace: '359900',
    label: 'solar',
  },

  // Wind — SKIP (nace ''). Wind turbines are modelled separately as
  // source_type=10, NOT an industrial NACE. Must still match wind keywords
  // BEFORE the generic 'power' rule so a "wind power plant" hits this skip
  // instead of being stamped 351100 thermal. Empty nace → parseInt→NaN→0 →
  // nace4=0 → the stamp loop's `if (nace4 === 0) continue` leaves the row
  // untouched (no nace, no source_id).
  {
    keywords: ['wind farm', 'wind park', 'windpark', 'éolien', 'vindpark', 'vindkraft', 'parque eólico', 'větrná', 'wiatrowy', 'turbin'],
    nace: '',
    label: 'wind (skip)',
  },

  // Power/energy (NACE 35) — generic power keywords after solar/wind
  {
    keywords: ['power station', 'power plant', 'kraftwerk', 'centrale', 'central eléctrica', 'elektrárna', 'elektrownia', 'erőmű', 'termocentrala'],
    nace: '351100',
    label: 'power',
  },

  // Mining (NACE 07-08) — 99 dB
  {
    keywords: ['mine', 'mining', 'quarry', 'gravel', 'sand pit', 'grustak', 'steinbrudd', 'carrière', 'cantera', 'pedreira', 'steinbruch', 'kopalnia', 'lom ', 'důl', 'bánya', 'mina ', 'tambang'],
    nace: '070000',
    label: 'mining',
  },

  // Food/beverage (NACE 10-11) — 88 dB
  {
    keywords: ['brewery', 'distillery', 'winery', 'slaughterhouse', 'abattoir', 'dairy', 'mill ', 'flour', 'sugar', 'pivovar', 'mlékárna', 'jatky', 'brasserie', 'cervecería', 'cervejaria', 'brauerei', 'moulin', 'mühle', 'sucre'],
    nace: '100000',
    label: 'food',
  },

  // Textile (NACE 13) — 88 dB
  {
    keywords: ['textile', 'cotton', 'weaving', 'spinning', 'garment', 'textil'],
    nace: '130000',
    label: 'textile',
  },

  // Wood/paper (NACE 16-17) — 90 dB
  {
    keywords: ['sawmill', 'lumber', 'timber', 'pulp', 'paper', 'cellulose', 'pila ', 'scierie', 'sägewerk', 'aserradero', 'serraria', 'tartaczny'],
    nace: '160000',
    label: 'wood',
  },

  // Chemical/pharma (NACE 20-21) — 90 dB
  {
    keywords: ['chemical', 'pharma', 'refinery', 'petrochemical', 'fertilizer', 'plastic', 'chemie', 'chimique', 'química', 'raffinerie', 'refinería', 'rafinérie', 'rafineria'],
    nace: '200000',
    label: 'chemical',
  },

  // Cement/mineral (NACE 23) — 100 dB
  {
    keywords: ['cement', 'concrete', 'brick', 'ceramic', 'glass', 'tile', 'ciment', 'béton', 'zement', 'beton', 'cemento', 'cimenterie', 'betonárna', 'cihelna', 'keramik', 'vidrio', 'verrerie', 'tegel'],
    nace: '230000',
    label: 'cement/mineral',
  },

  // Metal/steel (NACE 24) — 100 dB
  {
    keywords: ['steel', 'smelter', 'foundry', 'metallurg', 'aluminum', 'aluminium', 'copper', 'iron works', 'forge', 'acier', 'stahl', 'acero', 'siderúrg', 'hutní', 'odlévárna', 'hütte', 'fonderie', 'fundición', 'fundição'],
    nace: '240000',
    label: 'metal',
  },

  // Machinery/vehicle (NACE 28-29) — 94 dB
  {
    keywords: ['automotive', 'car factory', 'vehicle', 'engine', 'turbine', 'automobile', 'automovil'],
    nace: '290000',
    label: 'vehicle',
  },

  // Water/waste (NACE 36-38) — 93 dB
  {
    keywords: ['waste', 'recycl', 'landfill', 'sewage', 'wastewater', 'treatment plant', 'incinerator', 'déchets', 'abfall', 'residuo', 'reciclaje', 'skládka', 'čistírna', 'spalovna', 'klärwerk', 'deponie', 'aterro'],
    nace: '380000',
    label: 'waste',
  },

  // Warehouse/logistics (NACE 52) — lower noise than generic industrial
  {
    keywords: ['warehouse', 'logistics', 'distribution center', 'storage', 'depot', 'lager', 'entrepôt', 'almacén', 'armazém', 'sklad', 'magazyn'],
    nace: '520000',
    label: 'warehouse',
  },

  // Agriculture (NACE 01) — 70 dB
  {
    keywords: ['farm', 'ranch', 'livestock', 'poultry', 'greenhouse', 'hatchery', 'statek', 'farma', 'ferme', 'granja', 'fazenda', 'bauernhof', 'gewächshaus', 'invernadero'],
    nace: '010000',
    label: 'agriculture',
  },
]

// ── Match a name against NAME_RULES, return the first matching rule or null ──

function matchName(name: string): NameRule | null {
  const lower = name.toLowerCase()
  for (const rule of NAME_RULES) {
    for (const kw of rule.keywords) {
      if (lower.includes(kw)) return rule
    }
  }
  return null
}

// ── Main ──

async function main() {
  console.log(`=== OSM Industrial Name Heuristic — Global (${YEAR}) ===\n`)
  console.log(`  H3R4 dir: ${H3R4_DIR}`)
  console.log(`  Rules:    ${NAME_RULES.length}`)
  console.log()

  if (!existsSync(H3R4_DIR)) {
    console.error(`ERROR: H3R4 directory not found: ${H3R4_DIR}`)
    process.exit(1)
  }

  // ── Scan all hexes globally ──
  console.log('--- Scanning all H3R4 hexes for industrial.arrow ---')
  const allHexes = readdirSync(H3R4_DIR).filter(d => d.length === 15 && d.endsWith('ffffffff'))
  console.log(`  Total hexes: ${allHexes.length.toLocaleString()}\n`)

  let hexesScanned = 0
  let hexesWithIndustrial = 0
  let totalOsmSites = 0
  let totalWithName = 0
  let newEntries = 0
  const labelCounts: Record<string, number> = {}
  const startTime = Date.now()
  let lastLog = startTime

  for (const hexId of allHexes) {
    hexesScanned++

    const now = Date.now()
    if (now - lastLog >= PROGRESS_INTERVAL_MS) {
      const pct = (hexesScanned / allHexes.length * 100).toFixed(1)
      const elapsed = ((now - startTime) / 1000).toFixed(0)
      console.log(
        `  Progress: ${hexesScanned.toLocaleString()}/${allHexes.length.toLocaleString()} hexes (${pct}%)` +
        `  with_name=${totalWithName.toLocaleString()}  new=${newEntries.toLocaleString()}  ${elapsed}s elapsed`,
      )
      lastLog = now
    }

    const indPath = resolve(H3R4_DIR, hexId, 'industrial.arrow')
    if (!existsSync(indPath)) continue
    hexesWithIndustrial++

    try {
      await withArrowWrite(indPath, table => {
        const n = table.numRows
        if (n === 0) return table
        totalOsmSites += n

        const osmId = table.getChild('osm_id')
        const nameCol = table.getChild('name')
        const existingNaceCol = table.getChild('nace_4digit')
        const existingDatasetIdCol = table.getChild('source_id')
        if (!osmId || !nameCol) return table

        const newNace = new Uint16Array(n)
        const newDatasetId = new Uint16Array(n)
        const existingSourceId = new Uint16Array(n)
        for (let j = 0; j < n; j++) {
          newNace[j] = (existingNaceCol?.get(j) as number) ?? 0
          existingSourceId[j] = (existingDatasetIdCol?.get(j) as number) ?? 0
          newDatasetId[j] = existingSourceId[j]
        }
        let anyChanged = false

        for (let i = 0; i < n; i++) {
          const rawName = nameCol.get(i) as string | null
          if (!rawName || rawName.trim() === '') continue
          totalWithName++

          const rule = matchName(rawName)
          if (!rule) continue

          const nace6 = parseInt(rule.nace, 10) || 0
          const nace4 = Math.floor(nace6 / 100)
          if (nace4 === 0) continue  // skip sentinel (e.g. wind → modelled as source_type=10, not a NACE)
          const existingId = existingSourceId[i]
          if (shouldOverwrite(existingId, MY_SOURCE_ID)) {
            newNace[i] = nace4
            newDatasetId[i] = MY_SOURCE_ID
            if (existingId === 0) newEntries++
            labelCounts[rule.label] = (labelCounts[rule.label] ?? 0) + 1
            anyChanged = true
          }
        }
        if (!anyChanged) return table
        const columns: Record<string, any> = {}
        for (const field of table.schema.fields) {
          if (field.name === 'nace_4digit' || field.name === 'source_id') continue
          columns[field.name] = table.getChild(field.name)!
        }
        columns['nace_4digit'] = makeVector(newNace)
        columns['source_id'] = makeVector(newDatasetId)
        return makeTable(columns)
      })
    } catch { /* continue */ }
  }

  const elapsed = ((Date.now() - startTime) / 1000).toFixed(1)
  console.log()
  console.log('=== Results ===')
  console.log(`  Hexes scanned:             ${hexesScanned.toLocaleString()}`)
  console.log(`  Hexes with industrial:     ${hexesWithIndustrial.toLocaleString()}`)
  console.log(`  OSM industrial sites seen: ${totalOsmSites.toLocaleString()}`)
  console.log(`  Sites with a name:         ${totalWithName.toLocaleString()}`)
  console.log(`  New/updated arrow rows:    ${newEntries.toLocaleString()}`)
  console.log(`  Time: ${elapsed}s`)

  if (Object.keys(labelCounts).length > 0) {
    console.log()
    console.log('  Label distribution:')
    const sorted = Object.entries(labelCounts).sort((a, b) => b[1] - a[1])
    for (const [label, count] of sorted) {
      console.log(`    ${label.padEnd(16)} ${count.toLocaleString()}`)
    }
  }
}

main().catch(err => { console.error('Error:', err); process.exit(1) })
