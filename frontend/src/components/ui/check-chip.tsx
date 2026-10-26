// The chip for a choice of any subset (the reachable area's walk and car): a visible checkbox square,
// so that a filled button is not read as an action; white in both states, the square signal enough
// (owner 2026-07-29).
import { Check } from 'lucide-react'

export function CheckChip({ checked, label, onToggle }: { checked: boolean; label: string; onToggle: () => void }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-pressed={checked}
      className={`inline-flex h-8 items-center gap-1.5 rounded-md border border-border bg-background px-2.5 text-sm hover:bg-accent ${checked ? 'text-foreground' : 'text-muted-foreground'}`}
    >
      <span
        aria-hidden
        className={`flex size-3.5 shrink-0 items-center justify-center rounded-[4px] border ${checked ? 'border-primary bg-primary text-primary-foreground' : 'border-muted-foreground/40 bg-background'}`}
      >
        {checked && <Check className="size-2.5" strokeWidth={3.5} />}
      </span>
      {label}
    </button>
  )
}
