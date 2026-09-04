import { useEffect, useState } from 'react'
import Markdown, { type Components } from 'react-markdown'
import { setDocumentTitle } from '../utils/page-title'
import remarkGfm from 'remark-gfm'
import rehypeRaw from 'rehype-raw'

interface DocData {
  title: string
  intro: string
  body: string
  children: { slug: string; title: string; status: string | null }[]
  breadcrumb: { slug: string; title: string }[]
}

// Authored into the markdown body (see docs/about/index.md, "Explore by
// region") to place the children grid inline instead of always at the top —
// a section-list page (a continent listing its countries) has no marker, so
// it keeps the top position.
const REGION_CHILDREN_MARKER = '<!-- REGION_CHILDREN -->'

const markdownComponents: Components = {
  img: ({ src, alt, ...props }) => (
    <img
      src={src && !src.startsWith('http') ? `/api/docs/assets/${src}` : src}
      alt={alt}
      {...props}
    />
  ),
}

function DocMarkdown({ children }: { children: string }) {
  return (
    <Markdown remarkPlugins={[remarkGfm]} rehypePlugins={[rehypeRaw]} components={markdownComponents}>
      {children}
    </Markdown>
  )
}

function ChildrenGrid({ items, basePath }: { items: DocData['children']; basePath: string }) {
  return (
    <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
      {items.map(child => (
        <a
          key={child.slug}
          href={`${basePath}/${child.slug}`}
          className="flex items-center gap-2 px-3 py-2 rounded-lg border border-border hover:bg-accent transition-colors"
        >
          <span className="text-sm font-medium text-foreground">{child.title}</span>
          {child.status && (
            <span className={`text-[10px] px-1.5 py-0.5 rounded-full ${
              child.status === 'active' ? 'bg-green-100 text-green-700' : 'bg-muted text-muted-foreground'
            }`}>
              {child.status}
            </span>
          )}
        </a>
      ))}
    </div>
  )
}

const PROSE = [
  'prose prose-sm max-w-none',
  'prose-headings:text-foreground',
  // Classic descending hierarchy (owner 2026-07-10): the previous
  // h2-as-small-caps-eyebrow rendered section titles SMALLER than their
  // subsections and read as inverted importance.
  // Compact rhythm (owner 2026-07-16: page read as oversized gaps) — headings
  // carry the section break on their own; hr and table padding tightened to
  // match instead of stacking a second, looser break on top.
  'prose-h2:text-2xl prose-h2:font-bold prose-h2:mt-10 prose-h2:mb-2',
  'prose-h3:text-lg prose-h3:font-semibold prose-h3:mt-6 prose-h3:mb-2',
  'prose-p:text-muted-foreground prose-p:leading-relaxed prose-p:my-3',
  'prose-a:text-primary prose-a:no-underline hover:prose-a:underline',
  'prose-ul:my-3 prose-ol:my-3 prose-li:text-muted-foreground prose-li:my-1',
  'prose-strong:text-foreground',
  // Compact table (owner 2026-07-17): body rows carry no border — a
  // borderless table with tight py reads as one dense block, not a
  // ladder of thin lines; the header keeps its rule to mark the label row.
  'prose-table:text-sm prose-table:my-3 prose-th:text-left prose-th:font-semibold prose-th:text-foreground prose-th:py-1 prose-td:text-muted-foreground prose-td:py-1 [&_tbody_tr]:border-0',
  'prose-code:text-xs prose-code:bg-muted prose-code:text-foreground prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded-md',
  'prose-pre:bg-muted prose-pre:text-xs prose-pre:rounded-lg prose-pre:border prose-pre:border-border',
  '[&_hr]:border-border [&_hr]:my-8',
  '[&_details]:border [&_details]:border-border [&_details]:rounded-lg [&_details]:px-4 [&_details]:py-2 [&_details]:my-3',
  '[&_details_summary]:cursor-pointer [&_details_summary]:text-foreground [&_details_summary]:font-medium [&_details_summary]:text-sm',
].join(' ')

/** Breadcrumb + title + body for a loaded doc — kept separate from the
 *  fetch/loading shell so the marker-split derivation below lives next to
 *  its only consumer instead of running unconditionally ahead of it. */
