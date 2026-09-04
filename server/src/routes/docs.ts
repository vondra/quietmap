import fs from 'node:fs'
import path from 'node:path'
import type { FastifyInstance } from 'fastify'

const DOCS_DIR = path.resolve(import.meta.dirname, '..', '..', '..', 'docs')
const ABOUT_DIR = path.resolve(DOCS_DIR, 'about')

interface DocFrontmatter {
  title: string
  intro: string
  map?: { center: [number, number]; zoom: number }
  status?: string
  // 'hidden' keeps a page reachable by URL (e.g. /about/methodology) without
  // listing it in its parent's auto-generated children grid — used for
  // hand-linked topic pages living alongside the region/country tree.
  nav?: string
}

function parseFrontmatter(raw: string): { data: DocFrontmatter; content: string } {
  const match = raw.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/)
  if (!match) return { data: { title: '', intro: '' }, content: raw }

  const yaml = match[1]
  const data: Record<string, unknown> = {}
  for (const line of yaml.split('\n')) {
    const kv = line.match(/^(\w+):\s*(.+)$/)
    if (!kv) continue
    const [, key, val] = kv
    if (val.startsWith('{')) {
      try { data[key] = JSON.parse(val.replace(/(\w+):/g, '"$1":')) } catch { data[key] = val }
    } else {
      data[key] = val
    }
  }
  return { data: data as unknown as DocFrontmatter, content: match[2] }
}

function getChildren(dirPath: string): { slug: string; title: string; status: string | null }[] {
  if (!fs.existsSync(dirPath)) return []
  const entries = fs.readdirSync(dirPath, { withFileTypes: true })
  const children: { slug: string; title: string; status: string | null }[] = []

  for (const entry of entries) {
    if (entry.name === 'index.md') continue
    let slug: string
    let resolved: string | null
    if (entry.isDirectory()) {
      slug = entry.name
      resolved = resolveAboutRegularFile(path.join(dirPath, entry.name, 'index.md'))
    } else if (entry.isFile() && entry.name.endsWith('.md')) {
      slug = entry.name.replace(/\.md$/, '')
      resolved = resolveAboutRegularFile(path.join(dirPath, entry.name))
    } else {
      continue
    }
    if (!resolved) continue
    const { data } = parseFrontmatter(fs.readFileSync(resolved, 'utf-8'))
    if (data.nav === 'hidden') continue
    children.push({ slug, title: data.title || slug, status: data.status ?? null })
  }
  return children.sort((a, b) => a.title.localeCompare(b.title))
}

function buildBreadcrumb(segments: string[]): { slug: string; title: string }[] {
  const crumbs: { slug: string; title: string }[] = []
  let currentPath = ABOUT_DIR

  const rootIndex = resolveAboutRegularFile(path.join(currentPath, 'index.md'))
  if (rootIndex) {
    const { data } = parseFrontmatter(fs.readFileSync(rootIndex, 'utf-8'))
    crumbs.push({ slug: '/about', title: data.title || 'About' })
  }

  for (let i = 0; i < segments.length; i++) {
    currentPath = path.join(currentPath, segments[i])
    const slug = '/about/' + segments.slice(0, i + 1).join('/')
    const indexFile = resolveAboutRegularFile(path.join(currentPath, 'index.md'))
    const mdFile = resolveAboutRegularFile(currentPath + '.md')

    if (indexFile) {
      const { data } = parseFrontmatter(fs.readFileSync(indexFile, 'utf-8'))
      crumbs.push({ slug, title: data.title || segments[i] })
    } else if (mdFile) {
      const { data } = parseFrontmatter(fs.readFileSync(mdFile, 'utf-8'))
      crumbs.push({ slug, title: data.title || segments[i] })
    }
  }
  return crumbs
}

function isInside(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate)
  return relative === '' || (!path.isAbsolute(relative) && relative !== '..' && !relative.startsWith(`..${path.sep}`))
}

