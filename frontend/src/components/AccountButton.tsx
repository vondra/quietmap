// The phone's account, beside the search field, where a visitor looks for it: a person while signed
// out, the address's first letter once signed in; it opens the account's panel under it (a desktop
// has it as the corner card's tab, ControlCard). Nothing on a server without the sign-in, or before it
// has answered. A press elsewhere or Escape closes it, Escape giving the focus back to the button.
import { useEffect, useRef } from 'react'
import { UserRound } from 'lucide-react'
import { isSignedIn } from '../lib/places-keeper'
import AccountPanel, { type AccountProps } from './AccountPanel'
import { FLOATING } from './ui/button'

export default function AccountButton({ open, onOpenChange, ...account }: AccountProps & {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const box = useRef<HTMLDivElement>(null)
  const button = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (!open) return
    const outside = (event: PointerEvent) => {
      if (!box.current?.contains(event.target as Node)) onOpenChange(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      onOpenChange(false)
      button.current?.focus()
    }
    document.addEventListener('pointerdown', outside)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('pointerdown', outside)
      document.removeEventListener('keydown', escape)
    }
  }, [open, onOpenChange])
  if (account.account !== null && !isSignedIn(account.account)) return null
  return (
    <div ref={box} data-testid="account" className="relative md:hidden">
      <button
        ref={button}
        type="button"
        aria-expanded={open}
        aria-label={isSignedIn(account.account) ? `Account: ${account.account.email}` : 'Sign in'}
        onClick={() => onOpenChange(!open)}
        className={`${FLOATING} grid size-11 place-items-center hover:bg-accent`}
      >
        {isSignedIn(account.account)
          ? <span className="grid size-6 place-items-center rounded-full bg-foreground text-xs text-background">{account.account.email[0]?.toUpperCase()}</span>
          : <UserRound className="size-5" />}
      </button>
      {open && (
        <div className={`${FLOATING} absolute right-0 top-full mt-2 w-[calc(100vw-2*var(--map-gutter))] p-3`}>
          <AccountPanel {...account} />
        </div>
      )}
    </div>
  )
}
