import type { ReactNode } from 'react'
import type { MapHover } from '../lib/use-map-hover'

/**
 * The dark monospace box a hover readout renders in, pinned next to the cursor
 * and inert to the pointer. `above` and `below` are two distinct slots, so the
 * Lden readout (above) and the raster cell inspector (below) never stack.
 */
export default function MapHoverBox({ hover, placement, testId, children }: {
  hover: MapHover
  placement: 'above' | 'below'
  testId?: string
  children: ReactNode
}) {
  return (
    <div
      data-testid={testId}
      style={{
        position: 'fixed',
        left: hover.clientX + 14,
        // 'above' keeps one box height of room so the readout never leaves the screen at the top edge.
        top: placement === 'below' ? hover.clientY + 14 : Math.max(hover.clientY - 10, 32),
        transform: placement === 'above' ? 'translateY(-100%)' : undefined,
        pointerEvents: 'none',
        zIndex: 1002,
      }}
      className="rounded-md bg-zinc-900/95 text-zinc-50 border border-zinc-700/60 shadow-xl px-2 py-1 font-mono text-[11px] leading-snug"
    >
      {children}
    </div>
  )
}
