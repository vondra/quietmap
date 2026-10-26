// A signed-in visitor's recordings and photos of the open place (lib/media-keeper.ts), at its row under
// the search field. On a touch screen, where a visitor stands at the place, the row carries a microphone
// and a camera (MediaButtons; a desktop has neither). The microphone first says in two sentences how to
// record, and its OK starts the take, which runs until Stop. Under the row (PlaceMedia): that, the take
// while it runs, what is on its way or was not kept, and the place's items, each opened by a tap to play
// or look at, from the page until the account keeps it.
import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { Camera, CircleStop, Image as ImageIcon, LoaderCircle, Mic, Play, X } from 'lucide-react'
import { mediaUrl } from '../lib/media'
import type { MediaKeeper } from '../lib/media-keeper'
import { samePlace } from '../lib/place-rules'
import { BUTTON, ICON_BUTTON, PRIMARY_BUTTON, TEXT_LINK } from './ui/button'

const CHIP = 'flex h-8 items-center gap-1.5 rounded-md border border-border px-2.5 text-sm hover:bg-accent'

/** m:ss */
const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`
const when = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })

/** The microphone and the camera in the open place's row, on a touch screen only. */
export function MediaButtons({ media }: { media: MediaKeeper }) {
  const shown = useSyncExternalStore(media.subscribe, media.shown)
  const camera = useRef<HTMLInputElement>(null)
  if (!shown.account || !shown.place) return null
  return (
    <div className="hidden pointer-coarse:flex">
      <button aria-label="Record the sound here" className={ICON_BUTTON} disabled={shown.recording != null} onClick={() => media.ask(true)}>
        <Mic className="size-5" />
      </button>
      <button aria-label="Add a photo of this place" className={ICON_BUTTON} onClick={() => camera.current?.click()}>
        <Camera className="size-5" />
      </button>
      <input
        ref={camera}
        data-testid="photo-input"
        type="file"
        accept="image/*"
        hidden
        onChange={event => {
          const file = event.target.files?.[0]
          event.target.value = ''
          if (file) void media.addPhoto(file)
        }}
      />
    </div>
  )
}

/** What is open, by its id (an upload kept stays open as its item), with the address it plays or shows
 *  from, made in the tap that opened it: the page's own copy for an upload. */
type Opened = { id: string, kind: 'sound' | 'photo', src: string }

export default function PlaceMedia({ media }: { media: MediaKeeper }) {
  const shown = useSyncExternalStore(media.subscribe, media.shown)
  const [opened, setOpened] = useState<Opened | null>(null)
  const player = useRef<HTMLAudioElement>(null)
  useEffect(() => () => {
    if (opened?.src.startsWith('blob:')) URL.revokeObjectURL(opened.src)
  }, [opened])
  const { place, recording } = shown
  if (!shown.account || !place) return null
  const pending = shown.pending.filter(upload => upload.account === shown.account && samePlace(upload, place))
  const items = shown.items ?? []
  if (!shown.asking && !recording && !shown.problem && pending.length === 0 && items.length === 0) return null
  const upload = pending.find(each => each.id === opened?.id)
  const item = upload ? undefined : items.find(each => each.id === opened?.id)
  const close = () => {
    player.current?.pause()
    setOpened(null)
  }
  /** Opens an upload or an item, or closes it; a recording starts playing in the tap, the one moment
   *  iOS lets a page play sound. */
  const toggle = (each: { id: string, kind: 'sound' | 'photo' }, source: string | Blob) => {
    if (opened?.id === each.id) return close()
    const src = typeof source === 'string' ? source : URL.createObjectURL(source)
    setOpened({ id: each.id, kind: each.kind, src })
    const audio = player.current
    if (!audio) return
    audio.pause()
    if (each.kind !== 'sound') return
    audio.src = src
    void audio.play().catch(() => {})
  }
  const shownKind = (upload ?? item) && opened?.kind
  return (
    <div data-testid="place-media" className="flex flex-col gap-2 border-t border-border px-3 py-2 text-sm text-muted-foreground">
      {shown.asking && (
        <div className="flex flex-col gap-2">
          <p className="text-foreground">
            Stand still and hold the phone out, its bottom edge towards the noise and your fingers off the
            microphone. It records until you press Stop, voices nearby too.
          </p>
          <div className="flex justify-end gap-2">
            <button className={BUTTON} onClick={() => media.ask(false)}>Cancel</button>
            <button className={PRIMARY_BUTTON} onClick={() => media.record()}>OK, record</button>
          </div>
        </div>
      )}
      {recording && (
        <div className="flex items-center gap-3">
          {recording.phase === 'asking' && <span className="flex-1">Waiting for the microphone…</span>}
          {recording.phase === 'held' && (
            <button className={`${TEXT_LINK} flex-1 text-left font-medium text-foreground`} onClick={media.resumeRecording}>Tap to start recording</button>
          )}
          {recording.phase === 'recording' && (
            <span data-testid="recording-time" className="flex flex-1 items-center gap-2 font-medium text-foreground tabular-nums">
              <span className="size-2.5 animate-pulse rounded-full bg-red-600" aria-hidden="true" />
              {clock(recording.seconds)}
            </span>
          )}
          <button className={PRIMARY_BUTTON} onClick={media.stopRecording}>
            <CircleStop className="size-4" /> {recording.phase === 'recording' ? 'Stop' : 'Cancel'}
          </button>
        </div>
      )}
      {shown.problem && <p role="alert" className="text-destructive">{shown.problem}</p>}
      {(pending.length > 0 || items.length > 0) && (
        <div className="flex flex-wrap gap-1.5">
          {pending.map(each => (
            <button key={each.id} aria-pressed={opened?.id === each.id} className={`${CHIP} ${each.problem ? 'text-destructive' : ''}`} onClick={() => toggle(each, each.body)}>
              {each.problem ? 'Not kept' : <><LoaderCircle className="size-4 animate-spin" /> Saving</>}
            </button>
          ))}
          {items.map(each => (
            <button key={each.id} data-testid="media-item" aria-pressed={opened?.id === each.id} title={when(each.takenAt)} className={`${CHIP} text-foreground`} onClick={() => toggle(each, mediaUrl(each.id))}>
              {each.kind === 'sound'
                ? <><Play className="size-4" /> {clock(each.durationS ?? 0)}</>
                : <><ImageIcon className="size-4" /> Photo</>}
            </button>
          ))}
        </div>
      )}
      {/* Always there, so that a tap can start it. */}
      <audio ref={player} controls preload="none" hidden={shownKind !== 'sound'} className="h-9 w-full" />
      {shownKind === 'photo' && <img src={opened?.src} alt="Photo of this place" className="max-h-[max(8rem,calc(50dvh-14rem))] w-full rounded object-contain" />}
      {/* While on its way it plays from the page; a save that never ends can be sent again or given up. */}
      {upload && (
        <p className="flex flex-wrap items-center gap-x-3 text-xs">
          {upload.problem ? <span className="flex-1 text-destructive">Not kept: {upload.problem}</span> : <span className="flex-1">Saving…</span>}
          <button className={TEXT_LINK} onClick={() => media.retry(upload.id)}>Send again</button>
          <button
            className={TEXT_LINK}
            onClick={() => {
              close()
              media.discard(upload.id)
            }}
          >
            Discard
          </button>
          <button aria-label="Close" className={ICON_BUTTON} onClick={close}><X className="size-4" /></button>
        </p>
      )}
      {item && (
        <p className="flex items-center gap-3 text-xs">
          <span className="flex-1">{when(item.takenAt)}</span>
          <button
            className={TEXT_LINK}
            onClick={() => {
              if (!window.confirm(item.kind === 'sound' ? 'Delete this recording?' : 'Delete this photo?')) return
              close()
              void media.remove(item.id)
            }}
          >
            Delete
          </button>
          <button aria-label="Close" className={ICON_BUTTON} onClick={close}><X className="size-4" /></button>
        </p>
      )}
    </div>
  )
}
