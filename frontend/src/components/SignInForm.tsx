// The sign-in by a mailed link: an e-mail field and "Send me a link", then where to look. No password:
// the link signs in (lib/account.ts). A plain form with a named, labelled e-mail field and a submit
// button, as browsers want to offer a saved address: the field is left for the visitor to press, as
// Chrome offers its addresses on a press, not on a focus a script gave.
import { useId, useState } from 'react'
import { mailSignInLink } from '../lib/account'
import { PRIMARY_BUTTON, TEXT_LINK } from './ui/button'

export default function SignInForm() {
  // The phone's panel and the desktop's tab may both hold a form, across a resize.
  const field = useId()
  const [email, setEmail] = useState('')
  const [sending, setSending] = useState(false)
  const [sent, setSent] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  if (sent) {
    return (
      <p data-testid="sign-in-sent" className="text-sm text-muted-foreground">
        A link to sign in is on its way to <span className="font-medium text-foreground">{sent}</span>. Open it on any
        device; it works once.
      </p>
    )
  }
  return (
    <form
      name="sign-in"
      className="flex flex-col gap-2"
      onSubmit={async event => {
        event.preventDefault()
        setSending(true)
        const failure = await mailSignInLink(email)
        setSending(false)
        setError(failure)
        if (failure == null) setSent(email.trim())
      }}
    >
      <label htmlFor={field} className="text-sm font-medium text-foreground">Sign in with your e-mail</label>
      <div className="flex gap-2">
        {/* 16 px on a touch screen: iOS zooms the page into a smaller field and keeps it zoomed. */}
        <input
          id={field}
          name="email"
          type="email"
          required
          autoComplete="email"
          inputMode="email"
          placeholder="you@example.com"
          value={email}
          onChange={event => setEmail(event.target.value)}
          className="h-8 min-w-0 flex-1 rounded-md border border-input bg-background px-2 text-base text-foreground focus:border-ring focus:outline-none pointer-fine:text-sm"
        />
        <button type="submit" disabled={sending} className={PRIMARY_BUTTON}>Send me a link</button>
      </div>
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
      <p className="text-xs text-muted-foreground">
        Your named places and the places you opened are then kept with your account, on every device you sign
        in on. <a href="/about/terms" className={TEXT_LINK}>Privacy</a>
      </p>
    </form>
  )
}
