import { useState, useRef, useCallback } from 'react'
import LayerControlsBody, { type LayerControlsBodyProps } from './LayerControlsBody'

interface LayersPanelProps extends LayerControlsBodyProps {
  open: boolean
  onClose: () => void
}

export default function LayersPanel({ open, onClose, ...body }: LayersPanelProps) {
  const [dismissing, setDismissing] = useState(false)
  const [dragOffset, setDragOffset] = useState(0)
  const dragRef = useRef({ startY: 0, isDragging: false })

  const handleDismiss = useCallback(() => {
    setDismissing(true)
    setDragOffset(0)
    setTimeout(() => {
      onClose()
      setDismissing(false)
    }, 300)
  }, [onClose])

  const onTouchStart = useCallback((e: React.TouchEvent) => {
    dragRef.current = { startY: e.touches[0].clientY, isDragging: true }
  }, [])

  const onTouchMove = useCallback((e: React.TouchEvent) => {
    if (!dragRef.current.isDragging) return
    const deltaY = e.touches[0].clientY - dragRef.current.startY
    if (deltaY > 0) setDragOffset(deltaY)
  }, [])

  const onTouchEnd = useCallback((e: React.TouchEvent) => {
    if (!dragRef.current.isDragging) return
    const deltaY = e.changedTouches[0].clientY - dragRef.current.startY
    dragRef.current.isDragging = false
    // Only a real drag cancels the synthesized click — a tap on the handle
    // must still reach onClick (audit 2026-09-06: it never did on touch).
    if (Math.abs(deltaY) >= 10) {
      e.preventDefault()
      e.stopPropagation()
    }
    if (deltaY > 80) {
      handleDismiss()
    } else {
      setDragOffset(0)
    }
  }, [handleDismiss])

  if (!open) return null

  const transform = dismissing
    ? 'translateY(100%)'
    : dragOffset > 0
      ? `translateY(${dragOffset}px)`
      : undefined
  const transition = dragRef.current.isDragging ? 'none' : 'transform 0.3s ease-out'

  // Only the body scrolls, in an inner div — never the fixed + transformed
  // sheet itself: once that sheet's content overflowed (stay filters open),
  // Chrome stopped painting the WebGL map behind it — a white map until the
  // layer was switched off (audit 2026-09-06, reproduced on production). The
  // handle stays pinned above the scroller, like ControlCard's header.
  return (
    <div
      className="fixed z-[1002] bg-background shadow-xl border border-border bottom-0 left-0 right-0 rounded-t-xl"
      style={{ transform, transition }}
    >
      <div
        className="flex justify-center py-3 cursor-grab"
        onTouchStart={onTouchStart}
        onTouchMove={onTouchMove}
        onTouchEnd={onTouchEnd}
        onClick={handleDismiss}
        style={{ touchAction: 'none' }}
        role="button"
        aria-label="Drag to close"
      >
        <div className="w-10 h-1 bg-muted-foreground/30 rounded-full" />
      </div>

      <div className="max-h-[60vh] overflow-y-auto px-4 pb-3">
        <LayerControlsBody {...body} dividerSpacing="comfortable" />
      </div>
    </div>
  )
}
