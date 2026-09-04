//! Write the immutable frontend release cohort consumed by server readiness.

import { createHash } from 'node:crypto'
import { mkdir, open, readdir, readFile, rename, rm, stat } from 'node:fs/promises'
import { dirname, join, relative, resolve, sep } from 'node:path'

const root = resolve(process.argv[2] ?? 'dist')
const manifestName = 'asset-manifest.json'
const manifestPath = join(root, manifestName)
const temporaryPath = join(root, `.${manifestName}.${process.pid}.tmp`)

async function filesUnder(directory) {
  const files = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) files.push(...await filesUnder(path))
    else if (entry.isFile()) files.push(path)
    else throw new Error(`frontend build contains unsupported entry: ${path}`)
  }
  return files
}

await mkdir(root, { recursive: true })
await rm(manifestPath, { force: true })
await rm(temporaryPath, { force: true })

const files = []
for (const path of (await filesUnder(root)).sort()) {
  const file = relative(root, path).split(sep).join('/')
  if (!file || file === manifestName || file.startsWith('../')) {
    throw new Error(`unsafe frontend build path: ${path}`)
  }
  const contents = await readFile(path)
  if (contents.length === 0) throw new Error(`frontend build contains an empty file: ${file}`)
  files.push({
    file,
    bytes: contents.length,
    sha256: createHash('sha256').update(contents).digest('hex'),
  })
}

if (!files.some(({ file }) => file === 'index.html')) {
  throw new Error(`${root} has no index.html`)
}
if (!files.some(({ file }) => /^assets\/.*\.js$/.test(file))) {
  throw new Error(`${root} has no JavaScript bundle`)
}

const contents = Buffer.from(`${JSON.stringify({ version: 1, files }, null, 2)}\n`)
let handle
try {
  handle = await open(temporaryPath, 'wx')
  await handle.writeFile(contents)
  await handle.sync()
  await handle.close()
  handle = undefined
  await rename(temporaryPath, manifestPath)
  const directory = await open(dirname(manifestPath), 'r')
  await directory.sync()
  await directory.close()
} finally {
  await handle?.close()
  await rm(temporaryPath, { force: true })
}

const info = await stat(manifestPath)
console.log(`asset-manifest: ${files.length} files, ${info.size} manifest bytes`)
