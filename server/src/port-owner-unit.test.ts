import assert from 'node:assert/strict'
import test from 'node:test'
import { userUnitOwningPort } from './port-owner-unit.js'

const LOADED_UNITS = 'qm-web-dev4.service loaded active running Quiet Map Fastify dev4\nqm-web-dev2.service loaded inactive dead Quiet Map Fastify dev2\n'
// A disabled, stopped unit leaves the manager's memory: installed only.
const UNIT_FILES = 'qm-web-dev4.service enabled enabled\nqm-web-dev2.service enabled enabled\nqm-web-dev3.service disabled disabled\n'
const SHOWN: Record<string, string> = {
  'qm-web-dev4.service': 'Environment=HOST=127.0.0.1 PORT=8560 DATA_YEAR=2026\nInvocationID=81b0669166fd41c2a008f48c9548822c\n',
  'qm-web-dev2.service': 'Environment=PORT=8520\nInvocationID=\n',
  'qm-web-dev3.service': 'Environment=PORT=8530\nInvocationID=\n',
}
const systemctl = (args: string[]) => {
  if (args[0] === 'list-units') return LOADED_UNITS
  if (args[0] === 'list-unit-files') return UNIT_FILES
  return SHOWN[args[args.length - 1]] ?? ''
}

test('a port a qm-web user unit owns is refused unless this process is that unit\'s invocation', () => {
  // Manual start on the running unit's port, and on a stopped unit's port (the dev2 case).
  assert.equal(userUnitOwningPort('8560', systemctl, undefined), 'qm-web-dev4.service')
  assert.equal(userUnitOwningPort('8520', systemctl, undefined), 'qm-web-dev2.service')
  // An installed but unloaded unit (stopped, disabled) still owns its port.
  assert.equal(userUnitOwningPort('8530', systemctl, undefined), 'qm-web-dev3.service')
  // The unit's own ExecStart carries its InvocationID and must serve.
  assert.equal(userUnitOwningPort('8560', systemctl, '81b0669166fd41c2a008f48c9548822c'), null)
  // A foreign invocation id (another unit) is still refused; a free port and no user manager pass.
  assert.equal(userUnitOwningPort('8560', systemctl, 'other'), 'qm-web-dev4.service')
  assert.equal(userUnitOwningPort('8501', systemctl, undefined), null)
  assert.equal(userUnitOwningPort('8560', () => '', undefined), null)
})
