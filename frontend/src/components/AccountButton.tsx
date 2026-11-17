// The phone's account at the search field's end, where Google Maps has it (owner 2026-11-17): a person
// while signed out, the address's first letter once signed in; it opens the account's panel under the
// field (a desktop has it as the corner card's tab, ControlCard). Nothing on a server without the sign-in, or before it
// has answered. A press elsewhere or Escape closes it, Escape giving the focus back to the button.
import { useEffect, useRef } from 'react'
import { UserRound } from 'lucide-react'
import { isSignedIn } from '../lib/places-keeper'
import AccountPanel, { type AccountProps } from './AccountPanel'
import { FLOATING, ICON_BUTTON } from './ui/button'

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
        className={`${ICON_BUTTON} rounded-full`}
      >
        {isSignedIn(account.account)
          ? <span className="grid size-6 place-items-center rounded-full bg-foreground text-xs text-background">{account.account.email[0]?.toUpperCase()}</span>
          : <UserRound className="size-5" />}
      </button>
      {open && (
        <div className={`${FLOATING} absolute -right-1.5 top-full z-10 mt-2 w-[calc(100vw-2*var(--map-gutter))] p-3`}>
          <AccountPanel {...account} />
        </div>
      )}
    </div>
  )
}
