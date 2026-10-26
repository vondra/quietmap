// The desktop's card at the top right: the layers and the account as two tabs of one card, folded to
// its tabs while a popup is open (the popup has the column) and opened again when it closes (App). A
// tab opens the card on it, the arrow keys move between the tabs; the chevron folds it. Signed in, the
// account's tab shows the address's first letter; on a server without the sign-in, or before it has
// answered, the layers alone.
import type { KeyboardEvent, ReactNode } from 'react'
import { ChevronDown, ChevronUp, Layers3, UserRound } from 'lucide-react'
import FloatingCard from './FloatingCard'
import LayerControlsBody, { type LayerControlsBodyProps } from './LayerControlsBody'
import AccountPanel, { type AccountProps } from './AccountPanel'
import { isSignedIn } from '../lib/places-keeper'
import { ICON_BUTTON } from './ui/button'

export type CardTab = 'layers' | 'account'

export default function ControlCard({ tab, onTabChange, account, onSignOut, onDeleteAccount, ...layers }: LayerControlsBodyProps & AccountProps & {
  /** The tab open, or null while the card is folded. */
  tab: CardTab | null
  onTabChange: (tab: CardTab | null) => void
}) {
  const withAccount = account === null || isSignedIn(account)
  const tabs: CardTab[] = withAccount ? ['layers', 'account'] : ['layers']
  // The tab open while it is there (the account's goes with the sign-in), and the tab the keys are on:
  // the open one, or the first while the card is folded.
  const open = tab != null && tabs.includes(tab) ? tab : null
  const current = open ?? 'layers'
  const move = (event: KeyboardEvent) => {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft') return
    const next = tabs[(tabs.indexOf(current) + (event.key === 'ArrowRight' ? 1 : tabs.length - 1)) % tabs.length]
    onTabChange(next)
    document.getElementById(`card-tab-${next}`)?.focus()
  }
  const tabButton = (id: CardTab, label: string, icon: ReactNode) => (
    <button
      type="button"
      role="tab"
      id={`card-tab-${id}`}
      aria-selected={open === id}
      aria-controls={open === id ? 'card-panel' : undefined}
      tabIndex={current === id ? 0 : -1}
      onClick={() => onTabChange(id)}
      onKeyDown={move}
      className={`flex h-9 items-center gap-2 rounded-md px-2.5 text-sm font-medium hover:bg-accent ${open === id ? 'bg-accent text-foreground' : 'text-muted-foreground'}`}
    >
      {icon}
      {label}
    </button>
  )
  return (
    <FloatingCard className="pointer-events-auto flex min-h-0 flex-col">
      <div className="flex shrink-0 items-center p-1">
        {/* The tabs alone in their list; the fold beside it. */}
        <div role="tablist" aria-label="Map controls" className="flex items-center gap-1">
          {tabButton('layers', 'Layers', <Layers3 className="size-4 shrink-0" />)}
          {withAccount && tabButton('account', isSignedIn(account) ? 'Account' : 'Sign in', isSignedIn(account)
            ? <span className="grid size-5 place-items-center rounded-full bg-foreground text-[11px] text-background">{account.email[0]?.toUpperCase()}</span>
            : <UserRound className="size-4 shrink-0" />)}
        </div>
        <button
          type="button"
          aria-expanded={open != null}
          aria-label={open != null ? 'Fold' : 'Open'}
          onClick={() => onTabChange(open != null ? null : 'layers')}
          className={`${ICON_BUTTON} ml-auto`}
        >
          {open != null ? <ChevronUp className="size-4" /> : <ChevronDown className="size-4" />}
        </button>
      </div>
      {open === 'layers' && (
        // Only the body scrolls when the column squeezes the card (short viewports).
        <div role="tabpanel" id="card-panel" aria-labelledby="card-tab-layers" className="min-h-0 overflow-y-auto border-t border-border px-2.5 py-1.5 [scrollbar-gutter:stable] scroll-py-1.5">
          <LayerControlsBody {...layers} dividerSpacing="compact" />
        </div>
      )}
      {open === 'account' && (
        <div role="tabpanel" id="card-panel" aria-labelledby="card-tab-account" className="border-t border-border p-3">
          <AccountPanel account={account} onSignOut={onSignOut} onDeleteAccount={onDeleteAccount} />
        </div>
      )}
    </FloatingCard>
  )
}
