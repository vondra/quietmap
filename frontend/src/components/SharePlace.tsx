// Share the open place through the device's chooser, or copy its link when there is no chooser.
import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Check, Share2 } from 'lucide-react'
import { BUTTON, ICON_BUTTON } from './ui/button'

export default function SharePlace({ getUrl }: { getUrl: () => string }) {
  const sharing = useRef(false)
  const [copied, setCopied] = useState(false)
  const [manualLink, setManualLink] = useState('')
  const dialog = useRef<HTMLDialogElement>(null)

  useEffect(() => {
    if (!copied) return
    const timer = setTimeout(() => setCopied(false), 2500)
    return () => clearTimeout(timer)
  }, [copied])

  useEffect(() => {
    if (manualLink) dialog.current?.showModal()
  }, [manualLink])

  async function share() {
    if (sharing.current) return
    sharing.current = true
    const url = getUrl()
    setCopied(false)
    setManualLink('')
    try {
      if (navigator.share) {
        await navigator.share({ title: 'Quiet Map', url })
      } else {
        await navigator.clipboard.writeText(url)
        setCopied(true)
      }
    } catch (error) {
      if (!(error instanceof DOMException && error.name === 'AbortError')) {
        setManualLink(url)
      }
    } finally {
      sharing.current = false
    }
  }

  return (
    <div className="relative">
      <button type="button" onClick={share} aria-label="Share this place" title="Share this place"
        className={`${ICON_BUTTON} max-md:size-11 pointer-coarse:size-11`}>
        {copied ? <Check className="size-4 max-md:size-5 pointer-coarse:size-5" /> : <Share2 className="size-4 max-md:size-5 pointer-coarse:size-5" />}
      </button>
      <div role="status" className={copied ? 'absolute right-0 top-full z-20 w-max rounded-md border border-border bg-popover p-2 text-xs text-popover-foreground shadow-lg' : 'sr-only'}>
        {copied ? 'Link copied' : ''}
      </div>
      {createPortal(<dialog ref={dialog} aria-label="Share link" onClose={() => setManualLink('')}
        className="m-auto w-80 max-w-[calc(100vw-2rem)] rounded-lg border border-border bg-popover p-4 text-sm text-popover-foreground shadow-lg backdrop:bg-foreground/20">
        <label>Copy this link to share the place.
          <input value={manualLink} readOnly
            onFocus={event => event.currentTarget.select()}
            className="mt-3 block w-full rounded border border-input bg-background p-2 text-base pointer-fine:text-sm" />
        </label>
        <form method="dialog" className="mt-3 flex justify-end">
          <button className={BUTTON}>Done</button>
        </form>
      </dialog>, document.body)}
    </div>
  )
}
