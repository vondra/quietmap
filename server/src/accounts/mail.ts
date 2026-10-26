// The sign-in mail, sent through Resend's HTTP API from the map's domain: the logo, one button and the
// link written out, with the same words as plain text. Resend rewrites a mail's links only while
// click tracking is on, a setting of the domain that must stay off for quietmap.org: each sign-in
// link would otherwise pass through Resend's redirect.
import { readFileSync } from 'node:fs'
import { LINK_MS } from './store.ts'

/** Sends a sign-in link; throws when the mail could not be handed over. */
export type SendLink = (to: string, url: string) => Promise<void>

const RESEND_URL = 'https://api.resend.com/emails'
const TIMEOUT_MS = 10_000
// The quiet zones' green (frontend/src/lib/heatmap-palette.ts QUIET_RGB), and a step darker for the
// button, as on the sign-in page: white on #16a34a is 3.3:1, on #15803d 5.0:1 (WCAG AA).
const QUIET = '#16a34a'
const BUTTON = '#15803d'
const FONT = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif"

const escape = (text: string) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')

/** The mail for a sign-in `url` on the map's origin; its logo is that origin's icon. */
export function signInMail(url: string): { subject: string, html: string, text: string } {
  const origin = new URL(url).origin
  const link = escape(url)
  const expiry = `The link works once, within ${LINK_MS / 60_000} minutes.`
  const purpose = 'Your named places and the places you opened then follow you to every device you sign in on.'
  const ignore = "Didn't ask for it? Ignore this mail: no account is made until its link is used."
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta name="supported-color-schemes" content="light dark">
<title>Sign in to Quiet Map</title>
<style>
  @media (prefers-color-scheme: dark) {
    body, .page { background: #111111 !important; }
    .rule { border-color: #2c2c2c !important; }
    .card { background: #1b1b1b !important; border-color: #2c2c2c !important; }
    .ink { color: #ececec !important; }
    .muted, .muted a { color: #a3a3a3 !important; }
  }
</style>
</head>
<body style="margin:0;padding:0;background:#f5f6f5;">
<div style="display:none;max-height:0;overflow:hidden;">Press the button to sign in. ${expiry}</div>
<table role="presentation" class="page" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:#f5f6f5;">
<tr><td align="center" style="padding:32px 16px;">
<!--[if mso]><table role="presentation" width="480" align="center" cellpadding="0" cellspacing="0" border="0"><tr><td><![endif]-->
<table role="presentation" class="card" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:480px;background:#ffffff;border:1px solid #e7e7e7;border-radius:12px;">
<tr><td style="padding:32px 32px 8px 32px;">
  <table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr>
    <td style="vertical-align:middle;"><img src="${origin}/icon-192.png" width="40" height="40" alt="" style="display:block;border:0;"></td>
    <td class="ink" style="vertical-align:middle;padding-left:10px;font:600 20px/1.2 ${FONT};letter-spacing:-0.03em;color:#171717;">quietmap.org</td>
  </tr></table>
</td></tr>
<tr><td class="ink" style="padding:20px 32px 0 32px;font:600 22px/1.3 ${FONT};color:#171717;">Sign in to Quiet Map</td></tr>
<tr><td class="muted" style="padding:10px 32px 0 32px;font:400 15px/1.55 ${FONT};color:#525252;">Press the button to sign in. ${purpose}</td></tr>
<tr><td style="padding:24px 32px 0 32px;">
  <table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr>
    <td bgcolor="${BUTTON}" style="border-radius:8px;mso-padding-alt:12px 28px;">
      <a href="${link}" target="_blank" style="display:inline-block;padding:12px 28px;border-radius:8px;background:${BUTTON};font:600 16px/20px ${FONT};color:#ffffff;text-decoration:none;"><span style="color:#ffffff;">Sign in</span></a>
    </td>
  </tr></table>
</td></tr>
<tr><td class="muted" style="padding:20px 32px 0 32px;font:400 13px/1.55 ${FONT};color:#737373;">${expiry} If the button does nothing, paste this into your browser:<br>
  <a href="${link}" style="color:#737373;word-break:break-all;">${link}</a></td></tr>
<tr><td style="padding:24px 32px 0 32px;"><div class="rule" style="border-top:1px solid #e7e7e7;font-size:0;line-height:0;">&nbsp;</div></td></tr>
<tr><td class="muted" style="padding:16px 32px 0 32px;font:400 13px/1.55 ${FONT};color:#737373;">${ignore}</td></tr>
<tr><td class="muted" style="padding:16px 32px 28px 32px;font:400 13px/1.55 ${FONT};color:#737373;">
  <span style="color:${QUIET};">&#9679;</span>&nbsp;This mail: 0 sone.<br>
  <a href="${origin}/" style="color:#737373;">Quiet Map</a>: how loud any place on Earth is, and where it is quiet.</td></tr>
</table>
<!--[if mso]></td></tr></table><![endif]-->
</td></tr>
</table>
</body>
</html>
`
  const text = `Sign in to Quiet Map\n\nOpen this link to sign in:\n\n${url}\n\n${expiry} ${purpose}\n\n${ignore}\n\n`
    + `This mail: 0 sone.\nQuiet Map, ${origin}/\n`
  return { subject: 'Your Quiet Map sign-in link', html, text }
}

export function resendSender(keyFile: string, from: string): SendLink {
  const key = readFileSync(keyFile, 'utf8').trim()
  return async (to, url) => {
    const response = await fetch(RESEND_URL, {
      method: 'POST',
      headers: { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ from, to: [to], ...signInMail(url) }),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    })
    // The status alone: an answer may quote the mail, and so the link.
    if (!response.ok) throw new Error(`Resend answered ${response.status}`)
  }
}
