// Text on one line that uses the room it has and fades out where its box cuts it, instead of ending
// in an ellipsis; shown whole it wraps over lines.
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'

export function FadingText({ children, whole = false, className = '' }: {
  children: ReactNode
  /** Wraps it over lines instead (an opened row's name). */
  whole?: boolean
  className?: string
}) {
  const ref = useRef<HTMLSpanElement>(null)
  const [cut, setCut] = useState(false)
  // Measured after every change of the text and of the box: the fade marks text that goes on.
  useLayoutEffect(() => {
    const element = ref.current
    if (!element) return
    const measure = () => setCut(element.scrollWidth > element.clientWidth)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [children, whole])
  const line = whole ? 'break-words' : `overflow-hidden whitespace-nowrap ${cut ? 'fade-out-end' : ''}`
  return <span ref={ref} className={`block min-w-0 ${line} ${className}`}>{children}</span>
}
