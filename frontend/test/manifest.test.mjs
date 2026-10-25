// The home-screen manifest: valid JSON, opening the map full screen, each icon a PNG of its size.
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

const PUBLIC = new URL('../public/', import.meta.url)

test("the manifest's icons are PNGs of the sizes it gives, one of them maskable", () => {
  const manifest = JSON.parse(readFileSync(new URL('manifest.webmanifest', PUBLIC), 'utf8'))
  assert.equal(manifest.display, 'standalone')
  assert.equal(manifest.start_url, '/')
  for (const icon of manifest.icons) {
    const png = readFileSync(new URL(icon.src.slice(1), PUBLIC))
    // A PNG's width and height are the first fields of its IHDR chunk.
    assert.equal(`${png.readUInt32BE(16)}x${png.readUInt32BE(20)}`, icon.sizes, icon.src)
  }
  assert.ok(manifest.icons.some(icon => icon.purpose === 'maskable'))
  const apple = readFileSync(new URL('apple-touch-icon.png', PUBLIC))
  assert.equal(apple.readUInt32BE(16), 180)
})
