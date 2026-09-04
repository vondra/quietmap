import { Check } from 'lucide-react'

/**
 * Canonical multi-select chip — THE element for every "pick any subset"
 * choice on the site (isochrone Walk/Car, stays Hotels/Apartments; owner
 * 2026-07-29). The visible checkbox square is the point: a filled button
 * alone reads as an action, so clicking an active one "to select it"
 * surprisingly deselects. Single-choice groups should use radio-style
 * pills instead, not this.
 */
export function CheckChip({ checked, label, onToggle, testId }: {
  checked: boolean
  label: string
  onToggle: () => void
  testId?: string
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      data-testid={testId}
      aria-pressed={checked}
      // White in both states (owner 2026-07-29: the filled checkbox square is
      // signal enough; a tinted background just competes with the primary CTA).
      className={`inline-flex items-center gap-1.5 rounded-md border border-border bg-background px-2 py-1 text-xs cursor-pointer transition-colors hover:bg-black/5 ${
        checked ? 'text-foreground' : 'text-muted-foreground'
      }`}
    >
      <span
        aria-hidden
        className={`flex size-3.5 shrink-0 items-center justify-center rounded-[4px] border ${
          checked ? 'border-primary bg-primary text-primary-foreground' : 'border-muted-foreground/40 bg-background'
        }`}
      >
        {checked && <Check className="size-2.5" strokeWidth={3.5} />}
      </span>
      {label}
    </button>
  )
}
