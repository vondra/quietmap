// Public popup disclosures use the same state in the desktop card, phone sheet and copied address.
import { createContext, useCallback, useContext } from 'react'
import type { PopupState, UrlState } from '../lib/url-state'
import type { UrlUpdate } from './useUrlState'
export const PopupStateContext = createContext<{ popup: PopupState; scroll: UrlState['scroll']; update: (patch: UrlUpdate) => void } | null>(null)
export function usePopupState<T extends boolean | number | string | null>(key: string, fallback: T) {
  const shared = useContext(PopupStateContext)
  if (!shared) throw new Error('Popup state requires the map provider')
  const { popup, update } = shared
  const set = useCallback((next: T) => update(state => {
    const popup = { ...state.popup }
    if (next === fallback || next === null) delete popup[key]
    else popup[key] = next
    return { popup }
  }), [key, fallback, update])
  return [(popup[key] ?? fallback) as T, set] as const
}
