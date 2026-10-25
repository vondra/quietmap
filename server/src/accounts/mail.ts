// The sign-in mail, sent through Resend's HTTP API from the map's domain: plain text only, so no
// link in it is rewritten for click tracking and the token goes nowhere but to the visitor.
import { readFileSync } from 'node:fs'
import { LINK_MS } from './store.ts'

/** Sends a sign-in link; throws when the mail could not be handed over. */
export type SendLink = (to: string, url: string) => Promise<void>

const RESEND_URL = 'https://api.resend.com/emails'
const TIMEOUT_MS = 10_000

export function resendSender(keyFile: string, from: string): SendLink {
  const key = readFileSync(keyFile, 'utf8').trim()
  return async (to, url) => {
    const response = await fetch(RESEND_URL, {
      method: 'POST',
      headers: { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({
        from,
        to: [to],
        subject: 'Your Quiet Map sign-in link',
        text: `Open this link to sign in to Quiet Map:\n\n${url}\n\n`
          + `It works once, within ${LINK_MS / 60_000} minutes. If you did not ask for it, ignore this mail: `
          + 'no account is made until its link is used.\n\nQuiet Map, quietmap.org\n',
      }),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    })
    // The status alone: an answer may quote the mail, and so the link.
    if (!response.ok) throw new Error(`Resend answered ${response.status}`)
  }
}
