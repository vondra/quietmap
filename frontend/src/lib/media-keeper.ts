// A signed-in visitor's recordings and photos at the open place: one keeper for the page, above the
// desktop card and the phone sheet that both draw it (components/PlaceMedia.tsx). It holds the place's
// items, what is on its way to the account, and the recording, which belongs to the account and the
// place it started at: another place or account ends it, kept for its own. A take or a photo stays in
// the page until its account keeps it, through a sign-out and back; meanwhile it can be sent again
// (under its id, so one copy is kept) or discarded.
import { deleteMedia, listMedia, newMediaId, uploadMedia, type MediaItem, type Upload } from './media.ts'
import { jpegOf } from './photo.ts'
import { samePlace } from './place-rules.ts'
import type { Recorder, RecordingPhase, Take, startRecording } from './recorder.ts'

type Point = { lat: number, lng: number }

export interface Pending extends Upload {
  /** Why it is not kept; null while it is on its way. */
  problem: string | null
}

export interface MediaShown {
  /** The account and the open place the items are for. */
  account: string | null
  place: Point | null
  /** The place's items, the latest first; null until they come. */
  items: MediaItem[] | null
  /** Recordings and photos not yet kept, of every place and account. */
  pending: Pending[]
  recording: { phase: RecordingPhase, seconds: number, limitS: number } | null
  /** What went wrong with the last recording or photo before it could be sent. */
  problem: string | null
  /** How to hold the phone is shown. */
  guide: boolean
}

const samePoint = (a: Point | null, b: Point | null) => a?.lat === b?.lat && a?.lng === b?.lng

/** The keeper, recording with `start` (recorder.ts's `startRecording`). */
export function createMediaKeeper(start: typeof startRecording) {
  let shown: MediaShown = { account: null, place: null, items: null, pending: [], recording: null, problem: null, guide: false }
  const listeners = new Set<() => void>()
  const show = (next: Partial<MediaShown>) => {
    shown = { ...shown, ...next }
    listeners.forEach(listener => listener())
  }
  let recorder: Recorder | null = null

  async function send(upload: Pending) {
    show({ pending: [...shown.pending.filter(pending => pending.id !== upload.id), { ...upload, problem: null }] })
    const answer = await uploadMedia(upload)
    const pending = shown.pending.filter(other => other.id !== upload.id)
    if (typeof answer === 'string') {
      if (pending.length < shown.pending.length) show({ pending: [...pending, { ...upload, problem: answer }] })
      return
    }
    const here = shown.account === upload.account && shown.place && samePlace(answer, shown.place)
    show({ pending, items: here ? [answer, ...(shown.items ?? []).filter(item => item.id !== answer.id)] : shown.items })
  }

  /** A new upload of what was made at the open place for its account. */
  const made = (at: { account: string, place: Point }, fields: Pick<Upload, 'kind' | 'takenAt' | 'body' | 'durationS' | 'details'>): Pending =>
    ({ id: newMediaId(), account: at.account, ...at.place, ...fields, problem: null })

  return {
    subscribe(listener: () => void) {
      listeners.add(listener)
      return () => void listeners.delete(listener)
    },
    shown: () => shown,

    /** The page shows `place` for `account` (null: none, or signed out): its items are fetched, again
     *  at each call until they come (the page calls when it comes back to the screen). */
    open(account: string | null, place: Point | null) {
      if (account !== shown.account || !samePoint(place, shown.place)) {
        recorder?.stop()
        show({ account, place, items: null, problem: null })
      }
      if (!account || !place || shown.items) return
      void listMedia(place).then(items => {
        if (!items || shown.account !== account || !samePoint(shown.place, place)) return
        // A list asked before an upload was kept may lack it.
        const merged = new Map([...items, ...(shown.items ?? [])].map(item => [item.id, item]))
        show({ items: [...merged.values()].sort((a, b) => b.takenAt - a.takenAt) })
      })
    },

    /** Records at the open place; called in the tap. */
    record() {
      if (recorder || !shown.account || !shown.place) return
      const at = { account: shown.account, place: shown.place }
      const takenAt = Date.now()
      show({ problem: null })
      recorder = start((phase, seconds, limitS) => show({ recording: { phase, seconds, limitS } }), (take: Take | null, problem?: string) => {
        recorder = null
        show({ recording: null, problem: problem ?? null })
        if (take) void send(made(at, { kind: 'sound', takenAt, body: take.wav, durationS: take.durationS, details: take.details }))
      })
    },
    stopRecording: () => recorder?.stop(),
    keepRecording: () => recorder?.keepGoing(),
    resumeRecording: () => recorder?.resume(),

    /** Keeps a photo the visitor took or chose at the open place. */
    async addPhoto(file: File) {
      if (!shown.account || !shown.place) return
      const at = { account: shown.account, place: shown.place }
      const photo = await jpegOf(file)
      if (!photo) return show({ problem: 'This browser cannot read that photo. Try a JPEG.' })
      void send(made(at, {
        kind: 'photo', takenAt: file.lastModified || Date.now(), body: photo.blob, durationS: null,
        details: { userAgent: navigator.userAgent, width: photo.width, height: photo.height, file: { type: file.type, bytes: file.size } },
      }))
    },

    retry(id: string) {
      const upload = shown.pending.find(pending => pending.id === id)
      if (upload) void send(upload)
    },
    discard: (id: string) => show({ pending: shown.pending.filter(pending => pending.id !== id) }),

    async remove(id: string) {
      const account = shown.account
      if (!account) return
      if (!(await deleteMedia(account, id))) return show({ problem: 'It could not be deleted. Try again.' })
      show({ items: shown.items && shown.items.filter(item => item.id !== id) })
    },

    toggleGuide: () => show({ guide: !shown.guide }),
  }
}

export type MediaKeeper = ReturnType<typeof createMediaKeeper>
