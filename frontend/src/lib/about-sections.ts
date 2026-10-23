// The About pages' sections: every h2 and h3 gets an id from its words, so a link can land on a
// section (`/about/noise-at-a-point#the-weather`), and a page's `<!-- SECTIONS -->` becomes the
// list of its h2s, "On this page", under its intro (owner 2026-10-23).
import type { Element, ElementContent, Root, RootContent } from 'hast'
import type { Options } from 'react-markdown'
import rehypeRaw from 'rehype-raw'

/** Where a page lists its sections: the comment's place in its markdown. */
const SECTIONS_MARKER = 'SECTIONS'

/** A section's id: its heading's words in lower case, joined by hyphens ("The weather" is
 *  "the-weather"). */
export function sectionId(heading: string): string {
  return heading.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, '-').replace(/^-+|-+$/g, '')
}

function textOf(node: RootContent): string {
  if (node.type === 'text') return node.value
  return node.type === 'element' ? node.children.map(textOf).join('') : ''
}

function element(tagName: string, properties: Element['properties'], children: ElementContent[]): Element {
  return { type: 'element', tagName, properties, children }
}

/** Gives every h2 and h3 its id and puts the list of the h2s where the marker stands. */
function rehypeAboutSections() {
  return (tree: Root) => {
    const sections: [string, string][] = []
    const visit = (nodes: RootContent[]) => {
      for (const node of nodes) {
        if (node.type !== 'element') continue
        if (node.tagName !== 'h2' && node.tagName !== 'h3') {
          visit(node.children)
          continue
        }
        const text = textOf(node)
        const id = sectionId(text)
        node.properties.id = id
        if (node.tagName === 'h2') sections.push([id, text])
      }
    }
    visit(tree.children)
    const at = tree.children.findIndex(node => node.type === 'comment' && node.value.trim() === SECTIONS_MARKER)
    if (at < 0) return
    tree.children[at] = element('nav', { className: ['about-sections'], ariaLabel: 'On this page' }, [
      element('p', {}, [{ type: 'text', value: 'On this page' }]),
      element('ul', {}, sections.map(([id, text]) =>
        element('li', {}, [element('a', { href: `#${id}` }, [{ type: 'text', value: text }])]))),
    ])
  }
}

/** The About pages' HTML steps: the raw HTML in their markdown (folds, figures, explained words),
 *  then the sections. */
export const ABOUT_REHYPE_PLUGINS: Options['rehypePlugins'] = [rehypeRaw, rehypeAboutSections]
