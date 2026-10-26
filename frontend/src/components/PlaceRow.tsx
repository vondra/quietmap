// One place under the search field: the open place's row, or a row of the search's list. Its icon (the
// open place's pin, the history's clock, the geocoder's magnifier), its name over where it is, its
// whole-day loudness, and the actions at the end, as wide in every row so that nothing moves between
// them (two buttons on a touch screen, one on a mouse's): the open place's (the microphone and the
// camera on a phone), or a history row's cross that forgets it. A press anywhere else in the row opens
// it. The open place's name is the field it is renamed in, right where it stands: a press puts the
// caret into the name as written.
import { useRef, useState, type ReactNode } from 'react'
import { MapPin, X } from 'lucide-react'
import { TITLE_MAX } from '../lib/place-rules'
import { fmtSone } from '../utils/formatters'
import { ICON_BUTTON } from './ui/button'

type Point = { lat: number; lng: number }

// Touch screens a step larger: 16 px is also what keeps iOS from zooming into the name's field.
const NAME = 'text-base font-medium text-popover-foreground pointer-fine:text-sm'
const DETAIL = 'text-sm text-muted-foreground pointer-fine:text-xs'

/** The open place's name as a field: leaving it (Enter or a press elsewhere) keeps what differs from
 *  the name shown when the writing began, Escape keeps nothing, emptied it gives the geocoded name
 *  back. */
function NameField({ at, name, given, proposal, onRename }: {
  at: Point
  name: string
  given: boolean
  proposal: string | null
  onRename: (at: Point, title: string | null) => void
}) {
  const [draft, setDraft] = useState<{ from: string; text: string } | null>(null)
  const key = useRef<string | null>(null)
  return (
    <input
      data-testid="place-title"
      aria-label="Place name"
      title="Rename this place"
      className={`${NAME} -mx-1 w-full min-w-0 truncate rounded bg-transparent px-1 outline-none hover:bg-background/60 focus:bg-background focus:ring-1 focus:ring-ring`}
      placeholder={proposal ?? 'Name this place'}
      value={draft?.text ?? name}
      maxLength={TITLE_MAX}
      enterKeyHint="done"
      onFocus={() => setDraft({ from: name, text: name })}
      onChange={event => setDraft({ from: draft?.from ?? name, text: event.target.value })}
      onBlur={() => {
        const written = draft?.text.trim() ?? ''
        if (draft && key.current !== 'Escape') {
          if (written && written !== draft.from) onRename(at, written)
          else if (!written && given) onRename(at, null)
        }
        key.current = null
        setDraft(null)
      }}
      onKeyDown={event => {
        if (event.key !== 'Enter' && event.key !== 'Escape') return
        event.stopPropagation()
        key.current = event.key
        event.currentTarget.blur()
      }}
    />
  )
}

export default function PlaceRow({ at, name, given, proposal, detail, sone, highlighted, listed = false, onOpen, onRename, onForget, action, children }: {
  at: Point
  /** The name shown: the visitor's, else the geocoded one, else the coordinates. */
  name: string
  /** The visitor gave the place its name. */
  given: boolean
  /** The geocoded name, proposed in an emptied field. */
  proposal: string | null
  /** The second line: where the place is. */
  detail?: string | null
  sone: number | null
  /** The row the keys or the pointer are on, or the open place's: drawn as a dropdown's selected item. */
  highlighted: boolean
  /** An option of the search's list, which the keys move through from the field: its buttons are for
   *  the pointer alone. */
  listed?: boolean
  onOpen: () => void
  /** Makes the name its field (the open place's row). */
  onRename?: (at: Point, title: string | null) => void
  /** A cross at the end that forgets the place (a history row). */
  onForget?: () => void
  /** The actions at the end instead (the open place's row). */
  action?: ReactNode
  /** Before the name: what the row is (the open place's pin by default). */
  children?: ReactNode
}) {
  return (
    <div
      className={`group flex min-h-12 cursor-pointer items-center gap-1 pr-1 ${highlighted ? 'bg-accent' : 'hover:bg-accent'}`}
      onClick={event => {
        if (!(event.target as Element).closest('button, input')) onOpen()
      }}
    >
      <span className="grid place-items-center self-stretch pl-3 pr-2">
        {children ?? <MapPin className="size-4 shrink-0 text-muted-foreground" />}
      </span>
      <div className="min-w-0 flex-1 py-1.5">
        {onRename
          ? <NameField at={at} name={name} given={given} proposal={proposal} onRename={onRename} />
          : <button type="button" tabIndex={listed ? -1 : undefined} className={`${NAME} block w-full truncate text-left`} onClick={onOpen}>{name}</button>}
        {detail && <span className={`${DETAIL} block truncate`}>{detail}</span>}
      </div>
      {sone != null && sone > 0 && (
        <span className={`${NAME} shrink-0 px-1 font-semibold tabular-nums`}>
          {fmtSone(sone)} <span className="font-normal text-muted-foreground">sone</span>
        </span>
      )}
      <div className="flex w-8 shrink-0 justify-end pointer-coarse:w-16">
        {action ?? (onForget && (
          <button
            type="button"
            aria-label={`Forget ${name}`}
            title="Forget (Shift+Delete)"
            tabIndex={-1}
            // On a mouse's screen it shows on the row the pointer or the keys are on; on a touch screen
            // always, there being no pointer over a row.
            className={`${ICON_BUTTON} ${highlighted ? '' : 'pointer-fine:invisible pointer-fine:group-hover:visible'}`}
            onClick={onForget}
          >
            <X className="size-4" />
          </button>
        ))}
      </div>
    </div>
  )
}
