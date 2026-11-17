// The bottom-sheet handle: tap, drag and dismiss.
import assert from 'node:assert/strict'
import test from 'node:test'

import { resolveSheetTouchEnd } from '../src/lib/sheet-drag.ts'

test('a tap on the sheet handle keeps its click; only a real drag cancels it, a long one dismisses', () => {
  assert.deepEqual(resolveSheetTouchEnd(3, 400), { cancelClick: false, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(-4, 400), { cancelClick: false, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(40, 400), { cancelClick: true, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(-40, 400), { cancelClick: true, dismiss: false })
  assert.deepEqual(resolveSheetTouchEnd(100, 400), { cancelClick: true, dismiss: true })
})

test('a short sheet dismisses within what a finger can drag before the screen ends', () => {
  // The shortest answer's sheet: its handle about 70 px above the screen's bottom.
  assert.deepEqual(resolveSheetTouchEnd(50, 120), { cancelClick: true, dismiss: true })
  assert.deepEqual(resolveSheetTouchEnd(30, 120), { cancelClick: true, dismiss: false })
})
