/** CN road-network classification policy ported from dev1. */

import { inBbox } from '../spatial.js'
import type { NationalRoadLinePolicy } from './types.js'

const CN_SCAN_BBOX: [number, number, number, number] = [18.0, 73.0, 54.0, 135.5]

// Exclusion zones for neighbour countries inside CN bbox
const TIER1_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Beijing', bbox: [39.7, 116.1, 40.2, 116.7] },
  { name: 'Shanghai', bbox: [31.0, 121.3, 31.5, 121.8] },
  { name: 'Guangzhou', bbox: [23.0, 113.1, 23.3, 113.6] },
  { name: 'Shenzhen', bbox: [22.4, 113.8, 22.8, 114.4] },
  { name: 'Chengdu', bbox: [30.5, 103.9, 30.8, 104.3] },
  { name: 'Chongqing', bbox: [29.4, 106.3, 29.7, 106.8] },
  { name: 'Wuhan', bbox: [30.4, 114.1, 30.8, 114.5] },
  { name: 'Xian', bbox: [34.1, 108.8, 34.4, 109.1] },
  { name: 'Hangzhou', bbox: [30.1, 120.0, 30.4, 120.4] },
  { name: 'Nanjing', bbox: [32.0, 118.6, 32.2, 119.0] },
  { name: 'Suzhou', bbox: [31.2, 120.5, 31.4, 120.8] },
  { name: 'Tianjin', bbox: [39.0, 117.1, 39.3, 117.4] },
]

// Tier-2 Chinese cities (×1.4 multiplier)
const TIER2_CITIES: Array<{ name: string; bbox: [number, number, number, number] }> = [
  { name: 'Changsha', bbox: [28.1, 112.9, 28.3, 113.1] },
  { name: 'Qingdao', bbox: [36.0, 120.2, 36.2, 120.5] },
  { name: 'Dalian', bbox: [38.8, 121.5, 39.0, 121.8] },
  { name: 'Hefei', bbox: [31.7, 117.1, 31.9, 117.4] },
  { name: 'Zhengzhou', bbox: [34.6, 113.5, 34.9, 113.8] },
  { name: 'Jinan', bbox: [36.6, 116.9, 36.8, 117.1] },
  { name: 'Kunming', bbox: [24.9, 102.6, 25.1, 102.9] },
  { name: 'Fuzhou', bbox: [26.0, 119.2, 26.2, 119.4] },
  { name: 'Xiamen', bbox: [24.4, 118.0, 24.6, 118.2] },
  { name: 'Ningbo', bbox: [29.8, 121.4, 30.0, 121.7] },
  { name: 'Wuxi', bbox: [31.5, 120.2, 31.7, 120.4] },
  { name: 'Harbin', bbox: [45.6, 126.5, 45.9, 126.8] },
  { name: 'Shenyang', bbox: [41.7, 123.3, 42.0, 123.6] },
  { name: 'Nanchang', bbox: [28.6, 115.8, 28.8, 116.0] },
  { name: 'Hohhot', bbox: [40.7, 111.6, 40.9, 111.8] },
  { name: 'Urumqi', bbox: [43.7, 87.5, 43.9, 87.8] },
  { name: 'Lanzhou', bbox: [36.0, 103.7, 36.2, 103.9] },
  { name: 'Xining', bbox: [36.6, 101.7, 36.8, 101.9] },
  { name: 'Guiyang', bbox: [26.5, 106.6, 26.7, 106.8] },
  { name: 'Haikou', bbox: [20.0, 110.2, 20.1, 110.4] },
  { name: 'Nanning', bbox: [22.7, 108.2, 22.9, 108.5] },
  { name: 'Lhasa', bbox: [29.6, 91.0, 29.7, 91.2] },
  { name: 'Yinchuan', bbox: [38.4, 106.1, 38.6, 106.3] },
  { name: 'Taiyuan', bbox: [37.7, 112.4, 37.9, 112.6] },
  { name: 'Shijiazhuang', bbox: [38.0, 114.4, 38.2, 114.6] },
  { name: 'Dongguan', bbox: [22.9, 113.6, 23.1, 113.9] },
  { name: 'Foshan', bbox: [22.9, 113.0, 23.1, 113.2] },
  { name: 'Wenzhou', bbox: [27.9, 120.5, 28.1, 120.8] },
  { name: 'Shantou', bbox: [23.3, 116.6, 23.5, 116.8] },
  { name: 'Zhuhai', bbox: [22.2, 113.5, 22.4, 113.7] },
  { name: 'Zhongshan', bbox: [22.4, 113.3, 22.6, 113.5] },
  { name: 'Huizhou', bbox: [23.0, 114.3, 23.2, 114.5] },
  { name: 'Jinhua', bbox: [29.0, 119.5, 29.2, 119.8] },
]

function cityTier(lat: number, lon: number): 0 | 1 | 2 {
  for (const c of TIER1_CITIES) if (inBbox(lat, lon, c.bbox)) return 1
  for (const c of TIER2_CITIES) if (inBbox(lat, lon, c.bbox)) return 2
  return 0
}

// ── Load highway network ──

const HWY_AADT: Record<string, number> = {
  'Highway': 60000,
  'Major road': 25000,
  'Local road': 8000,
  'Ferry': 0,
}

function tierMultiplier(tier: 0 | 1 | 2): number {
  return tier === 1 ? 2.0 : tier === 2 ? 1.4 : 1.0
}

function splitVehicles(aadt: number, tier: 0 | 1 | 2): { light: number; medium: number; heavy: number; moto: number } {
  if (tier === 1 || tier === 2) {
    // Urban Chinese: cars dominant, trucks restricted daytime, few gas motos
    return {
      light: Math.round(aadt * 0.75),
      medium: Math.round(aadt * 0.10),
      heavy: Math.round(aadt * 0.10),
      moto: Math.round(aadt * 0.05),
    }
  }
  // Rural: more trucks (freight corridors)
  return {
    light: Math.round(aadt * 0.65),
    medium: Math.round(aadt * 0.12),
    heavy: Math.round(aadt * 0.18),
    moto: Math.round(aadt * 0.05),
  }
}

const text = (line: Parameters<NationalRoadLinePolicy['traffic']>[1], name: string): string =>
  String(line.properties[name] ?? '').trim()

function traffic(row: Parameters<NationalRoadLinePolicy['traffic']>[0], line: Parameters<NationalRoadLinePolicy['traffic']>[1]) {
  const tier = cityTier(row.midLat, row.midLon)
    return splitVehicles(HWY_AADT[text(line, 'TYPE')] * tierMultiplier(tier), tier)
}

export const policy: NationalRoadLinePolicy = {
  country: 'CN',
  bbox: CN_SCAN_BBOX,
  coverage: new Set([0, 1, 2]),
  files: [
    { relativePath: 'cn/highway-network.geojson', sha256: 'abae6ee4b57dc08aa8aac3a336a6498abcdc7091dce0f1e27e8712bd1b10fe73' },
  ],
  radiusMetres: 400,
  acceptLine: line => text(line, 'ISO_CC') === 'CN' && HWY_AADT[text(line, 'TYPE')] !== undefined,
  traffic,
}
