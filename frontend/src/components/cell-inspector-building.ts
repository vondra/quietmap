/** Pure response parsing and display helpers for the vector building tooltip. */

export interface BuildingAtResult {
  height_m: number
  building_type: string
}

export type BuildingAtState =
  | { status: 'loading' }
  | { status: 'ready'; result: BuildingAtResult | null }
  | { status: 'failed' }

export type BuildingAtResponse =
  | { kind: 'unavailable' }
  | { kind: 'none' }
  | { kind: 'building'; result: BuildingAtResult }

export function parseBuildingAtResponse(value: unknown): BuildingAtResponse {
  if (value === null) return { kind: 'none' }
  if (typeof value !== 'object') throw new Error('invalid building lookup response')

  const record = value as Record<string, unknown>
  if (record.status === 'unavailable') return { kind: 'unavailable' }

  const height = record.height_m
  const buildingType = record.building_type
  if (
    typeof height !== 'number' || !Number.isFinite(height) || height <= 0 ||
    typeof buildingType !== 'string' || buildingType.length === 0
  ) {
    throw new Error('invalid building lookup response')
  }
  return { kind: 'building', result: { height_m: height, building_type: buildingType } }
}

export function formatBuildingAt(state: BuildingAtState): string {
  if (state.status === 'loading') return '…'
  if (state.status === 'failed') return 'unavailable'
  if (!state.result) return 'none'
  return `height ${state.result.height_m.toFixed(1)} m - ${state.result.building_type}`
}
