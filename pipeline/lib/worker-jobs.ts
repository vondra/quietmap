/** CPU/memory worker cap shared by Node square shards. */

import { availableParallelism } from 'node:os'
import { readFileSync } from 'node:fs'
import { resolve, dirname } from 'node:path'

const CGROUP_ROOT = '/sys/fs/cgroup'

export function cpuJobs(): number {
  try {
    return availableParallelism()
  } catch {
    return 1
  }
}

export function availableMemoryBytes(): number {
  return cgroupMemoryMax() ?? hostAvailableBytes()
}

export function fitJobs(requested: number, bytesPerWorker: number, memoryBytes = availableMemoryBytes()): number {
  if (!Number.isInteger(requested) || requested < 1) throw new Error('--jobs must be >= 1')
  if (!Number.isInteger(bytesPerWorker) || bytesPerWorker < 1) throw new Error('bytes_per_worker must be >= 1')
  if (!Number.isInteger(memoryBytes) || memoryBytes < 1) throw new Error('memory limit must be >= 1')
  return Math.max(1, Math.min(requested, Math.floor(memoryBytes / bytesPerWorker)))
}

function cgroupMemoryMax(): number | undefined {
  try {
    const text = readFileSync('/proc/self/cgroup', 'utf8')
    const line = text.split('\n').find(row => row.startsWith('0::'))
    if (!line) return undefined
    let path = resolve(CGROUP_ROOT, line.slice(3).replace(/^\//, ''))
    while (true) {
      try {
        const raw = readFileSync(resolve(path, 'memory.max'), 'utf8').trim()
        if (raw !== 'max') {
          const bytes = Number(raw)
          if (Number.isFinite(bytes)) return bytes
        }
      } catch {
        // Walk toward the root until a numeric limit exists.
      }
      if (path === CGROUP_ROOT) return undefined
      const parent = dirname(path)
      if (parent === path) return undefined
      path = parent
    }
  } catch {
    return undefined
  }
}

function hostAvailableBytes(): number {
  try {
    const line = readFileSync('/proc/meminfo', 'utf8').split('\n').find(row => row.startsWith('MemAvailable:'))
    const kilobytes = line?.split(/\s+/)[1]
    if (kilobytes) return Number(kilobytes) * 1024
  } catch {
    // Fall through to os.totalmem-equivalent via /proc.
  }
  const total = readFileSync('/proc/meminfo', 'utf8').split('\n').find(row => row.startsWith('MemTotal:'))
  return Number(total?.split(/\s+/)[1] ?? 0) * 1024
}
