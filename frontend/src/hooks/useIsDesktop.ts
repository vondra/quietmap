// Whether the page is laid out for a desktop (Tailwind's md breakpoint and up): the popup is then
// the card in the right-hand column, below it the phone's bottom sheet. Both stay mounted; what
// only one of them may show (the detailed calculation, which asks the server again) goes to the
// visible one.
import { useMediaQuery } from './useMediaQuery'

export const DESKTOP_QUERY = '(min-width: 48rem)'

export function useIsDesktop(): boolean {
  return useMediaQuery(DESKTOP_QUERY)
}
