/** Literal source order, admission stages and fuel classifications of fourteen national adapters. */

import { gemFuelNace } from './industrial-gem-source.js'

type Properties = Record<string, unknown>
export interface SpecialFeed {
  file: string
  classify(properties: Properties): number | null
  active?(properties: Properties): boolean
  require: 'coordinates' | 'area' | 'active' | 'classified'
  geometry?: readonly ('Point' | 'Polygon' | 'MultiPolygon')[]
  deduplicateDigits?: number
}
export interface SpecialContainmentFeed {
  file: string
  nace4: number
  active?(properties: Properties): boolean
}
export const text = (value: unknown) => String(value || '')
const status = (p: Properties) => text(p.Status).toLowerCase()
const operating = (p: Properties) => status(p).includes('operating')
const operatingOrBlank = (p: Properties) => !status(p) || status(p) === 'operating'
const fuel = (p: Properties) => text(p.Type || p.FuelType || p.Fuel || p.primary_fuel || 'unknown').toLowerCase()
function explicitPowerNace(f: string): number | null {
  if (/wind/.test(f)) return null
  if (/hydro|pump/.test(f)) return 3512
  if (/solar|csp|photovolt|pv/.test(f)) return 3599
  if (/coal|nuclear|gas|oil|biomass|bioenergy|thermal|fossil|diesel|peat/.test(f)) return 3511
  return null
}
function bolivianFuelNace(f: string): number | null {
  if (/wind|eolic|eólic/.test(f)) return null
  if (f.includes('hydro')) return 3512
  if (/solar|photovolt|fotovolt/.test(f)) return 3599
  return !f || f.includes('unknown') ? null : 3511
}
const bolivianTypes: Record<string, string> = { HE: 'hydropower', TG: 'oil/gas', BM: 'bioenergy', EO: 'wind', SL: 'solar', DO: 'diesel' }
const bolivianGeneration = (p: Properties) => bolivianFuelNace(bolivianTypes[text(p.Tipo).toUpperCase()] || 'unknown')
const voltage = (p: Properties) => typeof (p.Tension ?? p.tension) === 'number'
  ? (p.Tension ?? p.tension) as number : parseInt(String(p.Tension ?? p.tension ?? 0), 10) || 0
const gemExplicit: SpecialFeed = { file: 'power-plants-gem.geojson', active: operating, classify: p => explicitPowerNace(fuel(p)), require: 'classified' }
const constantFeed = (file: string, nace: number, active?: SpecialFeed['active']): SpecialFeed => ({ file, classify: () => nace, active, require: 'active' })
const deduplicate = (feeds: readonly SpecialFeed[], digits: number) =>
  feeds.map(feed => ({ ...feed, deduplicateDigits: digits }))
const brazilOperating = (p: Properties) => !text(p.ESTAGIO) || /opera|sim/.test(text(p.ESTAGIO).toLowerCase())
const chinaOperating = (p: Properties) => !text(p.Status || p.status) || /operating|运营中|in operation/.test(text(p.Status || p.status).toLowerCase())
const chileOperating = (p: Properties) => text(p.ESTADO).toUpperCase() === 'OPERATIVA'
const chileTailingsActive = (p: Properties) => /^(ACT|EN CONST)/.test(text(p.ESTADO_INS).toUpperCase())
const indianParkNace = (p: Properties) => {
  const category = text(p.pollution_cat).toLowerCase()
  return category.includes('red') ? 2400 : category.includes('orange') ? 2000
    : category.includes('green') ? 1300 : category.includes('white') ? 6200 : 2500
}
const peruMineActive = (p: Properties) => ['activa', 'produccion', 'ampliacion', 'desarrollo', 'exploracionavz']
  .some(status => text(p.ESTADO).toLowerCase().includes(status))
const eskomNace = (p: Properties) => {
  const category = text(p.CATEGORY).toLowerCase()
  const type = /nuclear/.test(category) ? 'nuclear' : /hydro|pump/.test(category) ? 'hydropower'
    : /gas/.test(category) ? 'gas' : /wind/.test(category) ? 'wind' : /csp|solar/.test(category) ? 'solar' : 'coal'
  return explicitPowerNace(type)
}

