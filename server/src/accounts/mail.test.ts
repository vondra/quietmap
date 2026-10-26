// The sign-in mail: its links and its logo on the map's own origin, the link written out for a
// client that drops the button, nothing embedded (Gmail drops data: images).
import assert from 'node:assert/strict'
import test from 'node:test'
import { signInMail } from './mail.ts'
import { LINK_MS } from './store.ts'

test('the sign-in mail links only its own origin, writes the link out and embeds nothing', () => {
  const url = 'https://quietmap.org/signin#AbC-123_x'
  const { subject, html, text } = signInMail(url)
  assert.match(subject, /Quiet Map/)
  const links = [...html.matchAll(/(?:href|src)="([^"]*)"/g)].map(match => match[1])
  assert.deepEqual(links.filter(link => link === url).length, 2)
  assert.ok(links.every(link => link.startsWith('https://quietmap.org/')), links.join(' '))
  assert.ok(links.includes('https://quietmap.org/icon-192.png'))
  assert.doesNotMatch(html, /data:/)
  assert.ok(text.includes(`\n${url}\n`))
  for (const part of [html, text]) assert.ok(part.includes(`within ${LINK_MS / 60_000} minutes`))
})
