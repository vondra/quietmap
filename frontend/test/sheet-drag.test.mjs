import assert from 'node:assert/strict'
import test from 'node:test'

import { resolveSheetTouchEnd } from '../src/lib/sheet-drag.ts'

test('a tap on the sheet handle keeps its click; only a real drag cancels it, a long one dismisses', () => {
  assert.deepEqual(resolveSheetTouchEnd(3), { cancelClick: false, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(-4), { cancelClick: false, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(40), { cancelClick: true, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(-40), { cancelClick: true, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(100), { cancelClick: true, dismiss: true })
})