function AboutBody({ doc }: { doc: DocData }) {
  const basePath = window.location.pathname.replace(/\/$/, '')
  const hasInlineChildrenMarker = doc.children.length > 0 && doc.body.includes(REGION_CHILDREN_MARKER)
  const [bodyBeforeChildren, bodyAfterChildren] = hasInlineChildrenMarker
    ? doc.body.split(REGION_CHILDREN_MARKER)
    : ['', '']
  const childrenGrid = doc.children.length > 0
    ? <ChildrenGrid items={doc.children} basePath={basePath} />
    : null

  return (
    <>
      {doc.breadcrumb.length > 1 && (
        <nav className="flex items-center gap-1 text-sm text-muted-foreground mb-4">
          {doc.breadcrumb.map((crumb, i) => (
            <span key={crumb.slug}>
              {i > 0 && <span className="mx-1">/</span>}
              {i < doc.breadcrumb.length - 1 ? (
                <a href={crumb.slug} className="text-primary hover:underline">{crumb.title}</a>
              ) : (
                <span className="text-foreground font-medium">{crumb.title}</span>
              )}
            </span>
          ))}
        </nav>
      )}

      {doc.breadcrumb.length === 1 ? (
        <h1 className="mb-4 flex items-center gap-3">
          <img
            src="/favicon.svg"
            alt=""
            className="size-14 shrink-0"
          />
          <span className="text-4xl font-semibold tracking-[-0.04em] text-foreground">{doc.title}</span>
        </h1>
      ) : (
        <h1 className="text-3xl font-bold text-foreground mb-2">{doc.title}</h1>
      )}
      {doc.intro && <p className="text-lg text-muted-foreground mb-6">{doc.intro}</p>}

      {childrenGrid && !hasInlineChildrenMarker && (
        <div className="mb-8">{childrenGrid}</div>
      )}

      {doc.body && (
        <div className={PROSE}>
          {hasInlineChildrenMarker ? (
            <>
              <DocMarkdown>{bodyBeforeChildren}</DocMarkdown>
              {childrenGrid}
              <DocMarkdown>{bodyAfterChildren}</DocMarkdown>
            </>
          ) : (
            <DocMarkdown>{doc.body}</DocMarkdown>
          )}
        </div>
      )}
    </>
  )
}

/** Footer with the contact address assembled at render time from parts —
 *  the literal string never appears in the bundle or static HTML, which
 *  defeats source-grepping harvesters (the mailbot + Resend filtering handle
 *  whatever gets through anyway). */
function PageFooter() {
  const contact = ['info', 'quietmap.org'].join('@')
  return (
    <div className="mt-12 flex items-center justify-between border-t border-border pt-6 text-sm text-muted-foreground/60">
      <a href="https://quietmap.org" className="hover:underline">quietmap.org</a>
      <a
        href="#contact"
        onClick={(e) => { e.preventDefault(); window.location.href = `mailto:${contact}` }}
        className="hover:underline"
      >
        {contact}
      </a>
    </div>
  )
}

export default function AboutPage() {
  const [doc, setDoc] = useState<DocData | null>(null)
  const [error, setError] = useState<string | null>(null)

  const subpath = window.location.pathname.replace(/^\/about\/?/, '')
  const apiUrl = subpath ? `/api/docs/about/${subpath}` : '/api/docs/about'

  useEffect(() => {
    fetch(apiUrl)
      .then(res => { if (!res.ok) throw new Error(`${res.status}`); return res.json() })
      .then((data: DocData) => {
        setDoc(data)
        // Root About = "About - quietmap.org"; subpages carry their own title
        // ("Czechia - quietmap.org"). /about is a full page load, no restore needed.
        setDocumentTitle([data.breadcrumb.length > 1 ? data.title : 'About'])
      })
      .catch(err => setError(err.message))
  }, [apiUrl])

  return (
    <div style={{ position: 'fixed', inset: 0, overflow: 'auto' }} className="bg-background">
      <div className="max-w-2xl mx-auto px-6 py-12">
        <a href="/" className="text-sm text-primary hover:underline mb-6 inline-block">&larr; Back to map</a>

        {error && <div className="text-destructive">Error: {error}</div>}

        {doc && <AboutBody doc={doc} />}

        {!doc && !error && <div className="text-muted-foreground">Loading...</div>}

        <PageFooter />
      </div>
    </div>
  )
}
