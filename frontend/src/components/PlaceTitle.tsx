// The open place's name over its answer, a field of its own: the name the visitor gave it, else its
// geocoded name proposed in grey; typing names it, leaving the field (Enter, a tap elsewhere) keeps
// the name, Escape leaves it as it was, and an emptied field gives the proposal back. Kept in this
// browser (`lib/recent-places.ts`).
import { Pencil } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

/** The longest name kept. */
const TITLE_MAX = 80

export default function PlaceTitle({ title, proposal, onRename }: {
  /** The name the visitor gave the place. */
  title: string | null
  /** The geocoded name, once it has come. */
  proposal: string | null
  /** Called with the new name, or null to give the proposal back, when it changed. */
  onRename: (title: string | null) => void
}) {
  const field = useRef<HTMLInputElement>(null)
  const [draft, setDraft] = useState(title ?? '')
  // A name changed elsewhere (in another tab) shows unless the visitor is typing here.
  useEffect(() => {
    if (document.activeElement !== field.current) setDraft(title ?? '')
  }, [title])
  // Escape leaves the field without keeping what was typed.
  const leaving = useRef(false)
  const keep = () => {
    if (leaving.current) {
      leaving.current = false
      setDraft(title ?? '')
      return
    }
    const name = draft.trim()
    if (name !== (title ?? '')) onRename(name || null)
    setDraft(name)
  }
  return (
    <label className="flex min-w-0 items-center gap-1.5 px-2.5 pr-8 pt-1.5">
      {/* 16 px on a phone: iOS zooms the page into a smaller field and keeps it zoomed. */}
      <input
        ref={field}
        data-testid="place-title"
        aria-label="Place name"
        className="min-w-0 flex-1 rounded bg-transparent text-base font-semibold text-foreground outline-none placeholder:text-muted-foreground focus:ring-1 focus:ring-border md:text-sm"
        placeholder={proposal ?? 'Name this place'}
        value={draft}
        maxLength={TITLE_MAX}
        enterKeyHint="done"
        onChange={event => setDraft(event.target.value)}
        onBlur={keep}
        onKeyDown={event => {
          if (event.key !== 'Enter' && event.key !== 'Escape') return
          leaving.current = event.key === 'Escape'
          event.currentTarget.blur()
        }}
      />
      <Pencil className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
    </label>
  )
}