/** Resolve a public file and reject final or intermediate symlink escapes. */
export function resolveAboutRegularFile(candidate: string, aboutDir = ABOUT_DIR): string | null {
  try {
    const lexicalRoot = path.resolve(aboutDir)
    const lexical = path.resolve(candidate)
    if (!isInside(lexicalRoot, lexical)) return null
    const info = fs.lstatSync(lexical)
    if (!info.isFile() || info.isSymbolicLink()) return null
    const realRoot = fs.realpathSync(lexicalRoot)
    const real = fs.realpathSync(lexical)
    return isInside(realRoot, real) ? real : null
  } catch {
    return null
  }
}

function docSegments(wildcard: string): string[] | null {
  if (wildcard.includes('\\')) return null
  const withoutSuffix = wildcard.replace(/\.md$/, '')
  const segments = withoutSuffix.split('/').filter(Boolean)
  if (segments.some((segment) => !/^[a-z0-9-]+$/.test(segment))) return null
  if (segments.at(-1) === 'index') segments.pop()
  return segments
}

function assetPath(wildcard: string): string | null {
  if (wildcard.includes('\\')) return null
  const segments = wildcard.split('/').filter(Boolean)
  if (segments.length === 0 || segments.some((segment) =>
    segment === '.' || segment === '..' || !/^[A-Za-z0-9._-]+$/.test(segment))) return null
  const candidate = path.resolve(ABOUT_DIR, ...segments)
  return isInside(ABOUT_DIR, candidate) ? candidate : null
}

export async function docsRoutes(app: FastifyInstance): Promise<void> {
  // Serve static assets (images) from docs/about/
  app.get('/api/docs/assets/*', async (request, reply) => {
    const wildcard = (request.params as Record<string, string>)['*']
    if (!/\.(png|jpg|jpeg|webp|svg|gif)$/i.test(wildcard)) {
      return reply.status(404).send({ error: 'Not found' })
    }
    const filePath = assetPath(wildcard)
    const publicFile = filePath ? resolveAboutRegularFile(filePath) : null
    if (!publicFile) {
      return reply.status(404).send({ error: 'Not found' })
    }
    const ext = path.extname(publicFile).slice(1).toLowerCase()
    const mime = ext === 'svg' ? 'image/svg+xml' : `image/${ext === 'jpg' ? 'jpeg' : ext}`
    return reply.type(mime).send(fs.readFileSync(publicFile))
  })

  app.get('/api/docs/about', async () => serveDoc([]))
  app.get('/api/docs/about/*', async (request) => {
    const wildcard = (request.params as Record<string, string>)['*']
    // Tolerate links written with a literal .md suffix (authors copy file
    // paths): /about/europe/index.md → /about/europe, /about/europe/cz.md →
    // /about/europe/cz — resolution below is extension-less.
    const segments = docSegments(wildcard)
    if (!segments) throw { statusCode: 404, message: 'Page not found' }
    return serveDoc(segments)
  })
}

function serveDoc(segments: string[]) {
  let filePath: string
  let dirPath: string

  if (segments.length === 0) {
    const rootIndex = resolveAboutRegularFile(path.join(ABOUT_DIR, 'index.md'))
    if (!rootIndex) throw { statusCode: 404, message: 'Page not found' }
    filePath = rootIndex
    dirPath = ABOUT_DIR
  } else {
    const joined = path.join(ABOUT_DIR, ...segments)
    const indexPath = resolveAboutRegularFile(path.join(joined, 'index.md'))
    const markdownPath = resolveAboutRegularFile(joined + '.md')
    if (indexPath) {
      filePath = indexPath
      dirPath = path.dirname(indexPath)
    } else if (markdownPath) {
      filePath = markdownPath
      dirPath = ''
    } else {
      throw { statusCode: 404, message: 'Page not found' }
    }
  }

  const raw = fs.readFileSync(filePath, 'utf-8')
  const { data, content } = parseFrontmatter(raw)

  return {
    title: data.title || '',
    intro: data.intro || '',
    map: data.map ?? null,
    status: data.status ?? null,
    body: content.trim(),
    children: getChildren(dirPath),
    breadcrumb: buildBreadcrumb(segments),
  }
}
