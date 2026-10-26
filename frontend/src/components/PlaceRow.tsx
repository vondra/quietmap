// One place under the search field: the open place's row, or a row of the history of places opened.
// Its name and its whole-day loudness; a tap on it does `onOpen`; its pencil (or F2 in the list, from
// SearchBar) renames the place there; its cross, where it has one, forgets the place.
import { useRef, useState, type ReactNode } from 'react'
import { MapPin, Pencil, X } from 'lucide-react'
import { TITLE_MAX } from '../lib/place-rules'
import { fmtSone } from '../utils/formatters'

const ICON_BUTTON = 'grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-black/5 hover:text-foreground'

type Point = { lat: number; lng: number }

/** The field a place's name is written in, bound to the place and to the name shown when the writing
 *  began (a geocoded name coming meanwhile, or another place opening, changes neither): Enter or
 *  leaving keeps what differs from that name, Escape keeps nothing, an emptied field gives the
 *  geocoded name back. */
function NameField({ at, name, given, proposal, onRename, onDone }: {
  at: Point
  name: string
  given: boolean
  proposal: string | null
  onRename: (at: Point, title: string | null) => void
  /** The field is left, by Enter or Escape, or elsewhere. */
  onDone: (byKeys: boolean) => void
}) {
  const [writing] = useState({ at, from: name, given })
  const [text, setText] = useState(name)
  const key = useRef<'Enter' | 'Escape' | null>(null)
  const keep = () => {
    const written = text.trim()
    if (key.current === 'Escape') {
      // Left as it was.
    } else if (written && written !== writing.from) onRename(writing.at, written)
    else if (!written && writing.given) onRename(writing.at, null)
    onDone(key.current != null)
  }
  return (
    // 16 px on a touch screen: iOS zooms the page into a smaller field and keeps it zoomed.
    <input
      data-testid="place-title"
      aria-label="Place name"
      autoFocus
      className="ml-2 h-8 min-w-0 flex-1 rounded border border-input bg-background px-2 text-base font-medium text-foreground outline-none focus:border-ring pointer-fine:text-sm"
      placeholder={proposal ?? 'Name this place'}
      value={text}
      maxLength={TITLE_MAX}
      enterKeyHint="done"
      onFocus={event => event.currentTarget.select()}
      onChange={event => setText(event.target.value)}
      onBlur={keep}
      onKeyDown={event => {
        event.stopPropagation()
        if (event.key !== 'Enter' && event.key !== 'Escape') return
        key.current = event.key
        event.currentTarget.blur()
      }}
    />
  )
}

export default function PlaceRow({ at, name, given, proposal, detail, sone, highlighted, listed = false, editing, onEditingChange, onOpen, onRename, onForget, children }: {
  at: Point
  /** The name shown: the visitor's, else the geocoded one, else the coordinates. */
  name: string
  /** The visitor gave the place its name. */
  given: boolean
  /** The geocoded name, proposed in an emptied field. */
  proposal: string | null
  /** A second line: where the place is. */
  detail?: string | null
  sone: number | null
  /** The row the keys or the pointer are on, or the open place's: drawn as a dropdown's selected item. */
  highlighted: boolean
  /** An option of the search's list, which the keys move through from the field (F2 renames, Shift+Delete
   *  forgets): its buttons are for the pointer alone. */
  listed?: boolean
  /** Its name is being written. */
  editing: boolean
  /** Writing begins, or ends (by Enter or Escape, or elsewhere). */
  onEditingChange: (editing: boolean, byKeys: boolean) => void
  onOpen: () => void
  onRename: (at: Point, title: string | null) => void
  onForget?: () => void
  /** Before the name: what the row is (the open place's pin by default). */
  children?: ReactNode
}) {
  return (
    <div className={`group flex min-h-11 items-center gap-1 pr-1 ${highlighted ? 'bg-accent' : 'hover:bg-accent'}`}>
      {editing ? (
        <NameField at={at} name={name} given={given} proposal={proposal} onRename={onRename} onDone={byKeys => onEditingChange(false, byKeys)} />
      ) : (
        <button type="button" tabIndex={listed ? -1 : undefined} className="flex min-w-0 flex-1 items-center gap-3 py-1.5 pl-3 text-left" onClick={onOpen}>
          {children ?? <MapPin className="size-4 shrink-0 text-muted-foreground" />}
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-medium text-popover-foreground">{name}</span>
            {detail && <span className="block truncate text-xs text-muted-foreground">{detail}</span>}
          </span>
          {sone != null && sone > 0 && <b className="shrink-0 text-xs tabular-nums text-popover-foreground">{fmtSone(sone)} sone</b>}
        </button>
      )}
      {!editing && (
        <button
          type="button"
          aria-label={`Rename ${name}`}
          title={listed ? 'Rename (F2)' : 'Rename'}
          tabIndex={listed ? -1 : undefined}
          className={ICON_BUTTON}
          onClick={() => onEditingChange(true, false)}
        >
          <Pencil className="size-3.5" />
        </button>
      )}
      {!editing && onForget && (
        <button
          type="button"
          aria-label={`Forget ${name}`}
          title="Forget (Shift+Delete)"
          tabIndex={-1}
          // On a mouse's screen it shows on the row the pointer or the keys are on.
          className={`${ICON_BUTTON} ${highlighted ? '' : 'pointer-fine:invisible pointer-fine:group-hover:visible'}`}
          onClick={onForget}
        >
          <X className="size-3.5" />
        </button>
      )}
    </div>
  )
}
