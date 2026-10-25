// The line under a place's name: after naming places while signed out, the offer to keep them on
// every device, until taken or dismissed for good; while signed in, a change the server did not take.
import { useState } from 'react'
import { X } from 'lucide-react'
import { isSignedIn, type AccountState } from '../lib/places-keeper'
import SignInForm from './SignInForm'

/** Set once the visitor dismissed the offer to sign in. */
const OFFER_DISMISSED_KEY = 'quietmap.sign-in-offer-dismissed'

function offerDismissed(): boolean {
  try {
    return localStorage.getItem(OFFER_DISMISSED_KEY) != null
  } catch {
    return false
  }
}

export default function PlaceNote({ account, named, notSaved }: {
  account: AccountState
  /** The open place has a name the visitor gave it. */
  named: boolean
  notSaved: boolean
}) {
  const [dismissed, setDismissed] = useState(offerDismissed)
  const [signingIn, setSigningIn] = useState(false)
  if (isSignedIn(account)) {
    return notSaved
      ? <p role="alert" className="px-2.5 pr-8 text-xs text-destructive">Not saved: the map could not be reached.</p>
      : null
  }
  if (account !== null || !named || dismissed) return null
  if (signingIn) return <div className="px-2.5 pr-8 pt-1"><SignInForm /></div>
  return (
    <p data-testid="sign-in-offer" className="flex items-center gap-1 px-2.5 pr-8 text-xs text-muted-foreground">
      <span>
        Keep your named places on all your devices:{' '}
        <button className="underline hover:text-foreground" onClick={() => setSigningIn(true)}>sign in</button>
      </span>
      <button
        aria-label="Do not offer again"
        className="ml-auto rounded p-0.5 hover:bg-black/5"
        onClick={() => {
          try {
            localStorage.setItem(OFFER_DISMISSED_KEY, '1')
          } catch {
            // Blocked storage: dismissed for this visit only.
          }
          setDismissed(true)
        }}
      >
        <X className="size-3" />
      </button>
    </p>
  )
}
