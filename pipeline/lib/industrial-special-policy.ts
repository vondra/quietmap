/** Literal source order, admission stages and fuel classifications of ten national adapters. */

import { gemFuelNace } from './industrial-gem-source.js'

type Properties = Record<string, unknown>
export interface SpecialFeed {
  file: string
  classify(properties: Properties): number | null
  active?(properties: Properties): boolean
  require: 'coordinates' | 'area' | 'active' | 'classified'
  deduplicateBeforeFuel?: boolean
}
export const text = (value: unknown) => String(value || '')
const status = (p: Properties) => text(p.Status).toLowerCase()
const operating = (p: Properties) => status(p).includes('operating')
const operatingOrBlank = (p: Properties) => !status(p) || status(p) === 'operating'
const fuel = (p: Properties) => text(p.Type || 'unknown').toLowerCase()
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
const brazilOperating = (p: Properties) => !text(p.ESTAGIO) || /opera|sim/.test(text(p.ESTAGIO).toLowerCase())
const chinaOperating = (p: Properties) => !text(p.Status || p.status) || /operating|运营中|in operation/.test(text(p.Status || p.status).toLowerCase())
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
    { file: 'power-gen-sin.geojson', classify: bolivianGeneration, require: 'active', deduplicateBeforeFuel: true },
    { file: 'power-gen-ais.geojson', classify: bolivianGeneration, require: 'active', deduplicateBeforeFuel: true },
    { file: 'power-plants-gem.geojson', active: operating, classify: p => bolivianFuelNace(fuel(p)), require: 'active', deduplicateBeforeFuel: true },
    { ...constantFeed('power-substations.geojson', 3511, p => !(voltage(p) > 0 && voltage(p) < 69)), deduplicateBeforeFuel: true },
  ],
  BR: [constantFeed('thermal-plants.geojson', 3511, brazilOperating), constantFeed('hydro-plants.geojson', 3512, brazilOperating),
    constantFeed('nuclear-plants.geojson', 3511, brazilOperating), constantFeed('solar-plants.geojson', 3599, brazilOperating)],
  CN: [constantFeed('coal-plants.geojson', 3511, chinaOperating), constantFeed('gas-plants.geojson', 3511, chinaOperating),
    constantFeed('nuclear-plants.geojson', 3511, chinaOperating), constantFeed('lng-terminals.geojson', 3511, chinaOperating),
    constantFeed('solar-plants.geojson', 3599, chinaOperating)],
  CO: [gemExplicit],
  FJ: [{ file: 'power-plants-gem.geojson', active: operating, classify: p => gemFuelNace(fuel(p)), require: 'area' }],
  ID: [{ file: 'power-plants.geojson', active: operatingOrBlank, classify: p => gemFuelNace(text(p.Fuel || p.Type || 'unknown')), require: 'area' }],
  PY: ['power-plants-gem-py.geojson', 'power-plants-gem-border.geojson'].map(file => ({ file, active: operating,
    classify: p => gemFuelNace(fuel(p)), require: 'coordinates', deduplicateBeforeFuel: true })),
  VE: [constantFeed('oil-plants.geojson', 1900), constantFeed('power-plants-ve360.geojson', 3511, p => {
    const value = p['OPERACIÓN_ACTUAL_MW']; return (typeof value === 'number' ? value : parseFloat(text(value)) || 0) > 0
  }), gemExplicit, constantFeed('oil-wells.geojson', 600), constantFeed('substations-ve360.geojson', 3511)],
  VN: [{ file: 'power-plants.geojson', active: operatingOrBlank, classify: p => gemFuelNace(text(p.Type || p.Fuel || 'unknown')), require: 'area' }],
  ZA: [{ file: 'power-plants-eskom.geojson', classify: eskomNace, require: 'classified' },
    { ...gemExplicit, classify: p => explicitPowerNace(text(p.Type || p.Fuel || 'unknown').toLowerCase()) },
    constantFeed('coal-mines-gem.geojson', 500, p => p.Status === 'Operating')],
}
