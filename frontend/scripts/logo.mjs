// The logo from the map's own colours: public/favicon.svg and every icon drawn from it (favicon.ico,
// the home-screen icons), so a new palette is one run away: node scripts/logo.mjs (from frontend/).
// A 0 in silence (white), ringed by the quiet zones' green, then the map's loudness ramp from its
// yellow (3 sone) toward its dark purple (48 sone at the gradient's radius, which the irregular blob's
// edge cuts at 6 to 40 sone): a quiet place amid rising noise. Rendered by Playwright's Chromium.
import { writeFileSync } from 'node:fs'
import { chromium } from '@playwright/test'
import { paletteHex, QUIET_RGB } from '../src/lib/heatmap-palette.ts'

const PUBLIC = new URL('../public/', import.meta.url)
const BLOB = 'M26.31,3.35 Q32.0,2.15 36.84,5.4 Q41.67,8.65 44.25,12.91 Q46.83,17.17 49.66,20.34 Q52.5,23.51 53.8,27.76 Q55.1,32.0 53.54,36.14 Q51.98,40.28 49.63,43.78 Q47.29,47.29 43.72,49.48 Q40.15,51.67 36.08,52.41 Q32.0,53.15 27.07,54.46 Q22.15,55.77 16.46,54.5 Q10.77,53.23 8.07,48.13 Q5.38,43.03 4.74,37.52 Q4.1,32.0 4.48,26.38 Q4.86,20.76 8.04,16.0 Q11.23,11.23 15.93,7.89 Q20.63,4.56 26.31,3.35 Z'
const ZERO = 'M32 23C26.8 23 24 26.5 24 32.5S26.8 42 32 42S40 38.5 40 32.5S37.2 23 32 23ZM32 27C34.6 27 35.75 28.8 35.75 32.5S34.6 38 32 38S28.25 36.2 28.25 32.5S29.4 27 32 27Z'
// Ring edges as fractions of the gradient's radius (30, around the 0 at 32,32): silence under the
// 0's stroke, the quiet green to QUIET_TO, fading into the ramp's yellow at RAMP_FROM, then the ramp
// by log2 of loudness, as the map draws it, to the edge.
const SILENCE_TO = 0.2
const QUIET_TO = 0.5
const RAMP_FROM = 0.6
const RAMP = Array.from({ length: 17 }, (_, k) => 3 * 2 ** (k / 4))

function logoSvg() {
  const green = '#' + QUIET_RGB.map(c => c.toString(16).padStart(2, '0')).join('')
  const span = Math.log2(RAMP.at(-1) / RAMP[0])
  const stops = [[0, '#ffffff'], [SILENCE_TO, '#ffffff'], [SILENCE_TO, green], [QUIET_TO, green],
    ...RAMP.map(sone => [RAMP_FROM + (1 - RAMP_FROM) * Math.log2(sone / RAMP[0]) / span, paletteHex('nden', sone)])]
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img" aria-label="quietmap.org">
  <!-- Drawn by frontend/scripts/logo.mjs from the map's colours (src/lib/heatmap-palette.ts): a 0 in
       silence, ringed by the quiet zones' green, then the loudness ramp to the blob's edge. -->
  <style>@media (prefers-color-scheme: dark) { .edge { stroke: rgba(255,255,255,.45) } }</style>
  <defs>
    <radialGradient id="g" cx="32" cy="32" r="30" gradientUnits="userSpaceOnUse">
${stops.map(([at, color]) => `      <stop offset="${+at.toFixed(4)}" stop-color="${color}"/>`).join('\n')}
    </radialGradient>
  </defs>
  <path class="edge" fill="url(#g)" stroke="rgba(0,0,0,.12)" stroke-width="0.6" d="${BLOB}"/>
  <path fill="#171717" fill-rule="evenodd" d="${ZERO}"/>
</svg>
`
}

/** An .ico of PNG images (Windows Vista and later, every browser). */
function ico(pngs) {
  const head = Buffer.alloc(6 + 16 * pngs.length)
  head.writeUInt16LE(1, 2)
  head.writeUInt16LE(pngs.length, 4)
  let offset = head.length
  pngs.forEach(({ size, data }, i) => {
    const at = 6 + 16 * i
    head.writeUInt8(size % 256, at)
    head.writeUInt8(size % 256, at + 1)
    head.writeUInt16LE(1, at + 4)
    head.writeUInt16LE(32, at + 6)
    head.writeUInt32LE(data.length, at + 8)
    head.writeUInt32LE(offset, at + 12)
    offset += data.length
  })
  return Buffer.concat([head, ...pngs.map(png => png.data)])
}

const svg = logoSvg()
writeFileSync(new URL('favicon.svg', PUBLIC), svg)
const browser = await chromium.launch()
const page = await browser.newPage()
await page.setContent(svg)
// The blob's bounds with its stroke: each icon centres them, whatever the blob's shape.
const box = await page.evaluate(() => {
  const { x, y, width, height } = document.querySelector('path').getBBox()
  return { x: x - 0.3, y: y - 0.3, side: Math.max(width, height) + 0.6, width: width + 0.6, height: height + 0.6 }
})
// `fill` of the side the blob takes; `background` none for a transparent icon.
const render = async (size, fill, background) => {
  const side = box.side / fill
  const viewBox = [box.x + box.width / 2 - side / 2, box.y + box.height / 2 - side / 2, side, side].join(' ')
  await page.setViewportSize({ width: size, height: size })
  await page.setContent(`<body style="margin:0;background:${background ?? 'transparent'}">${
    svg.replace('viewBox="0 0 64 64"', `viewBox="${viewBox}" width="${size}" height="${size}" style="display:block"`)}</body>`)
  return { size, data: await page.screenshot({ omitBackground: !background }) }
}
const icons = [[16, 1], [32, 0.98], [48, 0.96]]
const small = []
for (const [size, fill] of icons) small.push(await render(size, fill))
writeFileSync(new URL('favicon.ico', PUBLIC), ico(small))
writeFileSync(new URL('icon-192.png', PUBLIC), (await render(192, 0.94)).data)
writeFileSync(new URL('icon-512.png', PUBLIC), (await render(512, 0.94)).data)
// iOS fills a transparent icon black and rounds its corners; Android crops a maskable one to its
// inner 80 % circle.
writeFileSync(new URL('apple-touch-icon.png', PUBLIC), (await render(180, 0.8, '#ffffff')).data)
writeFileSync(new URL('icon-maskable-512.png', PUBLIC), (await render(512, 0.6, '#ffffff')).data)
await browser.close()
