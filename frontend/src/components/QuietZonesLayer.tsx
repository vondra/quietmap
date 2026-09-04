interface Props {
  enabled: boolean
  /** Quiet if total Lden ≤ this (dB). Kept for API compatibility. */
  threshold: number
}

/**
 * Highlight quiet areas (dev1 painted a green wash over the precomputed
 * `total` noise raster). Dev4 serves no tile raster, so this overlay is a
 * no-op that renders nothing — the toggle and URL state stay wired so the
 * UI shape is unchanged.
 */
export default function QuietZonesLayer(_props: Props): null {
  return null
}
