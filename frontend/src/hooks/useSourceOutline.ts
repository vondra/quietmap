// Load the opened industrial site's boundary; a late response never replaces another selection.
import { useEffect, useState } from 'react'
import type { Contributor } from '../types/noise'

export function useSourceOutline(source: Contributor | undefined): GeoJSON.FeatureCollection | null {
  const id = source?.source_type === 'industrial' && source.metadata?.source_type !== 'wind_turbine'
    ? source.id : null
  const [loaded, setLoaded] = useState<{ id: string; outline: GeoJSON.FeatureCollection } | null>(null)
  useEffect(() => {
    if (!id) return
    const controller = new AbortController()
    void fetch(`/api/source-outline/${id}`, { signal: controller.signal })
      .then(async response => {
        if (!response.ok) return
        const outline = await response.json() as GeoJSON.FeatureCollection
        if (!controller.signal.aborted) setLoaded({ id, outline })
      })
      .catch(() => { /* The computed source points remain visible when its boundary cannot load. */ })
    return () => controller.abort()
  }, [id])
  return loaded?.id === id && loaded.outline.features.length ? loaded.outline : null
}
