/** Ordered dev1 name priors, with owned retirement and unchanged native industrial payload. */

import { resolve } from 'node:path'
import { DataType, makeVector, Table, type Vector } from 'apache-arrow'
import { bakedIndustrialCountryReader, gridToLonLat, iso2Code, listPreparedSquares } from './prepared-grid.js'
import { shouldOverwrite, withArrowWrite } from './provenance.js'
import { SOURCE_ID_INDUSTRIAL_NAME_HEURISTIC, SOURCE_ID_KR_INDUSTRIAL_NAMES } from './sources.js'

// Literal dev1 keywords and first-match order; wind is a terminal skip before power.
const NAME_RULES = [
  { keywords: ['solar', 'photovoltaic', 'pv park', 'solární', 'fotovoltaico'], nace4: 3599 },
  { keywords: ['wind farm', 'wind park', 'windpark', 'éolien', 'vindpark', 'vindkraft', 'parque eólico', 'větrná', 'wiatrowy', 'turbin'], nace4: 0 },
  { keywords: ['power station', 'power plant', 'kraftwerk', 'centrale', 'central eléctrica', 'elektrárna', 'elektrownia', 'erőmű', 'termocentrala'], nace4: 3511 },
  { keywords: ['mine', 'mining', 'quarry', 'gravel', 'sand pit', 'grustak', 'steinbrudd', 'carrière', 'cantera', 'pedreira', 'steinbruch', 'kopalnia', 'lom ', 'důl', 'bánya', 'mina ', 'tambang'], nace4: 700 },
  { keywords: ['brewery', 'distillery', 'winery', 'slaughterhouse', 'abattoir', 'dairy', 'mill ', 'flour', 'sugar', 'pivovar', 'mlékárna', 'jatky', 'brasserie', 'cervecería', 'cervejaria', 'brauerei', 'moulin', 'mühle', 'sucre'], nace4: 1000 },
  { keywords: ['textile', 'cotton', 'weaving', 'spinning', 'garment', 'textil'], nace4: 1300 },
  { keywords: ['sawmill', 'lumber', 'timber', 'pulp', 'paper', 'cellulose', 'pila ', 'scierie', 'sägewerk', 'aserradero', 'serraria', 'tartaczny'], nace4: 1600 },
  { keywords: ['chemical', 'pharma', 'refinery', 'petrochemical', 'fertilizer', 'plastic', 'chemie', 'chimique', 'química', 'raffinerie', 'refinería', 'rafinérie', 'rafineria'], nace4: 2000 },
  { keywords: ['cement', 'concrete', 'brick', 'ceramic', 'glass', 'tile', 'ciment', 'béton', 'zement', 'beton', 'cemento', 'cimenterie', 'betonárna', 'cihelna', 'keramik', 'vidrio', 'verrerie', 'tegel'], nace4: 2300 },
  { keywords: ['steel', 'smelter', 'foundry', 'metallurg', 'aluminum', 'aluminium', 'copper', 'iron works', 'forge', 'acier', 'stahl', 'acero', 'siderúrg', 'hutní', 'odlévárna', 'hütte', 'fonderie', 'fundición', 'fundição'], nace4: 2400 },
  { keywords: ['automotive', 'car factory', 'vehicle', 'engine', 'turbine', 'automobile', 'automovil'], nace4: 2900 },
  { keywords: ['waste', 'recycl', 'landfill', 'sewage', 'wastewater', 'treatment plant', 'incinerator', 'déchets', 'abfall', 'residuo', 'reciclaje', 'skládka', 'čistírna', 'spalovna', 'klärwerk', 'deponie', 'aterro'], nace4: 3800 },
  { keywords: ['warehouse', 'logistics', 'distribution center', 'storage', 'depot', 'lager', 'entrepôt', 'almacén', 'armazém', 'sklad', 'magazyn'], nace4: 5200 },
  { keywords: ['farm', 'ranch', 'livestock', 'poultry', 'greenhouse', 'hatchery', 'statek', 'farma', 'ferme', 'granja', 'fazenda', 'bauernhof', 'gewächshaus', 'invernadero'], nace4: 100 },
] as const

export function industrialNameRule(name: string) {
  const lower = name.toLowerCase()
  return NAME_RULES.find(rule => rule.keywords.some(keyword => lower.includes(keyword))) ?? null
}

const KOREAN_NAME_RULES = [
  { keywords: ['제철', '제강', '철강', '제련', '특수강', 'steel', 'smelter', 'foundry', 'metallurg', 'iron works'], nace4: 2410 },
  { keywords: ['알루미늄', '비철금속', '아연', '동제련', '제련소', 'aluminum', 'aluminium', 'copper smelt', 'zinc'], nace4: 2442 },
  { keywords: ['시멘트', '레미콘', '요업', '내화물', '유리공업', 'cement', 'concrete plant'], nace4: 2351 },
  { keywords: ['조선', '중공업', 'shipyard', 'shipbuild', 'heavy industr'], nace4: 3011 },
  { keywords: ['석유화학', '정유', '화학', '케미칼', '석유정제', 'petrochem', 'refinery', 'chemical'], nace4: 2011 },
  { keywords: ['자동차', '모터스', 'automotive', 'motor company', 'car factory'], nace4: 2910 },
  { keywords: ['제지', '펄프', 'paper mill', 'pulp'], nace4: 1700 },
  { keywords: ['발전소', '화력', '원자력', '열병합', 'power plant', 'power station'], nace4: 3511 },
  { keywords: ['식품', '제분', '제당', '유가공', '양조', '맥주공장', 'brewery', 'flour mill'], nace4: 1000 },
] as const
const KOREAN_PETROCHEMICAL_COMPLEXES = [
  '여수국가산업단지', '여수산업단지', '울산미포', '온산국가산업단지', '대산석유화학', '대산산업단지',
] as const

