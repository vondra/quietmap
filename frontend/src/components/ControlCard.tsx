// The desktop layer card, under its button beside the account (App), which folds it while a popup is
// open, so the popup has the column, and opens it again when the popup closes.
import FloatingCard from './FloatingCard'
import LayerControlsBody, { type LayerControlsBodyProps } from './LayerControlsBody'

export default function ControlCard(props: LayerControlsBodyProps) {
  return (
    // Only the body scrolls when the column squeezes the card (short viewports).
    <FloatingCard className="py-2.5 pointer-events-auto min-h-0 flex flex-col">
      <div className="min-h-0 overflow-y-auto px-2.5 [scrollbar-gutter:stable] scroll-py-1.5">
        <LayerControlsBody {...props} dividerSpacing="compact" />
      </div>
    </FloatingCard>
  )
}
