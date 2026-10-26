// A signed-in visitor's recordings and photos of the open place (lib/media-keeper.ts), at the popup's
// head: a microphone, a camera and how to hold the phone; under them the recording while it runs, what
// was not kept, and the place's items, each opened by a tap. The desktop card and the phone sheet both
// draw them, from the one keeper. Nothing while signed out.
import { useRef, useState, useSyncExternalStore } from 'react'
import { Camera, CircleStop, Image as ImageIcon, Info, LoaderCircle, Mic, Play } from 'lucide-react'
import { mediaUrl } from '../lib/media'
import type { MediaKeeper } from '../lib/media-keeper'
import { samePlace } from '../lib/place-rules'
import { TAKE_S } from '../lib/recorder'

const ICON_BUTTON = 'grid size-8 shrink-0 place-items-center rounded-md text-muted-foreground hover:bg-black/5 hover:text-foreground disabled:opacity-40'
const LINK = 'underline hover:text-foreground'
const CHIP = 'flex h-7 items-center gap-1 rounded-md border border-border px-2 hover:bg-black/5'

/** How to hold the phone (the research of 2026-10-25: NIOSH, NoiseCapture, Hush City). */
const GUIDE = 'Stand still and quiet, a few metres from walls and cars. Hold the phone at arm’s length, chest to head '
  + 'high, its bottom edge towards the loudest sound (upright when sound comes from everywhere), fingers and case off '
  + 'the microphone holes. Not in wind or rain: wind on the microphone is louder than the place. Record 30 seconds, '
  + 'or a few minutes when trains, planes or gusts come and go. Voices nearby are recorded too; only you can open what '
  + 'you keep. A phone hears little below about 35 dB(A).'

/** m:ss */
const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`
const when = (ms: number) => new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })

export default function PlaceMedia({ media }: { media: MediaKeeper }) {
  const shown = useSyncExternalStore(media.subscribe, media.shown)
  const camera = useRef<HTMLInputElement>(null)
  // The item or upload open, as `item:<id>` or `upload:<id>`: an upload kept becomes an item closed.
  const [opened, setOpened] = useState<string | null>(null)
  const { place, recording } = shown
  if (!shown.account || !place) return null
  const pending = shown.pending.filter(upload => upload.account === shown.account && samePlace(upload, place))
  const items = shown.items ?? []
  const item = items.find(each => `item:${each.id}` === opened)
  const upload = pending.find(each => `upload:${each.id}` === opened)
  const toggle = (key: string) => setOpened(opened === key ? null : key)
  return (
    <div data-testid="place-media" className="flex flex-col gap-1.5 px-2.5 pt-1 text-xs text-muted-foreground">
      <div className="-ml-1.5 flex items-center gap-0.5">
        <button aria-label="Record the sound here" className={ICON_BUTTON} disabled={recording != null} onClick={() => media.record()}>
          <Mic className="size-4" />
        </button>
        <button aria-label="Add a photo of this place" className={ICON_BUTTON} onClick={() => camera.current?.click()}>
          <Camera className="size-4" />
        </button>
        <button aria-label="How to record" aria-expanded={shown.guide} className={ICON_BUTTON} onClick={media.toggleGuide}>
          <Info className="size-4" />
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
      {recording && (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          {recording.phase === 'asking' && <span>Waiting for the microphone…</span>}
          {recording.phase === 'held' && (
            <button className="font-medium text-foreground underline" onClick={media.resumeRecording}>Tap to start recording</button>
          )}
          {recording.phase === 'recording' && (
            <span data-testid="recording-time" className="flex items-center gap-1.5 font-medium text-foreground tabular-nums">
              <span className="size-2 animate-pulse rounded-full bg-red-600" aria-hidden="true" />
              {clock(recording.seconds)} / {clock(recording.limitS)}
            </span>
          )}
          {recording.phase === 'recording' && recording.limitS === TAKE_S && (
            <button className={LINK} onClick={media.keepRecording}>Keep recording</button>
          )}
          <button className="flex items-center gap-1 font-medium text-foreground" onClick={media.stopRecording}>
            <CircleStop className="size-4" /> {recording.phase === 'recording' ? 'Stop' : 'Cancel'}
          </button>
        </div>
      )}
      {shown.guide && <p className="leading-snug">{GUIDE}</p>}
      {shown.problem && <p role="alert" className="text-destructive">{shown.problem}</p>}
      {(pending.length > 0 || items.length > 0) && (
        <div className="flex flex-wrap gap-1">
          {pending.map(each => (
            <button key={each.id} className={`${CHIP} ${each.problem ? 'text-destructive' : ''}`} onClick={() => toggle(`upload:${each.id}`)}>
              {each.problem ? 'Not kept' : <><LoaderCircle className="size-3.5 animate-spin" /> Saving</>}
            </button>
          ))}
          {items.map(each => (
            <button key={each.id} data-testid="media-item" title={when(each.takenAt)} className={CHIP} onClick={() => toggle(`item:${each.id}`)}>
              {each.kind === 'sound'
                ? <><Play className="size-3.5" /> {clock(each.durationS ?? 0)}</>
                : <><ImageIcon className="size-3.5" /> Photo</>}
            </button>
          ))}
        </div>
      )}
      {/* A save that never ends can be sent again or given up too. */}
      {upload && (
        <p className="flex flex-wrap gap-x-3">
          {upload.problem ? <span className="text-destructive">Not kept: {upload.problem}</span> : <span>Saving…</span>}
          <button className={LINK} onClick={() => media.retry(upload.id)}>Send again</button>
          <button className={LINK} onClick={() => media.discard(upload.id)}>Discard</button>
        </p>
      )}
      {item && (
        <div className="flex flex-col gap-1">
          {item.kind === 'sound'
            ? <audio controls preload="none" src={mediaUrl(item.id)} className="h-8 w-full" />
            : <img src={mediaUrl(item.id)} alt="Photo of this place" className="max-h-[40dvh] w-full rounded object-contain" />}
          <p className="flex gap-3">
            <span>{when(item.takenAt)}</span>
            <button
              className={LINK}
              onClick={() => {
                if (window.confirm(item.kind === 'sound' ? 'Delete this recording?' : 'Delete this photo?')) void media.remove(item.id)
              }}
            >
              Delete
            </button>
          </p>
        </div>
      )}
    </div>
  )
}
