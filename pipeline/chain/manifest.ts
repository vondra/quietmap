/** Ordered z9 enrichment steps. Admin bake is a separate Python job and a preflight. */

import { GLOBAL_GTFS_FEEDS, NATIONAL_GTFS_FEEDS } from '../lib/railway-gtfs-feeds.ts'
import { NATIONAL_ROAD_POLICIES } from '../lib/road-national-policies/index.ts'
import { NATIONAL_ROAD_NETWORK_POLICIES } from '../lib/road-national-network-policies/index.ts'

export const PHASES = [
  'column-parents',
  'global-priors',
  'national',
  'city',
  'heuristics',
  'taper',
] as const
export type Phase = (typeof PHASES)[number]

export const ROAD_NATIONAL = ['ar', 'ca', 'cl', 'co', 'cz', 'de', 'dk', 'es', 'fi', 'fr', 'gb', 'id', 'ie', 'it', 'jp', 'mx', 'nl', 'no', 'nz', 'pe', 'pl', 'sa', 'th', 'us'] as const
export const ROAD_NATIONAL_POLICIES = [...NATIONAL_ROAD_POLICIES.keys()].map(country => country.toLowerCase())
export const ROAD_NATIONAL_NETWORKS = [...NATIONAL_ROAD_NETWORK_POLICIES.keys()].map(country => country.toLowerCase())
export const GTFS_COUNTRIES = [...new Set(GLOBAL_GTFS_FEEDS.map(feed => feed.country))].sort()
export const NATIONAL_GTFS_COUNTRIES = [...new Set(NATIONAL_GTFS_FEEDS.map(feed => feed.country))].sort()
export const SPATIAL_RAIL_COUNTRIES = ['CN', 'IN'] as const

export type Scope = { kind: 'world' }
export const LAYERS = ['roads', 'railways', 'industrial', 'buildings'] as const
export type Layer = (typeof LAYERS)[number]

export type StepKind =
  | 'built-up'
  | 'roads-europe'
  | 'roads-national'
  | 'roads-national-policy'
  | 'roads-national-network'
  | 'buildings-national'
  | 'cities-roads'
  | 'service-tree'
  | 'continuity'
  | 'taper'
  | 'industrial-global'
  | 'industrial-gem'
  | 'industrial-special'
  | 'industrial-wind'
  | 'industrial-name'
  | 'railways-cz'
  | 'railways-gtfs'
  | 'railways-national-gtfs'
  | 'railways-spatial'
  | 'railways-proxies'
  | 'railways-parallel'

export interface PlanStep {
  id: string
  phase: Phase
  kind: StepKind
  cc?: string
}

export function parseScope(raw: string): Scope {
  if (raw === 'world') return { kind: 'world' }
  throw new Error(`scope must be world; use an isolated prepared tree for a regional run, got '${raw}'`)
}

export function buildPlan(_scope: Scope, layer?: Layer): PlanStep[] {
  const steps: PlanStep[] = [
    { id: 'roads-built-up', phase: 'column-parents', kind: 'built-up' },
    { id: 'roads-europe', phase: 'global-priors', kind: 'roads-europe' },
    { id: 'industrial-global', phase: 'global-priors', kind: 'industrial-global' },
    { id: 'industrial-gem', phase: 'global-priors', kind: 'industrial-gem' },
    { id: 'industrial-special', phase: 'global-priors', kind: 'industrial-special' },
    { id: 'industrial-wind', phase: 'global-priors', kind: 'industrial-wind' },
    ...ROAD_NATIONAL.map((cc): PlanStep => ({
      id: `roads-${cc}`, phase: 'national', kind: 'roads-national',
      cc,
    })),
    ...ROAD_NATIONAL_POLICIES.map((cc): PlanStep => ({
      id: `roads-${cc}`, phase: 'national', kind: 'roads-national-policy',
      cc,
    })),
    ...ROAD_NATIONAL_NETWORKS.map((cc): PlanStep => ({
      id: `roads-${cc}`, phase: 'national', kind: 'roads-national-network',
      cc,
    })),
    { id: 'buildings-national', phase: 'national', kind: 'buildings-national' },
    { id: 'railways-cz', phase: 'national', kind: 'railways-cz' },
    ...GTFS_COUNTRIES.map((cc): PlanStep => ({
      id: `railways-gtfs-${cc.toLowerCase()}`, phase: 'national', kind: 'railways-gtfs',
      cc,
    })),
    ...NATIONAL_GTFS_COUNTRIES.map((cc): PlanStep => ({
      id: `railways-national-gtfs-${cc.toLowerCase()}`, phase: 'national', kind: 'railways-national-gtfs',
      cc,
    })),
    ...SPATIAL_RAIL_COUNTRIES.map((cc): PlanStep => ({
      id: `railways-spatial-${cc.toLowerCase()}`, phase: 'national', kind: 'railways-spatial',
      cc,
    })),
    { id: 'railways-proxies', phase: 'national', kind: 'railways-proxies' },
    { id: 'cities-roads', phase: 'city', kind: 'cities-roads' },
    { id: 'roads-service-tree', phase: 'heuristics', kind: 'service-tree' },
    { id: 'roads-continuity', phase: 'heuristics', kind: 'continuity' },
    { id: 'industrial-name', phase: 'heuristics', kind: 'industrial-name' },
    { id: 'railways-parallel', phase: 'heuristics', kind: 'railways-parallel' },
    { id: 'roads-taper', phase: 'taper', kind: 'taper' },
  ]
  return layer ? steps.filter(step => layerForStep(step) === layer) : steps
}

export function layerForStep(step: PlanStep): Layer {
  return (step.id === 'cities-roads' ? 'roads' : step.id.split('-')[0]) as Layer
}
