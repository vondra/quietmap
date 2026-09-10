import { spawnSync } from 'node:child_process'

/** `systemctl --user <args>` stdout, or '' where there is no user manager (CI, containers). */
export type SystemctlUser = (args: string[]) => string

const systemctlUser: SystemctlUser = (args) => {
  const result = spawnSync('systemctl', ['--user', ...args], {
    encoding: 'utf8',
    // A manual shell may lack the user bus variables the unit itself gets from systemd.
    env: { ...process.env, XDG_RUNTIME_DIR: process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid?.() ?? ''}` },
  })
  return result.status === 0 ? result.stdout : ''
}

/**
 * The `qm-web-*` user unit whose Environment carries PORT=<port>, unless this
 * process is that unit's own invocation. A manual server that grabs the port
 * while the unit is down (2026-09-04, dev2: 3 042 restarts on EADDRINUSE)
 * serves stale code, so the caller refuses to bind instead of fighting.
 * Installed unit files count as well as loaded units: a stopped disabled unit
 * leaves the manager's memory but still owns its port once started.
 */
export function userUnitOwningPort(
  port: string,
  systemctl: SystemctlUser = systemctlUser,
  invocationId: string | undefined = process.env.INVOCATION_ID,
): string | null {
  const firstColumn = (listing: string) => listing.split('\n').map((line) => line.trim().split(/\s+/)[0])
  const units = new Set([
    ...firstColumn(systemctl(['list-units', '--all', '--plain', '--no-legend', 'qm-web-*'])),
    ...firstColumn(systemctl(['list-unit-files', '--plain', '--no-legend', 'qm-web-*'])),
  ])
  units.delete('')
  for (const unit of units) {
    const shown = systemctl(['show', '-p', 'Environment', '-p', 'InvocationID', unit])
    const environment = /^Environment=(.*)$/m.exec(shown)?.[1] ?? ''
    if (!environment.split(' ').includes(`PORT=${port}`)) continue
    const unitInvocationId = /^InvocationID=(.*)$/m.exec(shown)?.[1] ?? ''
    if (unitInvocationId !== '' && unitInvocationId === invocationId) return null
    return unit
  }
  return null
}