// Order is the original observation/tie order. BR wind is deliberately not an
// admitted dependency: its original classifier never emitted an industrial row.
export const SPECIAL_FEEDS: Readonly<Record<string, readonly SpecialFeed[]>> = {
  BO: [
    { file: 'power-gen-sin.geojson', classify: bolivianGeneration, require: 'active', deduplicateDigits: 3 },
    { file: 'power-gen-ais.geojson', classify: bolivianGeneration, require: 'active', deduplicateDigits: 3 },
    { file: 'power-plants-gem.geojson', active: operating, classify: p => bolivianFuelNace(fuel(p)), require: 'active', deduplicateDigits: 3 },
    { ...constantFeed('power-substations.geojson', 3511, p => !(voltage(p) > 0 && voltage(p) < 69)), deduplicateDigits: 3 },
  ],
  BR: [constantFeed('thermal-plants.geojson', 3511, brazilOperating), constantFeed('hydro-plants.geojson', 3512, brazilOperating),
    constantFeed('nuclear-plants.geojson', 3511, brazilOperating), constantFeed('solar-plants.geojson', 3599, brazilOperating)],
  CL: [
    { ...constantFeed('thermal-plants.geojson', 3511, chileOperating), deduplicateDigits: 4 },
    { file: 'power-plants-gem.geojson', active: operating, classify: p => gemFuelNace(fuel(p)),
      require: 'classified', deduplicateDigits: 4 },
    { ...constantFeed('mining-tailings.geojson', 700, chileTailingsActive), deduplicateDigits: 4 },
    { ...constantFeed('substations.geojson', 3511, p => typeof p.TENSION_KV !== 'number' || p.TENSION_KV >= 110), deduplicateDigits: 4 },
  ],
  CN: [constantFeed('coal-plants.geojson', 3511, chinaOperating), constantFeed('gas-plants.geojson', 3511, chinaOperating),
    constantFeed('nuclear-plants.geojson', 3511, chinaOperating), constantFeed('lng-terminals.geojson', 3511, chinaOperating),
    constantFeed('solar-plants.geojson', 3599, chinaOperating)],
  CO: [gemExplicit],
  FJ: [{ file: 'power-plants-gem.geojson', active: operating, classify: p => gemFuelNace(fuel(p)), require: 'area' }],
  ID: [{ file: 'power-plants.geojson', active: operatingOrBlank, classify: p => gemFuelNace(fuel(p)), require: 'area' }],
  IN: [
    { ...constantFeed('cement-plants.geojson', 2300), geometry: ['Polygon', 'MultiPolygon'] },
    { file: 'power-plants.geojson', classify: p => gemFuelNace(fuel(p)), require: 'classified' },
    { file: 'industrial-parks.geojson', classify: indianParkNace, require: 'active', geometry: ['Point', 'Polygon', 'MultiPolygon'] },
  ],
  PE: [
    { file: 'power-plants-gem.geojson', active: operating, classify: p => gemFuelNace(fuel(p)), require: 'classified' },
    constantFeed('mining-yacimientos.geojson', 700, peruMineActive),
  ],
  PH: [
    { file: 'power-plants.geojson', active: operatingOrBlank, classify: p => gemFuelNace(fuel(p)), require: 'classified' },
    { ...constantFeed('economic-zones.geojson', 2500), geometry: ['Point', 'Polygon'] },
  ],
  PY: ['power-plants-gem-py.geojson', 'power-plants-gem-border.geojson'].map(file => ({ file, active: operating,
    classify: p => gemFuelNace(fuel(p)), require: 'coordinates', deduplicateDigits: 4 })),
  VE: deduplicate([constantFeed('oil-plants.geojson', 1900), constantFeed('power-plants-ve360.geojson', 3511, p => {
    const value = p['OPERACIÓN_ACTUAL_MW']; return (typeof value === 'number' ? value : parseFloat(text(value)) || 0) > 0
  }), gemExplicit, constantFeed('oil-wells.geojson', 600), constantFeed('substations-ve360.geojson', 3511)], 3),
  VN: [{ file: 'power-plants.geojson', active: operatingOrBlank, classify: p => gemFuelNace(fuel(p)), require: 'area' }],
  ZA: deduplicate([{ file: 'power-plants-eskom.geojson', classify: eskomNace, require: 'classified' },
    { ...gemExplicit, classify: p => explicitPowerNace(text(p.Type || p.Fuel || 'unknown').toLowerCase()) },
    constantFeed('coal-mines-gem.geojson', 500, p => p.Status === 'Operating')], 3),
}

export const SPECIAL_CONTAINMENT_FEEDS: Readonly<Record<string, readonly SpecialContainmentFeed[]>> = {
  PE: [
    { file: 'mining-tajos.geojson', nace4: 700 },
    { file: 'mining-relaveras.geojson', nace4: 700 },
    { file: 'mining-leach-pads.geojson', nace4: 700 },
    { file: 'mining-waste-dumps.geojson', nace4: 700 },
    { file: 'mining-major-concessions.geojson', nace4: 700 },
    { file: 'mining-unidades.geojson', nace4: 700,
      active: properties => text(properties.SITUACION).toLowerCase().includes('vigente') },
  ],
}
