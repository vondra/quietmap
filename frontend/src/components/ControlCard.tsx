import { useState } from 'react'
import { ChevronUp, Layers3 } from 'lucide-react'
import FloatingCard from './FloatingCard'
import LayerControlsBody, { type LayerControlsBodyProps } from './LayerControlsBody'

type ControlCardProps = LayerControlsBodyProps

export default function ControlCard(props: ControlCardProps) {
  const [collapsed, setCollapsed] = useState(false)

  if (collapsed) {
    return (
      <div className="flex justify-end pointer-events-auto">
        <button
          onClick={() => setCollapsed(false)}
          title="Show layers"
          className="flex items-center justify-center w-[29px] h-[29px] rounded border border-[rgba(0,0,0,0.2)] bg-white text-muted-foreground hover:bg-[#f4f4f4] cursor-pointer"
        >
          <Layers3 className="size-[18px]" strokeWidth={2} />
        </button>
      </div>
    )
  }

  return (
    <FloatingCard className="p-2.5 pointer-events-auto min-h-0 flex flex-col">
      <button
        onClick={() => setCollapsed(true)}
        title="Hide layers"
        className="flex items-center justify-between w-full px-1 py-0.5 rounded hover:bg-black/5 text-muted-foreground hover:text-foreground"
      >
        <span className="text-[11px] font-medium uppercase tracking-[0.08em]">Layers</span>
        <ChevronUp className="size-3.5" />
      </button>

      {/* Only the body scrolls, so the collapse header stays reachable when
          the column squeezes the card (short viewports). */}
      <div className="min-h-0 overflow-y-auto">
        <LayerControlsBody {...props} dividerSpacing="compact" />
      </div>
    </FloatingCard>
  )
}
