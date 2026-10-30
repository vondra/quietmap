// Where the visitor's places live, the recent places and the names given places: this browser's
// storage (browser-places.ts) while signed out, the account (account.ts) while signed in, never
// mixed. A change shows at once; until the server has said who keeps the places it is held, then kept
// there. The account's requests run one after another, so the server takes the changes in the order
// made and a sign-out comes after them; each read of the account shows its answer with the changes
// not yet sent applied over it. hooks/usePlaces.ts reads it again whenever the map comes back.
import { deleteAccount, fetchAccount, saveRecent, saveTitle, signOut } from './account.ts'
import { loadPlaceTitles, loadRecentPlaces, savePlaceTitles, saveRecentPlaces } from './browser-places.ts'
import { withAnswer, withName, withOpened, withTitle, withoutPlace, type PlaceTitle, type RecentPlace } from './place-rules.ts'

/** Who keeps the places: 'unknown' until the server answers, null when signed out, 'unavailable' on
 *  a server without the sign-in, else the signed-in account. */
export type AccountState = 'unknown' | 'unavailable' | null | { email: string }

export const isSignedIn = (account: AccountState): account is { email: string } => typeof account === 'object' && account !== null

interface Places {
  recent: RecentPlace[]
  titles: PlaceTitle[]
}

export interface Shown extends Places {
  account: AccountState
  /** The server did not take the last change. */
  notSaved: boolean
}

/** A change of the recent places, or a place's name given or taken back. */
type Change = { recent: (recent: RecentPlace[]) => RecentPlace[] } | { at: { lat: number, lng: number }, title: string | null, updatedAt: number }

function applied<T extends Places>(places: T, change: Change): T {
  return 'recent' in change
    ? { ...places, recent: change.recent(places.recent) }
    : { ...places, titles: withTitle(places.titles, change.at, change.title, change.updatedAt) }
}

const browserPlaces = (): Places => ({ recent: loadRecentPlaces(), titles: loadPlaceTitles() ?? [] })

