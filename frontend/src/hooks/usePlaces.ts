// The visitor's places for React: one keeper (lib/places-keeper.ts) for the page, read again whenever
// the map comes back to the screen, and the names another tab gives while they are this browser's.
import { useEffect, useState, useSyncExternalStore } from 'react'
import { PLACE_TITLES_KEY } from '../lib/browser-places'
import { createPlacesKeeper } from '../lib/places-keeper'

export function usePlaces() {
  const [keeper] = useState(createPlacesKeeper)
  const shown = useSyncExternalStore(keeper.subscribe, keeper.shown)
  useEffect(() => {
    void keeper.refresh()
    const back = () => {
      if (document.visibilityState === 'visible') void keeper.refresh()
    }
    const stored = (event: StorageEvent) => {
      if (event.key === PLACE_TITLES_KEY) keeper.titlesStored()
    }
    document.addEventListener('visibilitychange', back)
    window.addEventListener('focus', back)
    window.addEventListener('storage', stored)
    return () => {
      document.removeEventListener('visibilitychange', back)
      window.removeEventListener('focus', back)
      window.removeEventListener('storage', stored)
    }
  }, [keeper])
  const { open, answered, nameLate, forget, rename, signOut, deleteAccount } = keeper
  return { ...shown, open, answered, nameLate, forget, rename, signOut, deleteAccount }
}
