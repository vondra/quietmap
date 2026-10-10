// The list's shares as whole percents: they add up to 100 however many small rows there are.
import assert from 'node:assert/strict'
import test from 'node:test'

import { fmtPercent, wholePercents } from '../src/utils/formatters.ts'

test('the rows\' whole percents add up to 100, the largest remainders rounded up', () => {
  // Thirty rows of 1.6 % and one of 52 %: rounded one by one they would read 112 %.
  const percents = wholePercents([...Array(30).fill(0.016), 0.52])
  assert.equal(percents.reduce((sum, percent) => sum + percent, 0), 100)
  assert.deepEqual(wholePercents([0.481, 0.192, 0.129, 0.088, 0.071, 0.039]), [48, 19, 13, 9, 7, 4])
  assert.deepEqual(wholePercents([0, 0]), [0, 0])
  assert.equal(fmtPercent(0), '<1 %')
  assert.equal(fmtPercent(41), '41 %')
})
