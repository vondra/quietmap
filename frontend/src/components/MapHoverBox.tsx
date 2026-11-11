// The dark box a hover readout renders in, pinned above and right of the cursor.
import type { ReactNode } from 'react'
import type { MapHover } from '../lib/use-map-hover'

/** The dark monospace box a hover readout renders in, above and right of the cursor and inert to
 *  the pointer. */
export default function MapHoverBox({ hover, testId, children }: {
  hover: MapHover
  testId?: string
  children: ReactNode
}) {
  return (
    <div
      data-testid={testId}
      style={{
        position: 'fixed',
        left: hover.clientX + 14,
        // One box height of room, so the readout never leaves the screen at the top edge.
        top: Math.max(hover.clientY - 10, 32),
        transform: 'translateY(-100%)',
        pointerEvents: 'none',
        zIndex: 1002,
      }}
      className="rounded-md bg-zinc-900/95 text-zinc-50 border border-zinc-700/60 shadow-xl px-2 py-1 font-mono text-[11px] leading-snug"
    >
      {children}
    </div>
  )
}
