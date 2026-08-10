/** Regression tests for obstacle-height freshness proofs. */

import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtempSync, renameSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import {
  OBSTACLE_HEIGHT_PROOF_VERSION,
  obstacleHeightProofState,
  selectStaleObstacleHeightCells,
  stableFileIdentity,
  type ObstacleHeightCandidate,
} from './obstacle-height-materialization.js'

test('same-size atomic re-promotion with preserved mtime invalidates a current proof and selects the cell', () => {
  const dir = mkdtempSync(join(tmpdir(), 'obstacle-height-proof-'))
  try {
    const cell = '841e309ffffffff'
    const outputPath = join(dir, 'obstacles.arrow')
    const proofPath = join(dir, 'obstacles.height-materialization.json')
    const replacementPath = join(dir, 'promoted.arrow')
    const inputsSha256 = 'a'.repeat(64)
    const fixedSeconds = 1_700_000_000

    writeFileSync(outputPath, Buffer.from('tier-4-correct'))
    utimesSync(outputPath, fixedSeconds, fixedSeconds)
    writeFileSync(
      proofPath,
      JSON.stringify({
        version: OBSTACLE_HEIGHT_PROOF_VERSION,
        cell,
        inputsSha256,
        output: stableFileIdentity(outputPath),
        rows: 1,
        beforeTiers: [0, 0, 1, 0, 0],
        afterTiers: [0, 0, 0, 0, 1],
      }) + '\n',
    )
    const candidate: ObstacleHeightCandidate = { cell, outputPath, proofPath, inputsSha256 }
    assert.deepEqual(obstacleHeightProofState(candidate), { current: true, reason: 'current' })

    writeFileSync(replacementPath, Buffer.from('tier-2-revert!'))
    utimesSync(replacementPath, fixedSeconds, fixedSeconds)
    renameSync(replacementPath, outputPath)

    assert.equal(stableFileIdentity(outputPath).size, '14', 'fixture replacement must preserve byte size')
    assert.equal(stableFileIdentity(outputPath).mtimeNs, '1700000000000000000', 'fixture replacement must preserve mtime')
    assert.deepEqual(obstacleHeightProofState(candidate), { current: false, reason: 'output-replaced' })
    assert.deepEqual(selectStaleObstacleHeightCells([candidate]).map((c) => c.cell), [cell])
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('a missing or malformed proof is stale', () => {
  const dir = mkdtempSync(join(tmpdir(), 'obstacle-height-proof-'))
  try {
    const outputPath = join(dir, 'obstacles.arrow')
    const proofPath = join(dir, 'obstacles.height-materialization.json')
    writeFileSync(outputPath, 'payload')
    const candidate: ObstacleHeightCandidate = {
      cell: '841e309ffffffff',
      outputPath,
      proofPath,
      inputsSha256: 'b'.repeat(64),
    }
    assert.equal(obstacleHeightProofState(candidate).reason, 'missing-proof')
    writeFileSync(proofPath, '{not json')
    assert.equal(obstacleHeightProofState(candidate).reason, 'invalid-proof')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('an input replacement after materialization makes the proof stale', () => {
  const dir = mkdtempSync(join(tmpdir(), 'obstacle-height-proof-'))
  try {
    const cell = '841e309ffffffff'
    const outputPath = join(dir, 'obstacles.arrow')
    const proofPath = join(dir, 'obstacles.height-materialization.json')
    const beforeInputsSha256 = 'c'.repeat(64)
    writeFileSync(outputPath, 'payload')
    writeFileSync(
      proofPath,
      JSON.stringify({
        version: OBSTACLE_HEIGHT_PROOF_VERSION,
        cell,
        inputsSha256: beforeInputsSha256,
        output: stableFileIdentity(outputPath),
        rows: 1,
        beforeTiers: [0, 0, 1, 0, 0],
        afterTiers: [0, 0, 0, 0, 1],
      }) + '\n',
    )
    const afterInputReplacement: ObstacleHeightCandidate = {
      cell,
      outputPath,
      proofPath,
      inputsSha256: 'd'.repeat(64),
    }
    assert.deepEqual(obstacleHeightProofState(afterInputReplacement), {
      current: false,
      reason: 'input-changed',
    })
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
