// Precompress dist assets to .br at build time (brotli max quality). The
// server (@fastify/static preCompressed) then serves the sibling .br file
// verbatim — better ratio than on-the-fly q4-ish compression and zero
// per-request CPU. Only text-like assets; images/fonts are already packed.
//   run by `npm run build` after vite: node scripts/precompress.mjs dist
import { readdirSync, readFileSync, writeFileSync, statSync } from 'node:fs'
import { join, extname } from 'node:path'
import { brotliCompressSync, constants } from 'node:zlib'

const COMPRESS_EXT = new Set(['.js', '.css', '.html', '.svg', '.json', '.txt', '.map'])
const root = process.argv[2] ?? 'dist'

let files = 0
let rawB = 0
let brB = 0
const stack = [root]
while (stack.length) {
  const dir = stack.pop()
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) {
      stack.push(p)
      continue
    }
    if (!COMPRESS_EXT.has(extname(e.name))) continue
    const raw = readFileSync(p)
    const br = brotliCompressSync(raw, {
      params: {
        [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
        [constants.BROTLI_PARAM_SIZE_HINT]: raw.length,
      },
    })
    // A .br bigger than the original would be a net loss — skip (tiny files).
    if (br.length >= raw.length) continue
    writeFileSync(`${p}.br`, br)
    files++
    rawB += raw.length
    brB += br.length
  }
}
console.log(
  `precompress: ${files} files, ${(rawB / 1e6).toFixed(1)} MB → ${(brB / 1e6).toFixed(1)} MB (${((1 - brB / rawB) * 100).toFixed(0)} % smaller)`,
)
if (files === 0) {
  console.error(`precompress: nothing compressed under ${statSync(root) && root} — wrong dir?`)
  process.exit(1)
}
