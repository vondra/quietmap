#!/usr/bin/env node
//! Validate layer topology metadata without changing any runtime projection.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const spec = JSON.parse(readFileSync(join(root, 'scripts', 'layer-spec.json'), 'utf8'))

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
  assert(buildableArrows.has(group.seed_arrow),
    `groups.${groupName}.seed_arrow ${group.seed_arrow} is not buildable for the group`)
}

console.log(`layer-spec: ${Object.keys(spec.layers).length} layers and ${Object.keys(spec.groups).length} groups OK`)
