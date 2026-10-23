// Every link to an About section lands on a heading of its page: the pages' own links, their "On
// this page" lists and the popup's weather link; a page never gives two headings one id.
import assert from 'node:assert/strict'
import { globSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import test from 'node:test'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

import { splitFrontMatter } from '../src/lib/about-front-matter.ts'
import { ABOUT_WEATHER, aboutPageOf, aboutPagePath } from '../src/lib/about-paths.ts'
import { ABOUT_REHYPE_PLUGINS, sectionId } from '../src/lib/about-sections.ts'

const ABOUT_DIR = new URL('../src/about', import.meta.url).pathname

/** Every About page's HTML as the page renders its markdown. */
const PAGES = Object.fromEntries(globSync('**/*.md', { cwd: ABOUT_DIR }).map(file => {
  const { body } = splitFrontMatter(readFileSync(join(ABOUT_DIR, file), 'utf8'))
  const markdown = createElement(Markdown, { remarkPlugins: [remarkGfm], rehypePlugins: ABOUT_REHYPE_PLUGINS }, body)
  return [aboutPagePath(file), renderToStaticMarkup(markdown)]
}))

const idsOf = html => [...html.matchAll(/ id="([^"]+)"/g)].map(match => match[1])

test('a section id is its heading in lower case, its words joined by hyphens', () => {
  assert.equal(sectionId('The weather'), 'the-weather')
  assert.equal(sectionId('Credits & terms'), 'credits-terms')
  assert.equal(sectionId('Terrain, ground and weather'), 'terrain-ground-and-weather')
})

test('a page never gives two headings one id', () => {
  for (const [page, html] of Object.entries(PAGES)) {
    const ids = idsOf(html)
    assert.deepEqual(ids.filter((id, at) => ids.indexOf(id) !== at), [], page)
  }
})

test('every link to an About section lands on a heading of its page', () => {
  const links = [['the popup', ABOUT_WEATHER]]
  for (const [page, html] of Object.entries(PAGES)) {
    for (const [, href] of html.matchAll(/ href="([^"]*#[^"]*)"/g)) {
      if (href.startsWith('#')) links.push([page, `/about/${page}${href}`])
      else if (href.startsWith('/about')) links.push([page, href])
    }
  }
  assert.ok(links.some(([from]) => from === 'noise-at-a-point'), 'the weather page lists its sections')
  for (const [from, href] of links) {
    const [path, id] = href.split('#')
    const page = aboutPageOf(path)
    assert.ok(page !== null && PAGES[page] !== undefined, `${from}: ${href} names no page`)
    assert.ok(idsOf(PAGES[page]).includes(decodeURIComponent(id)), `${from}: ${href} lands on no heading`)
  }
})
