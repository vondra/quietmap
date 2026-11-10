// Keep the visible portion of a public panel through links and asynchronously arriving content.
import { useContext, useLayoutEffect, useRef, useState } from 'react'
import { PopupStateContext } from './usePopupState'

export function useViewScroll(panel: 'layers' | 'detail', active = true) {
  const shared = useContext(PopupStateContext)
  if (!shared) throw new Error('Panel scroll requires the map provider')
  const { scroll, update } = shared
  const [element, setElement] = useState<HTMLDivElement | null>(null)
  const target = scroll[panel] ?? 0
  const desired = useRef(target)
  const previous = useRef(target)
  const published = useRef<number | null>(null)
  const restoring = useRef(true)
  const restoreNow = useRef(() => {})
  if (previous.current !== target) {
    previous.current = desired.current = target
    restoring.current = published.current !== target
    published.current = null
  }
  useLayoutEffect(() => {
    if (!element || !active) return
    restoring.current = true
    let applied = element.scrollTop
    let settle: ReturnType<typeof setTimeout> | undefined
    const restore = () => {
      if (!restoring.current) return
      element.scrollTop = desired.current
      applied = element.scrollTop
    }
    restoreNow.current = restore
    const interact = () => { restoring.current = false }
    const publish = () => {
      clearTimeout(settle)
      if (restoring.current) return
      const top = Math.round(element.scrollTop)
      if (top === desired.current) return
      published.current = desired.current = top
      update(state => ({ scroll: { ...state.scroll, [panel]: top } }))
    }
    const moved = () => {
      if (restoring.current && Math.abs(element.scrollTop - applied) < 1) return
      restoring.current = false
      clearTimeout(settle)
      // Like the camera's moveend, publish a settled gesture without flooding browser history.
      settle = setTimeout(publish, 120)
    }
    // A streamed answer, lazy detail, image or font may make the requested offset possible later.
    const resized = new ResizeObserver(restore)
    const observe = () => {
      resized.disconnect(); resized.observe(element)
      for (const child of element.children) resized.observe(child)
      restore()
    }
    const changed = new MutationObserver(observe)
    changed.observe(element, { childList: true, subtree: true, characterData: true })
    observe()
    element.addEventListener('scroll', moved)
    element.addEventListener('scrollend', publish)
    for (const event of ['wheel', 'touchstart', 'pointerdown', 'keydown']) element.addEventListener(event, interact, { capture: true, passive: true })
    return () => {
      clearTimeout(settle)
      restoreNow.current = () => {}
      resized.disconnect(); changed.disconnect()
      element.removeEventListener('scroll', moved)
      element.removeEventListener('scrollend', publish)
      for (const event of ['wheel', 'touchstart', 'pointerdown', 'keydown']) element.removeEventListener(event, interact, true)
    }
  }, [element, panel, active, update])
  useLayoutEffect(() => {
    restoreNow.current()
  }, [element, active, target])
  return setElement
}
