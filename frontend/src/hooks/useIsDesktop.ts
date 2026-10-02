// Whether the page is laid out for a desktop (Tailwind's md breakpoint and up): the popup is then
// the card in the right-hand column, below it the phone's bottom sheet. Both stay mounted; what
// only one of them may show (the detailed calculation, which asks the server again) goes to the
// visible one.
import { useEffect, useState } from 'react'

const DESKTOP_QUERY = '(min-width: 48rem)'

export function useIsDesktop(): boolean {
  const [desktop, setDesktop] = useState(() => window.matchMedia(DESKTOP_QUERY).matches)
  useEffect(() => {
    const query = window.matchMedia(DESKTOP_QUERY)
    const update = () => setDesktop(query.matches)
    query.addEventListener('change', update)
    return () => query.removeEventListener('change', update)
  }, [])
  return desktop
}
