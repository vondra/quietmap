//! Shared primitive assertions for popup artifact runtime schemas.

export function fail(path, message) {
  throw new Error(`${path}: ${message}`)
}

export function object(value, path) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    fail(path, 'expected object')
  }
  return value
}

export function exactKeys(value, path, required, optional = []) {
  object(value, path)
  const allowed = new Set([...required, ...optional])
  for (const key of required) {
    if (!Object.hasOwn(value, key)) fail(path, `missing key "${key}"`)
  }
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) fail(path, `unknown key "${key}"`)
  }
}

export function array(value, path, length) {
  if (!Array.isArray(value)) fail(path, 'expected array')
  if (length !== undefined && value.length !== length) {
    fail(path, `expected length ${length}, got ${value.length}`)
  }
  return value
}

export function finite(value, path) {
  if (!Number.isFinite(value)) fail(path, 'expected finite number')
}

export function nullableFinite(value, path) {
  if (value !== null) finite(value, path)
}

export function integer(value, path, minimum = Number.MIN_SAFE_INTEGER) {
  if (!Number.isSafeInteger(value) || value < minimum) fail(path, `expected integer >= ${minimum}`)
}

export function string(value, path) {
  if (typeof value !== 'string') fail(path, 'expected string')
}

export function boolean(value, path) {
  if (typeof value !== 'boolean') fail(path, 'expected boolean')
}

export function coordinatePair(value, path) {
  array(value, path, 2)
  finite(value[0], `${path}[0]`)
  finite(value[1], `${path}[1]`)
  if (value[0] < -90 || value[0] > 90) fail(path, 'latitude outside [-90, 90]')
  if (value[1] < -180 || value[1] > 180) fail(path, 'longitude outside [-180, 180]')
}

export function numericObject(value, path, keys) {
  exactKeys(value, path, keys)
  for (const key of keys) finite(value[key], `${path}.${key}`)
}
