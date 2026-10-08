// An About page's front matter and body: the `---` block that opens its markdown holds `key: value`
// lines (title, intro; nav: hidden keeps a page out of its parent's list), as dev4's docs had.
export interface AboutMeta {
  title: string
  intro: string
  nav?: string
}

export function splitFrontMatter(markdown: string): { meta: AboutMeta; body: string } {
  const match = markdown.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/)
  if (!match) return { meta: { title: '', intro: '' }, body: markdown }
  const fields: Record<string, string> = {}
  for (const line of match[1].split('\n')) {
    const field = line.match(/^(\w+):\s*(.+)$/)
    if (field) fields[field[1]] = field[2]
  }
  return { meta: { title: fields.title ?? '', intro: fields.intro ?? '', nav: fields.nav }, body: match[2] }
}
