// Load an opened industrial site's or unmodelled range's boundary; a late response never replaces
// another selection.
import { useEffect, useState } from 'react'
import { MAP_YEAR } from '../lib/map-year'

export function useSourceOutline(id: string | null): GeoJSON.FeatureCollection | null {
  const [loaded, setLoaded] = useState<{ id: string; outline: GeoJSON.FeatureCollection } | null>(null)
  useEffect(() => {
    if (!id) return
    const controller = new AbortController()
    void fetch(`/api/source-outline/${id}?year=${MAP_YEAR}`, { signal: controller.signal })
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
