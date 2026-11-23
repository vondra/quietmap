// Keep a browser media query current when the layout or screen changes.
import { useEffect, useState } from 'react'

export function useMediaQuery(condition: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(condition).matches)
  useEffect(() => {
    const query = window.matchMedia(condition)
    const update = () => setMatches(query.matches)
    query.addEventListener('change', update)
    update()
    return () => query.removeEventListener('change', update)
  }, [condition])
  return matches
}
