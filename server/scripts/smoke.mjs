// Start the compiled production entrypoint, probe it over loopback, and shut
// it down gracefully. Uses a temporary frontend and never needs project data.
import { existsSync } from 'node:fs'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ensureReleaseLock } from './release-lock.mjs'

ensureReleaseLock(fileURLToPath(import.meta.url))

const serverRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
for (const relative of [
  'dist.next/server.js',
  'dist.next/package.json',
  'dist.next/app.js',
  'dist.next/workers/noise-onfly-worker.mjs',
]) {
  if (!existsSync(resolve(serverRoot, relative))) {
    throw new Error(`compiled smoke missing ${relative}; run npm run build first`)
  }
}
const compiledValidationRoute = await readFile(resolve(serverRoot, 'dist.next/routes/validation-view.js'), 'utf8')
if (compiledValidationRoute.includes('../../../pipeline/')) {
  throw new Error('compiled validation route imports mutable pipeline code outside its immutable release')
}
async function freeLoopbackPort() {
  const server = createServer()
  await new Promise((resolveListen, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolveListen)
  })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('failed to allocate smoke port')
  await new Promise((resolveClose, reject) => server.close((error) => error ? reject(error) : resolveClose()))
  return address.port
}

const frontend = await mkdtemp(join(tmpdir(), '0db-server-smoke-'))
await writeFile(join(frontend, 'index.html'), '<!doctype html><title>compiled-smoke</title>')
const port = await freeLoopbackPort()
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm'
const child = spawn(npm, ['start'], {
  cwd: serverRoot,
  env: {
    ...process.env,
    PORT: String(port),
    HOST: '127.0.0.1',
    FRONTEND_DIST: frontend,
    ENABLE_CLUSTER_ROUTES: '0',
    ROBOTS_NOINDEX: '1',
    PRELOAD_RUNTIME_DATA: '0',
    SERVER_DIST: 'dist.next',
  },
  stdio: ['ignore', 'pipe', 'pipe'],
  detached: process.platform !== 'win32',
})
let output = ''
child.stdout.on('data', (chunk) => { output += chunk })
child.stderr.on('data', (chunk) => { output += chunk })

const exited = new Promise((resolveExit) => child.once('exit', (code, signal) => resolveExit({ code, signal })))

function signalProcessTree(signal) {
  if (process.platform === 'win32') {
    if (child.exitCode === null) child.kill(signal)
    return
  }
  try {
    process.kill(-child.pid, signal)
  } catch (error) {
    if (error?.code !== 'ESRCH') throw error
  }
}

async function waitForPortToClose() {
  const deadline = Date.now() + 5_000
  while (Date.now() < deadline) {
    try {
      await fetch(`http://127.0.0.1:${port}/api/live`, {
        signal: AbortSignal.timeout(250),
      })
    } catch {
      return
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50))
  }
  throw new Error('compiled server kept listening after SIGTERM')
}

try {
  const deadline = Date.now() + 8_000
  let live = null
  while (Date.now() < deadline) {
    const earlyExit = child.exitCode !== null
    if (earlyExit) break
    try {
      live = await fetch(`http://127.0.0.1:${port}/api/live`)
      if (live.ok) break
    } catch {
      // Server has not bound yet.
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 100))
  }
  if (!live?.ok) throw new Error(`compiled server did not become live\n${output}`)
  if (JSON.stringify(await live.json()) !== '{"status":"ok"}') {
    throw new Error('compiled liveness returned an unexpected body')
  }

  const root = await fetch(`http://127.0.0.1:${port}/`)
  if (!root.ok || !(await root.text()).includes('compiled-smoke')) {
    throw new Error('compiled server did not serve the configured frontend')
  }
  if (root.headers.get('x-robots-tag') !== 'noindex, nofollow, noarchive') {
    throw new Error('compiled server lost the deployment noindex header')
  }
  const unknown = await fetch(`http://127.0.0.1:${port}/.env`)
  if (unknown.status !== 404) throw new Error(`compiled server returned ${unknown.status} for /.env`)
} finally {
  signalProcessTree('SIGTERM')
  let shutdownTimer
  try {
    const [result] = await Promise.race([
      Promise.all([exited, waitForPortToClose()]),
      new Promise((_, reject) => {
        shutdownTimer = setTimeout(
          () => reject(new Error('compiled server did not stop after SIGTERM')),
          5_000,
        )
      }),
    ])
    // `npm start` is the process-group leader, so the intentional group
    // SIGTERM may terminate npm itself even though Fastify shuts down cleanly.
    if (result.code !== 0 && result.signal !== 'SIGTERM') {
      throw new Error(`compiled server exited code=${result.code} signal=${result.signal}\n${output}`)
    }
  } finally {
    clearTimeout(shutdownTimer)
    signalProcessTree('SIGKILL')
    await rm(frontend, { recursive: true, force: true })
  }
}

console.log('compiled server smoke: OK')