export function createPlacesKeeper() {
  let shown: Shown = { account: 'unknown', ...browserPlaces(), notSaved: false }
  const listeners = new Set<() => void>()
  const show = (next: Partial<Shown>) => {
    shown = { ...shown, ...next }
    listeners.forEach(listener => listener())
  }
  // The changes made before the server said who keeps the places, and the account's not yet sent.
  const held: Change[] = []
  let unsent: Change[] = []
  let queue: Promise<unknown> = Promise.resolve()
  const enqueue = <T>(task: () => Promise<T>): Promise<T> => {
    const run = queue.then(task)
    queue = run.catch(() => {})
    return run
  }
  let flushQueued = false

  /** The browser keeps the places: its own, with the changes held at the start, kept in its storage. */
  function keepInBrowser(account: null | 'unavailable') {
    unsent = []
    if (shown.account !== 'unknown' && !isSignedIn(shown.account)) return show({ account })
    const places = held.splice(0).reduce(applied, browserPlaces())
    saveRecentPlaces(places.recent)
    savePlaceTitles(places.titles)
    show({ account, ...places, notSaved: false })
  }

  /** Reads who keeps the places, and the account's. */
  const refresh = () => enqueue(async () => {
    const answer = await fetchAccount()
    const was = shown.account
    if (answer === undefined) {
      // Not known at the start: the places stay this browser's for this visit.
      if (was === 'unknown') keepInBrowser(null)
    } else if (answer === null || answer === 'unavailable') {
      keepInBrowser(answer)
    } else {
      // Changes held at the start go to the account; another account's not yet sent are dropped.
      if (was === 'unknown') unsent = held.splice(0)
      else if (!isSignedIn(was) || was.email !== answer.email) unsent = []
      show({ account: { email: answer.email }, ...unsent.reduce(applied, { recent: answer.recent, titles: answer.titles }) })
      if (unsent.length > 0) flush()
    }
  })

  /** Sends the account's changes not yet sent: each name, then the recent places as one list. */
  function flush() {
    if (flushQueued) return
    flushQueued = true
    void enqueue(async () => {
      flushQueued = false
      const { account } = shown
      const sending = unsent.splice(0)
      if (!isSignedIn(account) || sending.length === 0) return
      let failed = false
      for (const change of sending) {
        if ('recent' in change) continue
        const response = await saveTitle(account.email, { ...change.at, title: change.title, updatedAt: change.updatedAt })
        if (!response?.ok) {
          failed = true
          break
        }
        const { titles } = await response.json() as { titles: PlaceTitle[] }
        show({ titles: unsent.reduce(applied, { recent: [], titles }).titles })
      }
      if (!failed && sending.some(change => 'recent' in change)) {
        // The list sent carries every change of it made so far, sent or not: once kept, none of them
        // is played again over the account's list.
        const carried = new Set<Change>(unsent.filter(change => 'recent' in change))
        failed = !(await saveRecent(account.email, shown.recent))?.ok
        if (!failed) unsent = unsent.filter(change => !carried.has(change))
      }
      if (shown.notSaved !== failed) show({ notSaved: failed })
      // What the server holds now, the changes since applied over it.
      if (failed) void refresh()
    })
  }

  function change(made: Change) {
    const next = applied(shown, made)
    if (next.recent === shown.recent && next.titles === shown.titles) return
    if (shown.account === 'unknown') {
      held.push(made)
      return show(next)
    }
    if (isSignedIn(shown.account)) {
      unsent.push(made)
      show(next)
      return flush()
    }
    if ('recent' in made) {
      saveRecentPlaces(next.recent)
      return show({ recent: next.recent })
    }
    // From what storage holds now: another tab may have named places since this one read them.
    const titles = withTitle(loadPlaceTitles() ?? shown.titles, made.at, made.title, made.updatedAt)
    savePlaceTitles(titles)
    show({ titles })
  }

  /** Signs out, or deletes the account, after its changes have gone, and shows this browser's own
   *  places again: whether done. */
  const leave = (request: (account: string) => Promise<boolean>) => enqueue(async () => {
    const { account } = shown
    if (!isSignedIn(account) || !await request(account.email)) return false
    unsent = []
    show({ account: null, ...browserPlaces(), notSaved: false })
    return true
  })

  return {
    subscribe(listener: () => void) {
      listeners.add(listener)
      return () => void listeners.delete(listener)
    },
    shown: () => shown,
    refresh,
    /** Another tab named a place, while the places are this browser's. */
    titlesStored() {
      if (shown.account !== 'unknown' && !isSignedIn(shown.account)) show({ titles: loadPlaceTitles() ?? [] })
    },
    /** A place opened, by a click, a search, the history or a link: the last opened, named `place` (a
     *  search's result) until its own name comes. Its time is taken now, so a replay over the account's
     *  places keeps it. */
    open: (at: { lat: number, lng: number }, place: string | null = null) => {
      const openedAt = Date.now()
      change({ recent: recent => withOpened(recent, at, openedAt, place) })
    },
    /** An answered point's numbers, and its name when its place has none. */
    answered: (answer: Parameters<typeof withAnswer>[1]) => change({ recent: recent => withAnswer(recent, answer) }),
    /** A place's geocoded name, which came back after the click. */
    nameLate: (at: { lat: number, lng: number }, name: string) => change({ recent: recent => withName(recent, at, name) }),
    forget: (place: RecentPlace) => change({ recent: recent => withoutPlace(recent, place) }),
    /** Names the place at `at`, or gives its name back with null. */
    rename: (at: { lat: number, lng: number }, title: string | null) => change({ at, title, updatedAt: Date.now() }),
    signOut: () => leave(() => signOut()),
    deleteAccount: () => leave(deleteAccount),
  }
}

export type PlacesKeeper = ReturnType<typeof createPlacesKeeper>
