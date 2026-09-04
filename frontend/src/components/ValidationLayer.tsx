// Validation-anchor overlay: every fixture (benchmarks/world-points.json ×
// last gate run) and network station (committed snapshots × Δ tables) as
// pickable dots over the basemap — colour = gate status / Δ verdict,
// size = |distance from external truth|. Data comes from
// /api/validation/points (see server/src/routes/validation-view.ts); the
// React map enabled by the `val=1` URL flag — an owner/QA tool, not a visitor
// feature. A dot click ALSO lands a normal map click underneath, so
// the live noise popup opens for the same spot — measured next to modelled
// is the point of putting the anchors on this map.
import { useEffect, useState } from 'react'
import { MapboxOverlay } from '@deck.gl/mapbox'
import { ScatterplotLayer } from '@deck.gl/layers'
import { useMap } from 'react-map-gl/maplibre'

export interface ValidationFixture {
  kind: 'fixture'
  id: string
  lat: number
  lng: number
  regime: string
  anchor_type: string
  role: string
  metric_field: string
  tags: string[]
  pair_id: string | null
  external: {
    value?: string; metric?: string; year?: number | null; url?: string | null
    months_covered?: number | null; band?: [number | null, number | null] | null
  }
  commensurability: Record<string, unknown>
  regression_band: [number, number] | null
  known_gap: string | null
  tolerance_note: string
  caveats: string | null
  model_value: number | null
  status: string | null
  drift: number | null
  ext: { delta: number; side: string } | null
}

export interface ValidationStation {
  kind: 'station'
  network: string
  station_id: string
  name: string
  lat: number
  lng: number
  font?: string
  months_covered?: number
  coverage_pct?: number
  model: Record<string, number | null> | null
  measured_metric_field: string
  model_metric_field: string
  measured_value: number | null
  model_value: number | null
  delta_db: number | null
  verdict: string | null
  dominant_source: string | null
  [metric: string]: unknown
}

export interface ValidationNetwork {
  network: string
  year: number
  mode: string
  license: string
  source: string[]
  commensurability: Record<string, unknown>
  comparison_mode: 'two_sided' | 'upper_bound' | 'trend_only'
  comparison_tolerance_db: number | null
  comparison_tolerance_basis: string | null
  measured_metric_field: string
  model_metric_field: string
  delta_meta: ValidationArtifactMeta | null
  stations: ValidationStation[]
}

export interface ValidationArtifactMeta {
  generated_at: string | null
  server: string | null
  model_cohort: string | null
  runner_commit: string | null
  runner_dirty: boolean | null
  requested_data_year: number | null
}

export interface ValidationCohort {
  schema_version: 1
  cohort_id: string
  cache_ttl_ms: number
  data_year: string
  runtime_sha256: string
  prepared_sha256: string
}

export interface ValidationPayload {
  model_cohort: ValidationCohort | null
  lastrun: ValidationArtifactMeta | null
  warnings: string[]
  fixtures: ValidationFixture[]
  networks: ValidationNetwork[]
}

export type ValidationSelection =
  | { kind: 'fixture'; fixture: ValidationFixture }
  | { kind: 'station'; station: ValidationStation; network: ValidationNetwork }

const FIXTURE_RGB: Record<string, [number, number, number]> = {
  'OK': [46, 125, 50], 'EXTERNAL-GAP': [239, 108, 0], 'KNOWN-GAP': [142, 36, 170],
  'PENDING': [117, 117, 117], 'DRIFT': [198, 40, 40], 'ERROR': [198, 40, 40], 'SKIPPED': [189, 189, 189],
}
const STATION_RGB: Record<string, [number, number, number]> = {
  above: [198, 40, 40], within_bound: [46, 125, 50], below: [239, 108, 0],
  unattributable: [120, 144, 156], trend_only: [92, 107, 192],
  error: [198, 40, 40], no_coverage: [189, 189, 189],
}
const FALLBACK_RGB: [number, number, number] = [141, 110, 99]

interface Props {
  payload: ValidationPayload | null
  onSelect?: (selection: ValidationSelection) => void
}

/** Fetch once for the QA map; the same payload drives its status and dots. */
export function useValidationPayload(enabled: boolean): ValidationPayload | null {
  const [payload, setPayload] = useState<ValidationPayload | null>(null)
  useEffect(() => {
    if (!enabled) return
    let cancelled = false
    void fetch('/api/validation/points')
      .then((response) => {
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        return response.json()
      })
      .then((data: ValidationPayload) => { if (!cancelled) setPayload(data) })
      .catch((error) => {
        if (!cancelled) setPayload({
          model_cohort: null,
          lastrun: null,
          warnings: [`validation API failed — ${error instanceof Error ? error.message : String(error)}`],
          fixtures: [], networks: [],
        })
      })
    return () => { cancelled = true }
  }, [enabled])
  return payload
}

/** Mounted ONLY while the `val=1` flag is on (MapView) — ordinary visitors
 *  never pay for the extra deck overlay. Desktop-only QA surface. */
export default function ValidationLayer({ payload, onSelect }: Props): null {
  const { current: mapRef } = useMap()
  const [overlay, setOverlay] = useState<MapboxOverlay | null>(null)

  useEffect(() => {
    if (!mapRef) return
    const map = mapRef.getMap()
    const next = new MapboxOverlay({ interleaved: false, layers: [] })
    map.addControl(next)
    setOverlay(next)
    return () => {
      map.removeControl(next)
      setOverlay(null)
    }
  }, [mapRef])

  useEffect(() => {
    if (!overlay) return
    if (!payload) {
      overlay.setProps({ layers: [] })
      return
    }
    const stations = payload.networks.flatMap((net) => net.stations.map((station) => ({ station, net })))
    overlay.setProps({
      layers: [
        new ScatterplotLayer<{ station: ValidationStation; net: ValidationNetwork }>({
          id: 'validation-stations',
          data: stations,
          pickable: true,
          radiusUnits: 'pixels',
          getPosition: (d) => [d.station.lng, d.station.lat],
          getRadius: (d) => 3.5 + Math.min(7, Math.abs(d.station.delta_db ?? 0) * 0.45),
          getFillColor: (d) => [...(STATION_RGB[d.station.verdict ?? ''] ?? FALLBACK_RGB), 205] as [number, number, number, number],
          getLineColor: [255, 255, 255, 230],
          getLineWidth: 1,
          lineWidthUnits: 'pixels',
          stroked: true,
          onClick: (info) => {
            if (info.object) onSelect?.({ kind: 'station', station: info.object.station, network: info.object.net })
          },
        }),
        new ScatterplotLayer<ValidationFixture>({
          id: 'validation-fixtures',
          data: payload.fixtures,
          pickable: true,
          radiusUnits: 'pixels',
          getPosition: (d) => [d.lng, d.lat],
          getRadius: (d) => 7 + Math.min(9, Math.abs(d.ext?.delta ?? 0) * 0.55),
          getFillColor: (d) => [...(FIXTURE_RGB[d.status ?? ''] ?? FALLBACK_RGB), 225] as [number, number, number, number],
          getLineColor: [255, 255, 255, 255],
          getLineWidth: 1.6,
          lineWidthUnits: 'pixels',
          stroked: true,
          onClick: (info) => {
            if (info.object) onSelect?.({ kind: 'fixture', fixture: info.object })
          },
        }),
      ],
    })
  }, [overlay, payload, onSelect])

  return null
}
