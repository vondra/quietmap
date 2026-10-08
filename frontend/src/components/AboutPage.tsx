// The About pages as dev4 had them: the product's markdown (src/about) with its front matter, the
// logo and title on the first page and a breadcrumb on the others, an intro, the pages below each
// one listed, and the contact at the foot.
import { useEffect, useState } from 'react'
import Markdown, { type Components } from 'react-markdown'
import rehypeRaw from 'rehype-raw'
import remarkGfm from 'remark-gfm'
import ABOUT_INDEX from 'virtual:about-index'
import { splitFrontMatter } from '../lib/about-front-matter'
import { ABOUT_PAGES } from '../lib/about-pages'
import { setDocumentTitle } from '../utils/page-title'

/** Where a page places the list of the pages below it; without it the list opens the page. */
const CHILDREN_MARKER = '<!-- REGION_CHILDREN -->'

/** An image named without a path is one of the About pages' own (public/about/images). */
const COMPONENTS: Components = {
  img: ({ src, alt, ...props }) => (
    <img src={typeof src === 'string' && !/^(https?:)?\//.test(src) ? `/about/images/${src}` : src} alt={alt} {...props} />
  ),
}

function Prose({ children }: { children: string }) {
  return <Markdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeRaw]} components={COMPONENTS}>{children}</Markdown>
}

/** The pages one level below `page`, by title, but those it does not list. */
function childrenOf(page: string): [string, string][] {
  return Object.entries(ABOUT_INDEX)
    .filter(([path, meta]) => {
      if (meta.hidden || path === page) return false
      const rest = page === '' ? path : path.startsWith(`${page}/`) ? path.slice(page.length + 1) : null
      return rest !== null && !rest.includes('/')
    })
    .map(([path, meta]): [string, string] => [path, meta.title])
    .sort(([, a], [, b]) => a.localeCompare(b))
}

function ChildrenGrid({ items }: { items: [string, string][] }) {
  return (
    <div className="my-6 grid grid-cols-2 gap-2 md:grid-cols-3">
      {items.map(([path, title]) => (
        <a key={path} href={`/about/${path}`}
          className="rounded-lg border border-border px-3 py-2 text-sm font-medium text-foreground transition-colors hover:bg-accent">
          {title}
        </a>
      ))}
    </div>
  )
}

/** The pages above `page` and itself: the first page, a region, a country. */
function breadcrumb(page: string): [string, string][] {
  const parts = page ? page.split('/') : []
  return ['', ...parts.map((_, at) => parts.slice(0, at + 1).join('/'))]
    .filter(path => ABOUT_INDEX[path])
    .map(path => [path, ABOUT_INDEX[path].title])
}

/** The contact address put together on the page, never whole in the bundle (address harvesters). */
function Footer() {
  const contact = ['info', 'quietmap.org'].join('@')
  return (
    <div className="mt-12 flex items-center justify-between border-t border-border pt-6 text-sm text-muted-foreground/60">
      <a href="/about" className="hover:underline">quietmap.org</a>
      <a href="#contact" className="hover:underline"
        onClick={event => { event.preventDefault(); window.location.href = `mailto:${contact}` }}>
        {contact}
      </a>
    </div>
  )
}

export default function AboutPage({ page }: { page: string }) {
  const [markdown, setMarkdown] = useState<string | null>(null)
  useEffect(() => {
    let open = true
    const load = ABOUT_PAGES[page]
    if (load) void load().then(text => { if (open) setMarkdown(text) })
    else setMarkdown('---\ntitle: No such page\n---\n[About quietmap.org](/about)')
    return () => { open = false }
  }, [page])
  const { meta, body } = splitFrontMatter(markdown ?? '')
  useEffect(() => setDocumentTitle([page ? meta.title || null : 'About']), [page, meta.title])
  const crumbs = breadcrumb(page)
  const children = childrenOf(page)
  const [before, after] = body.includes(CHILDREN_MARKER) ? body.split(CHILDREN_MARKER) : [null, body]
  return (
    <main className="h-full overflow-y-auto bg-background">
      <div className="mx-auto max-w-2xl px-6 py-12">
        <a href="/" className="mb-6 inline-block text-sm text-primary hover:underline">&larr; Back to map</a>
        {markdown !== null && (
          <>
            {crumbs.length > 1 && (
              <nav className="mb-4 flex flex-wrap items-center gap-1 text-sm text-muted-foreground">
                {crumbs.map(([path, title], at) => (
                  <span key={path}>
                    {at > 0 && <span className="mx-1">/</span>}
                    {at < crumbs.length - 1
                      ? <a href={path ? `/about/${path}` : '/about'} className="text-primary hover:underline">{title}</a>
                      : <span className="font-medium text-foreground">{title}</span>}
                  </span>
                ))}
              </nav>
            )}
            {page === '' ? (
              <h1 className="mb-4 flex items-center gap-3">
                <img src="/favicon.svg" alt="" className="size-14 shrink-0" />
                <span className="text-4xl font-semibold tracking-[-0.04em] text-foreground">{meta.title}</span>
              </h1>
            ) : (
              <h1 className="mb-2 text-3xl font-bold text-foreground">{meta.title}</h1>
            )}
            {meta.intro && <p className="mb-6 text-lg text-muted-foreground">{meta.intro}</p>}
            {before === null && children.length > 0 && <ChildrenGrid items={children} />}
            <div className="about-prose">
              {before !== null && (
                <>
                  <Prose>{before}</Prose>
                  {children.length > 0 && <ChildrenGrid items={children} />}
                </>
              )}
              <Prose>{after}</Prose>
            </div>
            <Footer />
          </>
        )}
      </div>
    </main>
  )
}
