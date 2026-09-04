#!/usr/bin/env node
//! Validate layer topology metadata without changing any runtime projection.

import { readFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const spec = JSON.parse(readFileSync(join(root, 'scripts', 'layer-spec.json'), 'utf8'))

const roleContract = join(root, 'scripts', 'gpu_model_role.py')
const roleContractResult = spawnSync('python3', [
  roleContract,
  'deployment-contract',
  join(root, 'scripts', 'model-role-spec.json'),
  join(root, 'scripts', 'layer-spec.json'),
], { encoding: 'utf8' })
if (roleContractResult.status !== 0) {
  throw new Error(`layer-spec: model-role deployment contract failed: ${roleContractResult.stderr.trim()}`)
}
const selectedArtifacts = JSON.parse(roleContractResult.stdout)

function assert(condition, message) {
  if (!condition) throw new Error(`layer-spec: ${message}`)
}

function assertUniqueNonEmptyStrings(values, path) {
  assert(Array.isArray(values), `${path} must be an array`)
  assert(values.every(value => typeof value === 'string' && value.length > 0),
    `${path} must contain only non-empty strings`)
  assert(new Set(values).size === values.length, `${path} must not contain duplicates`)
}

const buildableArrowsByLayer = {}
for (const [layerName, layer] of Object.entries(spec.layers)) {
  assertUniqueNonEmptyStrings(layer.arrows, `layers.${layerName}.arrows`)
  assertUniqueNonEmptyStrings(layer.dependency_arrows,
    `layers.${layerName}.dependency_arrows`)

  const readArrows = new Set(layer.arrows)
  for (const dependencyArrow of layer.dependency_arrows) {
    assert(readArrows.has(dependencyArrow),
      `layers.${layerName}.dependency_arrows contains ${dependencyArrow}, which is absent from arrows`)
  }

  const dependencies = new Set(layer.dependency_arrows)
  const buildableArrows = layer.arrows.filter(arrow => !dependencies.has(arrow))
  assert(buildableArrows.length > 0,
    `layers.${layerName} has no buildable arrow after removing dependencies`)
  buildableArrowsByLayer[layerName] = buildableArrows
}

for (const [groupName, group] of Object.entries(spec.groups)) {
  assertUniqueNonEmptyStrings(group.layers, `groups.${groupName}.layers`)
  const buildableArrows = new Set()
  for (const layerName of group.layers) {
    assert(buildableArrowsByLayer[layerName],
      `groups.${groupName}.layers references unknown layer ${layerName}`)
    for (const arrow of buildableArrowsByLayer[layerName]) buildableArrows.add(arrow)
  }
  // A seed arrow exists to fill a worker's {SEED} placeholder and for nothing
  // else, so a group whose workers take no {SEED} must not carry one.
  const seedsAWorker = Object.values(spec.worker_types)
    .some(worker => worker.group === groupName && worker.flags.includes('{SEED}'))
  if (seedsAWorker) {
    assert(buildableArrows.has(group.seed_arrow),
      `groups.${groupName}.seed_arrow ${group.seed_arrow} is not buildable for the group`)
  } else {
    assert(group.seed_arrow === undefined,
      `groups.${groupName} has no worker taking {SEED}, so its seed_arrow is read by nobody`)
  }
}

assert(Object.keys(selectedArtifacts.workers).length === Object.keys(spec.worker_types).length,
  'model-role deployment contract does not cover every worker type')
assert(/^[0-9a-f]{64}$/.test(selectedArtifacts.line_model_role_sha256),
  'line-model role digest is absent')
for (const [workerName, worker] of Object.entries(spec.worker_types)) {
  const identity = selectedArtifacts.workers[workerName]
  assert(identity, `worker_types.${workerName} has no selected model-role identity`)
  assert(identity.artifact_family === worker.artifact_family,
    `worker_types.${workerName}.artifact_family was not preserved by resolution`)
  assert(identity.binary === worker.binary,
    `worker_types.${workerName}.binary disagrees with its selected role`)
}

const codever = join(root, 'scripts', 'layer-codever.py')
const engine = join(root, 'engine')
const sharedCheck = spawnSync('python3', [codever, engine, '--check'], { encoding: 'utf8' })
assert(sharedCheck.status === 0,
  `layer-codever shared-input gate failed: ${sharedCheck.stderr.trim()}`)
const exclusiveMutation = spawnSync('python3', [codever, engine, '--check',
  'road=../scripts/model-role-spec.json'], { encoding: 'utf8' })
assert(exclusiveMutation.status !== 0
    && exclusiveMutation.stderr.includes(
      'GLOBAL input classified exclusive: scripts/model-role-spec.json (layer road)'),
'layer-codever accepted the model-role spec as a layer-exclusive input')

console.log(`layer-spec: ${Object.keys(spec.layers).length} layers and `
  + `${Object.keys(spec.groups).length} groups OK`)