export function koreanIndustrialNameRule(name: string) {
  const lower = name.toLowerCase()
  if (KOREAN_PETROCHEMICAL_COMPLEXES.some(complex => name.includes(complex))) return { nace4: 2011 }
  return KOREAN_NAME_RULES.find(rule => rule.keywords.some(keyword => lower.includes(keyword))) ?? null
}

function integerColumn(table: Table, name: string, bits: number, signed: boolean, optional = false): Vector | null {
  const column = table.getChild(name)
  if (!column && optional) return null
  if (!column || !DataType.isInt(column.type) || column.type.isSigned !== signed || column.type.bitWidth !== bits || column.nullCount) {
    throw new Error(`industrial Arrow '${name}' must be non-null ${signed ? 'Int' : 'Uint'}${bits}`)
  }
  return column
}

export async function enrichIndustrialNames(preparedDirectory: string) {
  const squares = listPreparedSquares(preparedDirectory, [-90, -180, 90, 180], 'industrial.arrow')
  if (!squares.length) throw new Error(`${preparedDirectory}: no industrial Arrow scope`)
  const result = { squares: squares.length, rows: 0, named: 0, classified: 0, retired: 0, squaresUpdated: 0 }
  for (const square of squares) {
    await withArrowWrite(resolve(preparedDirectory, square, 'industrial.arrow'), table => {
      if (table.schema.metadata.get('grid') !== 'z30') throw new Error('industrial Arrow requires grid=z30')
      const names = table.getChild('name')
      if (!names || !DataType.isUtf8(names.type)) throw new Error('industrial Arrow requires Utf8 name')
      const source = integerColumn(table, 'source_id', 16, false)!
      const sourceType = integerColumn(table, 'source_type', 8, false)!
      const nace = integerColumn(table, 'nace_4digit', 16, false, true)
      const gx = integerColumn(table, 'centroid_gx', 32, true)!
      const gy = integerColumn(table, 'centroid_gy', 32, true)!
      const country = bakedIndustrialCountryReader(table)
      const newSource = Uint16Array.from(source as Iterable<number>)
      const newNace = nace ? Uint16Array.from(nace as Iterable<number>) : new Uint16Array(table.numRows)
      let changed = false
      result.rows += table.numRows
      for (let row = 0; row < table.numRows; row++) {
        // Native turbines keep their independent point-source classification and measurements.
        if (sourceType.get(row) === 10) continue
        const name = names.get(row) as string | null
        if (name?.trim()) result.named++
        const globalRule = name ? industrialNameRule(name) : null
        const position = gridToLonLat(gx.get(row) as number, gy.get(row) as number)
        const koreanScope = position.lat >= 33 && position.lat <= 39 && position.lon >= 124.5 && position.lon <= 132 &&
          ![iso2Code('KP'), iso2Code('JP')].includes(country.codeAt(row))
        const koreanRule = name && koreanScope ? koreanIndustrialNameRule(name) : null
        const desired = koreanRule ? { source: SOURCE_ID_KR_INDUSTRIAL_NAMES, nace4: koreanRule.nace4 }
          : globalRule?.nace4 ? { source: SOURCE_ID_INDUSTRIAL_NAME_HEURISTIC, nace4: globalRule.nace4 } : null
        const currentSource = newSource[row], currentNace = newNace[row]
        const owned = currentSource === SOURCE_ID_INDUSTRIAL_NAME_HEURISTIC || currentSource === SOURCE_ID_KR_INDUSTRIAL_NAMES
        if (desired && (owned || shouldOverwrite(currentSource, desired.source))) {
          newSource[row] = desired.source
          newNace[row] = desired.nace4
          result.classified++
        } else if (owned && !desired) {
          // A renamed/removed name cannot keep an obsolete owned class after a rerun.
          newSource[row] = 0; newNace[row] = 0
          result.retired++
        }
        if (currentSource !== newSource[row] || currentNace !== newNace[row]) changed = true
      }
      if (!changed) return table
      const columns: Record<string, Vector> = {}
      for (const field of table.schema.fields) columns[field.name] = table.getChild(field.name)!
      columns.source_id = makeVector(newSource)
      columns.nace_4digit = makeVector(newNace)
      // Suppression belongs to the registry's verified facility election; names cannot wake a duplicate.
      result.squaresUpdated++
      return new Table(columns)
    })
  }
  return result
}
