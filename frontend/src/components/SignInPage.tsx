// The page a mailed link opens (/signin#token): it shows the account the link signs in and signs in
// only at the visitor's press, as mail scanners open links too; then, when this browser keeps places
// of its own, it asks whether to add them to the account (they stay in the browser as well), and
// opens the map.
import { useEffect, useRef, useState } from 'react'
import { importPlaces, linkEmail, signInWithLink } from '../lib/account'
import { loadPlaceTitles, loadRecentPlaces } from '../lib/browser-places'
import { setDocumentTitle } from '../utils/page-title'
import { BUTTON, PRIMARY_BUTTON } from './ui/button'

/** The link's token, read once as the page opens, before it leaves the address bar. */
const TOKEN = window.location.hash.slice(1)

type Step =
  | { kind: 'checking' }
  | { kind: 'confirm', email: string }
  | { kind: 'busy' }
  | { kind: 'add', email: string, titles: number, recent: number }
  | { kind: 'not-added' }
  | { kind: 'expired' }
  | { kind: 'unreachable' }


const openMap = () => window.location.replace('/')
const browserPlaces = () => ({ titles: loadPlaceTitles() ?? [], recent: loadRecentPlaces() })
const plural = (count: number, one: string) => `${count} ${one}${count === 1 ? '' : 's'}`

export default function SignInPage() {
  const [step, setStep] = useState<Step>({ kind: 'checking' })
  // One press signs in: a second, before the first is answered, would find the link used.
  const pressed = useRef(false)
  useEffect(() => {
    // The token goes from the address bar and the history: a shared screen or a back step shows none.
    window.history.replaceState(null, '', '/signin')
    setDocumentTitle(['Sign in'])
    // A newer link opened in this tab changes only the fragment: the page starts again with it.
    const newer = () => window.location.reload()
    window.addEventListener('hashchange', newer)
    let open = true
    void (TOKEN ? linkEmail(TOKEN) : Promise.resolve(null)).then(email => {
      if (open) setStep(email ? { kind: 'confirm', email } : email === null ? { kind: 'expired' } : { kind: 'unreachable' })
    })
    return () => {
      open = false
      window.removeEventListener('hashchange', newer)
    }
  }, [])

  const signIn = async (email: string) => {
    if (pressed.current) return
    pressed.current = true
    setStep({ kind: 'busy' })
    const signedIn = await signInWithLink(TOKEN)
    if (!signedIn) return setStep({ kind: signedIn === false ? 'expired' : 'unreachable' })
    const { titles, recent } = browserPlaces()
    if (titles.length + recent.length === 0) return openMap()
    setStep({ kind: 'add', email, titles: titles.length, recent: recent.length })
  }
  const add = async (email: string) => {
    setStep({ kind: 'busy' })
    if (await importPlaces(email, browserPlaces())) openMap()
    else setStep({ kind: 'not-added' })
  }

  return (
    <main className="h-full overflow-y-auto bg-muted px-4 py-12 sm:py-20">
      <div className="mx-auto flex max-w-md flex-col gap-4 rounded-xl border border-border bg-background p-8">
        <a href="/" className="mb-2 flex items-center gap-2.5">
          <img src="/favicon.svg" alt="" className="size-10" />
          <span className="text-2xl font-semibold tracking-[-0.04em] text-foreground">quietmap.org</span>
        </a>
        {step.kind === 'checking' || step.kind === 'busy' ? (
          <p className="text-muted-foreground">One moment…</p>
        ) : step.kind === 'confirm' ? (
          <>
            <h1 className="text-xl font-semibold text-foreground">Sign in as {step.email}?</h1>
            <p className="text-muted-foreground">Your named places and the places you opened then follow you to every device you sign in on.</p>
            <div><button className={PRIMARY_BUTTON} onClick={() => signIn(step.email)}>Sign in</button></div>
          </>
        ) : step.kind === 'add' ? (
          <>
            <h1 className="text-xl font-semibold text-foreground">You are signed in.</h1>
            <p className="text-muted-foreground">
              This browser keeps {[step.titles && plural(step.titles, 'named place'), step.recent && `${plural(step.recent, 'place')} you opened`].filter(Boolean).join(' and ')}.
              Add them to your account?
            </p>
            <div className="flex gap-2">
              <button className={PRIMARY_BUTTON} onClick={() => add(step.email)}>Add them</button>
              <button className={BUTTON} onClick={openMap}>Keep them apart</button>
            </div>
          </>
        ) : step.kind === 'not-added' ? (
          <>
            <p className="text-muted-foreground">The places could not be added; they stay in this browser.</p>
            <div><button className={PRIMARY_BUTTON} onClick={openMap}>Open the map</button></div>
          </>
        ) : step.kind === 'unreachable' ? (
          <>
            <h1 className="text-xl font-semibold text-foreground">The map could not be reached.</h1>
            <p className="text-muted-foreground">Open the link from your mail again in a minute.</p>
          </>
        ) : (
          <>
            <h1 className="text-xl font-semibold text-foreground">This link has expired or was used already.</h1>
            <p className="text-muted-foreground">Ask for a new one on the map, from the sign-in at its top right.</p>
            <div><button className={PRIMARY_BUTTON} onClick={openMap}>Open the map</button></div>
          </>
        )}
      </div>
    </main>
  )
}
