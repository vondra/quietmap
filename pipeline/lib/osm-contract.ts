/** Extraction versions are the native reader's explicit constants, shared by every producer. */
import { readFileSync } from 'node:fs'
import type { Table } from 'apache-arrow'

const source = readFileSync(new URL('../../engine/square-store/src/osm_contract.rs', import.meta.url), 'utf8')
const versions = new Map(Array.from(source.matchAll(/pub const (\w+): &str = "([^"]*)";/g), match => [match[1], match[2]]))
export function osmContract(family: 'roads' | 'railways' | 'industrial'): [string, string] {
  const version = versions.get(`${family.toUpperCase()}_CONTRACT`)
  if (!version) throw new Error(`missing native ${family} extraction contract`)
  return [`osm_${family}_contract`, version]
}
export function requireOsmContract(table: Table, family: 'roads' | 'railways' | 'industrial'): void {
  const [key, value] = osmContract(family)
  if (table.schema.metadata.get(key) !== value) throw new Error(`${family} requires ${key}=${value}; re-extract OSM`)
}
