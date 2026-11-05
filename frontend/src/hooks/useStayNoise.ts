// Sample visible stays from the native total; keep bounded point readings while local filters change.
import { useEffect, useMemo, useRef, useState } from 'react'
import { FAILED_TILE_RETRY_MS, hm3CellAt, readHeatmapCell, tileCells, type HeatmapCellReadout } from '../lib/hm3-sample'
import { publishes, tileUrl, useTileBuild } from '../lib/tile-urls'
import { MAX_PINS, type Stay, type ViewBox } from '../lib/stays'
import { stayInView } from '../lib/stay-filters'
import type { Metric } from '../lib/hm3-decoder'

export function useStayNoise(pins: Stay[], view: ViewBox | null, metric: Metric, threshold: number | null) {
  const build = useTileBuild()
  const [samples, setSamples] = useState<Map<string, HeatmapCellReadout>>(() => new Map())
  // Effects read the latest cache without restarting every other pending tile when one arrives.
  const cache = useRef(samples)
  const [retry, retryUnavailable] = useState(0)
  const enabled = threshold != null
  const requests = useMemo(() => pins.map(pin => {
    if (!build || !publishes(build, metric)) return null
    const cell = hm3CellAt(pin.lng, pin.lat, build.zoom)
    if (!cell) return null
    const url = tileUrl(build, metric === 'lden' ? 'lden-total' : 'total', cell.z, cell.tx, cell.ty)
    return { cell, url, key: `${url}:${cell.px},${cell.py}` }
  }), [pins, build, metric])

  useEffect(() => {
    if (!enabled || !view) return
    let current = true
    let retryTimer: ReturnType<typeof setTimeout> | undefined
    const tiles = new Map<string, NonNullable<typeof requests[number]>[]>()
    requests.forEach((request, at) => {
      if (!request || !stayInView(pins[at], view)) return
      const known = cache.current.get(request.key)
      if (known && known.kind !== 'unavailable') return
      const cells = tiles.get(request.url) ?? []
      cells.push(request)
      tiles.set(request.url, cells)
    })
    // One update per arriving tile: a slow tile does not hide already assessed places. The shared
    // decoder cache may evict its larger tile, while this keeps at most one point reading per pin.
    for (const [url, cells] of tiles) void tileCells(url).then(tile => {
      if (!current) return
      const next = new Map(cache.current)
      for (const request of cells) next.set(request.key, readHeatmapCell(tile, request.cell))
      while (next.size > MAX_PINS) next.delete(next.keys().next().value!)
      cache.current = next
      setSamples(next)
      if (tile === 'failed' && retryTimer === undefined) retryTimer = setTimeout(() => retryUnavailable(n => n + 1), FAILED_TILE_RETRY_MS)
    })
    return () => { current = false; clearTimeout(retryTimer) }
  }, [requests, pins, view, enabled, retry])

  return useMemo(() => {
    const readouts = requests.map(request => request ? samples.get(request.key) : build ? { kind: 'no-level' } as const : undefined)
    const visible = readouts.filter((_, at) => view && stayInView(pins[at], view))
    return {
      pins: threshold == null ? pins : pins.filter((_, at) => {
        const sample = readouts[at]
        return sample?.kind === 'silence'
          || (metric === 'nden' && sample?.kind === 'level' && sample.sone <= threshold)
          || (metric === 'lden' && sample?.kind === 'lden' && sample.db <= threshold)
      }),
      loading: enabled && visible.some(sample => sample === undefined),
      missing: enabled && visible.some(sample => sample?.kind === 'no-level'),
      unavailable: enabled && visible.some(sample => sample?.kind === 'unavailable'),
    }
  }, [requests, samples, pins, view, threshold, enabled, metric, build])
}
