// The open place's name over its answer: the name the visitor gave it, else its geocoded name as a
// proposal; a tap renames it, and a name emptied or set back to the proposal gives the proposal back.
// Kept in this browser (`lib/recent-places.ts`).
import { Pencil } from 'lucide-react'
import { useRef, useState } from 'react'

/** The longest name kept. */
const TITLE_MAX = 80

export default function PlaceTitle({ title, proposal, onRename }: {
  /** The name the visitor gave the place. */
  title: string | null
  /** The geocoded name, once it has come. */
  proposal: string | null
  onRename: (title: string | null) => void
}) {
  const [draft, setDraft] = useState<string | null>(null)
  // Escape leaves the field without the blur that follows saving what was typed.
  const cancelled = useRef(false)
  const shown = title ?? proposal
  if (draft != null) {
    const commit = () => {
      if (cancelled.current) {
        cancelled.current = false
        return
      }
      const name = draft.trim()
      onRename(name === '' || name === proposal ? null : name)
      setDraft(null)
    }
    return (
      <form className="px-2.5 pr-8 pt-1.5" onSubmit={event => { event.preventDefault(); commit() }}>
        <input
          id="place-title"
          aria-label="Place name"
          className="w-full rounded-md border border-border bg-background px-1.5 py-0.5 text-sm font-semibold text-foreground"
          value={draft}
          maxLength={TITLE_MAX}
          enterKeyHint="done"
          autoFocus
          onChange={event => setDraft(event.target.value)}
          onBlur={commit}
          onKeyDown={event => {
            if (event.key !== 'Escape') return
            cancelled.current = true
            setDraft(null)
          }}
        />
      </form>
    )
  }
  return (
    <button
      type="button"
      data-testid="place-title"
      className="flex w-full min-w-0 items-center gap-1.5 px-2.5 pr-8 pt-1.5 text-left text-sm font-semibold"
      title={title != null ? 'Rename this place' : 'Name this place'}
      onClick={() => {
        cancelled.current = false
        setDraft(shown ?? '')
      }}
    >
      <span className={`truncate ${title != null ? 'text-foreground' : 'text-muted-foreground'}`}>{shown ?? 'Name this place'}</span>
      <Pencil className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
    </button>
  )
}
