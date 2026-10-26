// The account at the map's top right, beside the layers, where a visitor looks for it: "Sign in" while
// signed out, which opens the mailed-link form under it; once signed in, the address's first letter,
// which opens the address, "Sign out" and "Delete account". Nothing on a server without the sign-in,
// or before it has answered. The history under the search field opens it too (App); a press
// elsewhere or Escape closes it.
import { useEffect, useRef, useState } from 'react'
import { UserRound } from 'lucide-react'
import { isSignedIn, type AccountState } from '../lib/places-keeper'
import SignInForm from './SignInForm'

export interface AccountProps {
  account: AccountState
  onSignOut: () => Promise<boolean>
  onDeleteAccount: () => Promise<boolean>
}

const LINK = 'text-xs text-muted-foreground underline hover:text-foreground'

export default function AccountButton({ account, onSignOut, onDeleteAccount, open, onOpenChange }: AccountProps & {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const box = useRef<HTMLDivElement>(null)
  const button = useRef<HTMLButtonElement>(null)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    if (!open) return
    const outside = (event: PointerEvent) => {
      if (!box.current?.contains(event.target as Node)) onOpenChange(false)
    }
    // Escape gives the focus back to the button, the field that had it going with the form.
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
  if (account !== null && !isSignedIn(account)) return null
  const act = async (request: () => Promise<boolean>) => {
    const done = await request()
    setFailed(!done)
    if (done) onOpenChange(false)
  }
  return (
    // Beside the search bar on a phone (`.map-top-bar` leaves it the room).
    <div ref={box} data-testid="account" className="relative">
      <button
        ref={button}
        type="button"
        aria-expanded={open}
        aria-label={isSignedIn(account) ? `Account: ${account.email}` : 'Sign in'}
        title={isSignedIn(account) ? account.email : 'Sign in'}
        onClick={() => {
          setFailed(false)
          onOpenChange(!open)
        }}
        className="flex h-11 min-w-11 items-center justify-center gap-2 rounded-lg border border-input bg-background px-3 text-sm font-medium text-foreground shadow-lg hover:bg-muted md:h-9 md:rounded-md"
      >
        {isSignedIn(account)
          ? <span className="grid size-6 place-items-center rounded-full bg-foreground text-xs text-background">{account.email[0]?.toUpperCase()}</span>
          : <><UserRound className="size-4 shrink-0" /><span className="hidden md:inline">Sign in</span></>}
      </button>
      {open && (
        <div className="absolute right-0 top-full mt-2 w-[min(20rem,calc(100vw-2*var(--map-gutter)))] rounded-md border border-black/5 bg-white p-3 shadow-lg">
          {isSignedIn(account) ? (
            <div className="flex flex-col gap-1.5">
              <p className="flex items-center gap-2 text-sm text-foreground">
                <UserRound className="size-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 truncate">{account.email}</span>
              </p>
              <div className="flex gap-3 pl-6">
                <button className={LINK} onClick={() => act(onSignOut)}>Sign out</button>
                <button
                  className={LINK}
                  onClick={() => {
                    if (window.confirm('Delete your account and the places, recordings and photos kept with it? Places this browser kept while signed out stay in it.')) void act(onDeleteAccount)
                  }}
                >
                  Delete account
                </button>
              </div>
              {failed && <p role="alert" className="pl-6 text-xs text-destructive">The map could not be reached. Try again.</p>}
            </div>
          ) : (
            <SignInForm />
          )}
        </div>
      )}
    </div>
  )
}
