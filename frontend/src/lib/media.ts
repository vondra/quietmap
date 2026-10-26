// A signed-in visitor's recordings and photos of places (server/src/routes/media.ts): listed within
// 30 m of a place, uploaded under an id this page chose, so that a retry after a lost answer keeps
// one copy, read by the account's cookie and deleted. An upload and a deletion name the account the
// page shows, as every change of an account does (account.ts).

/** A recording or a photo an account keeps (the server's answer, and the row it keeps). */
export interface MediaItem {
  /** 32 hex digits, chosen by the page that uploaded it. */
  id: string
  kind: 'sound' | 'photo'
  lat: number
  lng: number
  /** When it was recorded or chosen, by the phone's clock (ms). */
  takenAt: number
  bytes: number
  /** A recording's length (s). */
  durationS: number | null
}

/** A recording or a photo on its way to the account it was made for. */
export interface Upload extends Omit<MediaItem, 'bytes'> {
  account: string
  body: Blob
  /** What the server keeps beside it, for levels computed later: the browser, the microphone, the
   *  processing the browser reports. */
  details: Record<string, unknown>
}

export const newMediaId = () => Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, '0')).join('')

export const mediaUrl = (id: string) => `/api/media/${id}`

/** The account's items within 30 m of `at`, the latest first; null when the map was not reached. */
export async function listMedia(at: { lat: number, lng: number }): Promise<MediaItem[] | null> {
  const response = await fetch(`/api/media?lat=${at.lat}&lng=${at.lng}`, { cache: 'no-store' }).catch(() => null)
  if (!response?.ok) return null
  return ((await response.json().catch(() => null)) as { items: MediaItem[] } | null)?.items ?? null
}

/** Keeps `upload` with its account: the item kept, else what to tell the visitor. */
export async function uploadMedia({ account, body, details, id, lat, lng, takenAt }: Upload): Promise<MediaItem | string> {
  const response = await fetch(mediaUrl(id), {
    method: 'PUT',
    headers: {
      'Content-Type': body.type,
      'X-Quietmap-Account': account,
      'X-Quietmap-Media': encodeURIComponent(JSON.stringify({ lat, lng, takenAt, details })),
    },
    body,
  }).catch(() => null)
  if (response?.ok) {
    const item = await response.json().catch(() => null) as MediaItem | null
    if (item) return item
  }
  // No answer, or one cut short, leaves it unknown whether it was kept: sent again, it is kept once.
  if (!response || response.ok) return 'The map could not be reached.'
  if (response.status === 401 || response.status === 409) return 'Signed out, or in as another account.'
  if (response.status === 413) return 'Too large to keep.'
  const answer = await response.json().catch(() => null) as { error?: string } | null
  return answer?.error ?? 'The map could not keep it.'
}

/** Deletes the account's item `id`: whether it is gone. */
export async function deleteMedia(account: string, id: string): Promise<boolean> {
  const response = await fetch(mediaUrl(id), { method: 'DELETE', headers: { 'X-Quietmap-Account': account } }).catch(() => null)
  return response?.status === 204 || response?.status === 404
}
