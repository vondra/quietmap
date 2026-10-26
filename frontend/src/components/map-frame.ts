// The part of the map the floating controls leave in sight, which the camera frames what it shows in
// (an opened row's segments, the reachable area): fitBounds' padding.
import { DESKTOP_QUERY } from '../hooks/useIsDesktop'
import { sheetTop } from './MobileDetailSheet'

/** The desktop popup's column on the right: its width and two gutters (index.css's
 *  --map-card-column and --map-gutter). */
const CARD_COLUMN_PX = 360 + 2 * 12
/** The least height a phone's frame is fitted into below the search bar. */
const MIN_BAND_PX = 120

/** Clear of the search bar, and of the popup's card column on a desktop or of its sheet on a phone,
 *  where the search bar also carries the open place's row. */
export function framePadding(container: HTMLElement) {
  if (window.matchMedia(DESKTOP_QUERY).matches) return { top: 72, bottom: 48, left: 48, right: CARD_COLUMN_PX + 24 }
  const height = container.clientHeight
  const bar = document.querySelector('.map-top-bar')?.getBoundingClientRect().bottom ?? 60
  const bottom = height - sheetTop(height) + 12
  // A short phone under a tall card leaves too little between them (MapLibre fits nothing into
  // none): the frame then reaches up behind the card.
  return { top: height - bottom - bar - 12 >= MIN_BAND_PX ? bar + 12 : 12, bottom, left: 24, right: 24 }
}
