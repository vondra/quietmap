// The About pages: the product's own markdown (src/about), one page per path under /about.
import { useEffect } from 'react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import index from '../about/index.md?raw'
import methodology from '../about/methodology.md?raw'
import credits from '../about/credits.md?raw'
import news from '../about/news.md?raw'
import { setDocumentTitle } from '../utils/page-title'

const PAGES: Record<string, string> = { '': index, methodology, credits, news }

export default function AboutPage({ page }: { page: string }) {
  const text = PAGES[page] ?? index
  const title = text.match(/^# (.+)$/m)?.[1] ?? null
  useEffect(() => setDocumentTitle([title]), [title])
  return (
    <main className="h-full overflow-y-auto">
      <div className="about mx-auto max-w-3xl px-4 py-8">
        <a href="/">← Back to the map</a>
        <Markdown remarkPlugins={[remarkGfm]}>{text}</Markdown>
      </div>
    </main>
  )
}
