/** Brazil DNIT network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const BR_BBOX: [number, number, number, number] = [-34.0, -74.0, 5.3, -34.8]

// Prepared-square scan bbox (same Brazil extent, [minLat,minLon,maxLat,maxLon]) — the prepared-square listing
// Exclusion zones for neighbouring countries
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'São Paulo (Grande SP)', bbox: [-23.85, -46.9, -23.4, -46.3] },
  { name: 'Rio de Janeiro', bbox: [-23.0, -43.8, -22.7, -43.1] },
  { name: 'Brasília', bbox: [-16.0, -48.05, -15.7, -47.8] },
  { name: 'Belo Horizonte', bbox: [-19.95, -44.1, -19.75, -43.85] },
]

// Tier-2 Brazilian cities (×1.4)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Salvador', bbox: [-13.0, -38.6, -12.85, -38.4] },
  { name: 'Fortaleza', bbox: [-3.85, -38.65, -3.7, -38.45] },
  { name: 'Curitiba', bbox: [-25.55, -49.35, -25.35, -49.15] },
  { name: 'Manaus', bbox: [-3.15, -60.1, -3.0, -59.9] },
  { name: 'Recife', bbox: [-8.1, -35.0, -7.95, -34.85] },
  { name: 'Porto Alegre', bbox: [-30.15, -51.3, -29.95, -51.1] },
  { name: 'Goiânia', bbox: [-16.75, -49.35, -16.6, -49.2] },
  { name: 'Belém', bbox: [-1.5, -48.55, -1.35, -48.4] },
  { name: 'Guarulhos', bbox: [-23.5, -46.55, -23.4, -46.4] },
  { name: 'Campinas', bbox: [-22.95, -47.1, -22.85, -47.0] },
  { name: 'São Luís', bbox: [-2.6, -44.35, -2.5, -44.2] },
  { name: 'Maceió', bbox: [-9.7, -35.8, -9.55, -35.65] },
  { name: 'Natal', bbox: [-5.85, -35.25, -5.75, -35.15] },
  { name: 'Campo Grande', bbox: [-20.5, -54.7, -20.4, -54.55] },
  { name: 'Teresina', bbox: [-5.13, -42.85, -5.05, -42.75] },
  { name: 'João Pessoa', bbox: [-7.17, -34.93, -7.07, -34.83] },
  { name: 'Nova Iguaçu', bbox: [-22.8, -43.5, -22.7, -43.4] },
  { name: 'São Bernardo', bbox: [-23.75, -46.6, -23.65, -46.5] },
  { name: 'Santo André', bbox: [-23.7, -46.6, -23.6, -46.5] },
  { name: 'Osasco', bbox: [-23.56, -46.82, -23.46, -46.72] },
  { name: 'Ribeirão Preto', bbox: [-21.2, -47.85, -21.1, -47.75] },
  { name: 'Uberlândia', bbox: [-18.95, -48.35, -18.85, -48.25] },
  { name: 'Sorocaba', bbox: [-23.55, -47.5, -23.45, -47.4] },
  { name: 'São José dos Campos', bbox: [-23.25, -45.95, -23.15, -45.85] },
  { name: 'Niterói', bbox: [-22.92, -43.15, -22.85, -43.05] },
  { name: 'Contagem', bbox: [-19.95, -44.12, -19.85, -44.02] },
  { name: 'Joinville', bbox: [-26.35, -48.9, -26.25, -48.8] },
  { name: 'Aracaju', bbox: [-10.97, -37.1, -10.87, -37.0] },
  { name: 'Cuiabá', bbox: [-15.63, -56.15, -15.53, -56.05] },
  { name: 'Florianópolis', bbox: [-27.65, -48.6, -27.55, -48.5] },
  { name: 'Vitória', bbox: [-20.35, -40.35, -20.25, -40.25] },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

function propertyText(line: Parameters<NationalRoadLinePolicy['traffic']>[1], name: string): string {
  return String(line.properties[name] ?? '').trim()
}

function dnitAadt(line: Parameters<NationalRoadLinePolicy['traffic']>[1]): number {
  const surface = propertyText(line, 'Superficie')
  if (surface !== 'PAV' && surface !== 'PAVI') return 3000
  return propertyText(line, 'Administra').includes('oncess') ? 35000 : 25000
}

function traffic(row: Parameters<NationalRoadLinePolicy['traffic']>[0], line: Parameters<NationalRoadLinePolicy['traffic']>[1]) {
  const tier = cityTier(row.midLat, row.midLon)
  const aadt = dnitAadt(line) * (tier === 1 ? 2 : tier === 2 ? 1.4 : 1)
  const split = tier === 0
    ? { light: .60, medium: .10, heavy: .25, moto: .05 }
    : { light: .70, medium: .10, heavy: .15, moto: .05 }
  return {
    light: Math.round(aadt * split.light), medium: Math.round(aadt * split.medium),
    heavy: Math.round(aadt * split.heavy), moto: Math.round(aadt * split.moto),
  }
}

export const policy: NationalRoadLinePolicy = {
  country: 'BR',
  bbox: BR_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [{
    relativePath: 'br/dnit-federal-highways.geojson',
    sha256: 'fc2899611f016fee8ad3d7f1d5f6833bdc88129f81c98d774bbe2aa799b96c06',
  }],
  radiusMetres: 400,
  acceptLine: () => true,
  traffic,
}
