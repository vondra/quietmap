// The sign-in by a mailed link: an e-mail field and "Send me a link", then where to look. No password:
// the link signs in (lib/account.ts).
import { useState } from 'react'
import { mailSignInLink } from '../lib/account'

export default function SignInForm() {
  const [email, setEmail] = useState('')
  const [sending, setSending] = useState(false)
  const [sent, setSent] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  if (sent) {
    return (
      <p data-testid="sign-in-sent" className="text-xs text-muted-foreground">
        A link to sign in is on its way to <span className="font-medium text-foreground">{sent}</span>. Open it on any
        device; it works once.
      </p>
    )
  }
  return (
    <form
      className="flex flex-col gap-1.5"
      onSubmit={async event => {
        event.preventDefault()
        setSending(true)
        const failure = await mailSignInLink(email)
        setSending(false)
        setError(failure)
        if (failure == null) setSent(email.trim())
      }}
    >
      <div className="flex gap-1.5">
        {/* 16 px on a touch screen: iOS zooms the page into a smaller field and keeps it zoomed. The
            field takes the focus as the form opens (AccountButton). */}
        <input
          type="email"
          required
          autoFocus
          autoComplete="email"
          aria-label="E-mail"
          placeholder="Your e-mail"
          value={email}
          onChange={event => setEmail(event.target.value)}
          className="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1 text-base text-foreground focus:border-ring focus:outline-none pointer-fine:text-sm"
        />
        <button
          type="submit"
          disabled={sending}
          className="shrink-0 rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground hover:opacity-90 disabled:opacity-50"
        >
          Send me a link
        </button>
      </div>
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
      <p className="text-[11px] text-muted-foreground">
        Your named and recent places are then kept with your account, on every device you sign in on.{' '}
        <a href="/about/terms" className="underline">Privacy</a>
      </p>
    </form>
  )
}
