// The About pages: the product's own markdown (src/about), one page per path under /about.
import { useEffect, useState } from 'react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { ABOUT_PAGES } from '../lib/about-pages'
import { setDocumentTitle } from '../utils/page-title'

/** The pages every About page links to, by path under /about. */
const NAV: [string, string][] = [
  ['', 'About'],
  ['news', "What's new"],
  ['methodology', 'Methodology'],
  ['credits', 'Data, credits and terms'],
]

export default function AboutPage({ page }: { page: string }) {
  const [text, setText] = useState<string | null>(null)
  useEffect(() => {
    let open = true
    const load = ABOUT_PAGES[page]
    if (load) void load().then(markdown => { if (open) setText(markdown) })
    else setText('# No such page\n\n[About quietmap.org](/about)')
    return () => { open = false }
  }, [page])
  const title = text?.match(/^# (.+)$/m)?.[1] ?? null
  useEffect(() => setDocumentTitle([title]), [title])
  return (
    <main className="h-full overflow-y-auto">
      <div className="about mx-auto max-w-3xl px-4 py-8">
        <nav className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
          <a href="/">← The map</a>
          {NAV.map(([path, label]) => (
            <a key={path} href={`/about${path && `/${path}`}`} aria-current={page === path ? 'page' : undefined}
              className={page === path ? 'no-underline font-medium text-foreground' : undefined}>{label}</a>
          ))}
        </nav>
        {text !== null && <Markdown remarkPlugins={[remarkGfm]}>{text}</Markdown>}
      </div>
    </main>
  )
}
