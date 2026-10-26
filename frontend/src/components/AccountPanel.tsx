// The account's panel: signed out, the mailed-link form; signed in, the address, "Sign out" and
// "Delete account". The desktop card's tab (ControlCard) and the phone's account button show it.
import { useState } from 'react'
import { UserRound } from 'lucide-react'
import { isSignedIn, type AccountState } from '../lib/places-keeper'
import SignInForm from './SignInForm'
import { BUTTON } from './ui/button'

export interface AccountProps {
  account: AccountState
  onSignOut: () => Promise<boolean>
  onDeleteAccount: () => Promise<boolean>
}

export default function AccountPanel({ account, onSignOut, onDeleteAccount }: AccountProps) {
  const [failed, setFailed] = useState(false)
  if (!isSignedIn(account)) return <SignInForm />
  const act = async (request: () => Promise<boolean>) => setFailed(!await request())
  return (
    <div className="flex flex-col gap-2">
      <p className="flex items-center gap-2 text-sm text-foreground">
        <UserRound className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 truncate">{account.email}</span>
      </p>
      <div className="flex gap-2">
        <button className={BUTTON} onClick={() => act(onSignOut)}>Sign out</button>
        <button
          className={BUTTON}
          onClick={() => {
            if (window.confirm('Delete your account and the places, recordings and photos kept with it? Places this browser kept while signed out stay in it.')) void act(onDeleteAccount)
          }}
        >
          Delete account
        </button>
      </div>
      {failed && <p role="alert" className="text-xs text-destructive">The map could not be reached. Try again.</p>}
    </div>
  )
}
