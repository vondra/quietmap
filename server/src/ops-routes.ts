//! Optional ops-route modules: import-or-null — absence is a normal distribution shape, breakage is a bug.

import { fileURLToPath, pathToFileURL } from 'node:url'
import { basename, join } from 'node:path'

/**
 * True only when `error` reports the specifier ITSELF as unresolvable. Node ESM
 * (tsx and the compiled release) names the resolved absolute path; a CJS build
 * names the literal specifier. A present-but-broken module names ITS OWN missing
 * import instead — a different path — so the exact comparison keeps it a crash.
 */
function isAbsentModuleError(error: unknown, specifier: string, resolvedPath: string): boolean {
  const { code, message } = (error ?? {}) as { code?: unknown; message?: unknown }
  if (code !== 'ERR_MODULE_NOT_FOUND' && code !== 'MODULE_NOT_FOUND') return false
  const quoted = typeof message === 'string'
    ? /Cannot find module '([^']+)'/.exec(message)?.[1]
    : undefined
  return quoted === resolvedPath || quoted === specifier
}

/**
 * Dynamically imports an ops route module, resolving to null when the file is
 * absent — the public distribution ships without the ops routes, and a route's
 * presence in the tree is its only gate. Every other failure (a broken ops
 * file, a missing dependency of a present file, an evaluation throw) rethrows:
 * that is a real bug, not absence. Specifiers resolve relative to this file,
 * which sits next to app.ts, so './routes/x.js' sees exactly app.ts's view.
 * The specifier travels as a plain string so a distribution without the file
 * still typechecks — a literal import() would be resolved by tsc.
 *
 * Model B: an optional OPS_ROUTES_DIR can point at an external operations route
 * directory; it is consulted when the in-tree specifier is absent. Unset = the
 * public shape.
 */
export async function importOptionalOpsModule<T>(specifier: string): Promise<T | null> {
  const resolvedPath = fileURLToPath(new URL(specifier, import.meta.url))
  try {
    return (await import(specifier)) as T
  } catch (error) {
    if (!isAbsentModuleError(error, specifier, resolvedPath)) throw error
  }
  const opsDir = process.env.OPS_ROUTES_DIR
  if (!opsDir) return null
  const externalPath = join(opsDir, basename(resolvedPath))
  const externalSpecifier = pathToFileURL(externalPath).href
  try {
    return (await import(externalSpecifier)) as T
  } catch (error) {
    if (isAbsentModuleError(error, externalSpecifier, externalPath)) return null
    throw error
  }
}
