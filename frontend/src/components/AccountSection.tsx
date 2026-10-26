// The account in the layer controls: "Sign in" while signed out, which opens the mailed-link form;
// once signed in, the account's address, "Sign out" and "Delete account". Nothing on a server
// without the sign-in, or before it has answered.
import { useState } from 'react'
import { UserRound } from 'lucide-react'
import { isSignedIn, type AccountState } from '../lib/places-keeper'
import SignInForm from './SignInForm'

export interface AccountSectionProps {
  account: AccountState
  onSignOut: () => Promise<boolean>
  onDeleteAccount: () => Promise<boolean>
}

const LINK = 'text-xs text-muted-foreground underline hover:text-foreground'

export default function AccountSection({ account, onSignOut, onDeleteAccount, dividerClass }: AccountSectionProps & { dividerClass: string }) {
  const [open, setOpen] = useState(false)
  const [failed, setFailed] = useState(false)
  if (account !== null && !isSignedIn(account)) return null
  const act = async (request: () => Promise<boolean>) => setFailed(!await request())
  return (
    <>
      <div className={dividerClass} />
      <div data-testid="account" className="flex flex-col gap-1.5 px-1 py-1">
        {isSignedIn(account) ? (
          <>
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
          </>
        ) : open ? (
          <SignInForm />
        ) : (
          <button
            onClick={() => setOpen(true)}
            className="flex w-full items-center gap-2.5 rounded-lg text-left text-sm text-muted-foreground hover:bg-black/5"
          >
            <UserRound className="size-4 shrink-0" />
            <span className="flex-1">Sign in to keep your places on all your devices</span>
          </button>
        )}
      </div>
    </>
  )
}
