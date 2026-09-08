//! Cohort-pinned popup client with deterministic public-rate-limit pacing.

const SHA256 = /^[0-9a-f]{64}$/
export const REQUEST_INTERVAL_MS = 250
export const REQUEST_CONCURRENCY = 4
export const POPUP_TIMEOUT_MS = 120_000

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function responseInstance(response, label) {
  const instance = response.headers.get('x-0db-instance')
  if (!instance) throw new Error(`${label}: missing x-0db-instance header`)
  return instance
}

async function fetchChecked(fetchImpl, url, label, expectedInstance) {
  let response
  try {
    response = await fetchImpl(url, { signal: AbortSignal.timeout(POPUP_TIMEOUT_MS) })
  } catch (error) {
    throw new Error(`${label}: request failed: ${error.message}`)
  }
  const instance = responseInstance(response, label)
  if (expectedInstance && instance !== expectedInstance) {
    throw new Error(`${label}: instance changed from ${expectedInstance} to ${instance}`)
  }
  const body = await response.text()
  if (!response.ok) {
    throw new Error(`${label}: HTTP ${response.status}: ${body.slice(0, 500)}`)
  }
  return { body, instance, response }
}

function parseJson(body, label) {
  try {
    return JSON.parse(body)
  } catch (error) {
    throw new Error(`${label}: invalid JSON: ${error.message}`)
  }
}

function validateCohort(value, label) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label}: expected object`)
  }
  const keys = [
    'schema_version', 'cohort_id', 'cache_ttl_ms', 'data_year',
    'runtime_sha256', 'prepared_sha256',
  ]
  if (Object.keys(value).sort().join() !== [...keys].sort().join()) {
    throw new Error(`${label}: cohort schema keys changed`)
  }
  if (value.schema_version !== 1) throw new Error(`${label}: unsupported schema_version`)
  for (const key of ['cohort_id', 'runtime_sha256', 'prepared_sha256']) {
    if (typeof value[key] !== 'string' || !SHA256.test(value[key])) {
      throw new Error(`${label}.${key}: expected lowercase SHA-256`)
    }
  }
  if (!Number.isInteger(value.cache_ttl_ms) || value.cache_ttl_ms < 0 || value.cache_ttl_ms > 60_000) {
    throw new Error(`${label}.cache_ttl_ms: expected integer within [0, 60000]`)
  }
  if (typeof value.data_year !== 'string' || value.data_year.length === 0) {
    throw new Error(`${label}.data_year: expected non-empty string`)
  }
  return value
}

function deriveUrls(input) {
  const popup = new URL(input)
  if (popup.pathname === '/' || popup.pathname === '') popup.pathname = '/api/noise-onfly-v2'
  popup.hash = ''
  popup.searchParams.delete('full')
  return {
    popup,
    health: new URL('/api/health', popup),
    cohort: new URL('/api/validation/cohort', popup),
  }
}

export async function inspectEndpoint(input, options = {}) {
  const fetchImpl = options.fetchImpl ?? fetch
  const urls = deriveUrls(input)
  const result = { url: urls.popup.toString() }
  for (const [name, url] of [['health', urls.health], ['cohort', urls.cohort]]) {
    try {
      const response = await fetchImpl(url, { signal: AbortSignal.timeout(POPUP_TIMEOUT_MS) })
      result[name] = {
        status: response.status,
        instance: response.headers.get('x-0db-instance'),
        body: (await response.text()).slice(0, 500),
      }
    } catch (error) {
      result[name] = { error: error.message }
    }
  }
  return result
}

function sameCohort(a, b) {
  return a.schema_version === b.schema_version
    && a.cohort_id === b.cohort_id
    && a.data_year === b.data_year
    && a.runtime_sha256 === b.runtime_sha256
    && a.prepared_sha256 === b.prepared_sha256
}

export async function consumeRateLimited(items, task, onResult, options = {}) {
  const intervalMs = options.intervalMs ?? REQUEST_INTERVAL_MS
  const concurrency = options.concurrency ?? REQUEST_CONCURRENCY
  const active = new Set()
  let nextStart = Date.now()
  let index = 0
  let firstError

  for await (const item of items) {
    while (active.size >= concurrency) await Promise.race(active)
    if (firstError) break
    const delay = nextStart - Date.now()
    if (delay > 0) await sleep(delay)
    const started = Date.now()
    nextStart = Math.max(nextStart + intervalMs, started + intervalMs)
    const itemIndex = index
    index += 1
    const running = Promise.resolve()
      .then(() => task(item, itemIndex))
      .then((result) => onResult(result, item, itemIndex))
      .catch((error) => { firstError ??= error })
      .finally(() => active.delete(running))
    active.add(running)
  }
  await Promise.all(active)
  if (firstError) throw firstError
}

export async function mapRateLimited(items, task, options = {}) {
  const results = new Array(items.length)
  await consumeRateLimited(
    items,
    task,
    (result, _item, index) => { results[index] = result },
    options,
  )
  return results
}

export async function openEndpointSession(input, options = {}) {
  const fetchImpl = options.fetchImpl ?? fetch
  const urls = deriveUrls(input)
  const healthStart = await fetchChecked(fetchImpl, urls.health, 'health/start', options.expectedInstance)
  const instance = healthStart.instance
  const cohortResponse = await fetchChecked(fetchImpl, urls.cohort, 'cohort/start', instance)
  const cohort = validateCohort(parseJson(cohortResponse.body, 'cohort/start'), 'cohort/start')
  const cohortReadAt = Date.now()

  return {
    instance,
    cohort,
    popupUrl: urls.popup.toString(),
    async query(point) {
      const url = new URL(urls.popup)
      url.searchParams.set('lat', String(point.lat))
      url.searchParams.set('lng', String(point.lng))
      const started = performance.now()
      const result = await fetchChecked(fetchImpl, url, `popup/${point.id}`, instance)
      return {
        body: result.body,
        payload: parseJson(result.body, `popup/${point.id}`),
        elapsed_ms: performance.now() - started,
        http_status: result.response.status,
        content_type: result.response.headers.get('content-type') ?? '',
      }
    },
    async finish() {
      const remaining = cohortReadAt + cohort.cache_ttl_ms + 25 - Date.now()
      let wait = remaining
      while (wait > 0) {
        const chunk = Math.min(wait, 30_000)
        await sleep(chunk)
        wait -= chunk
      }
      await fetchChecked(fetchImpl, urls.health, 'health/end', instance)
      const finalResponse = await fetchChecked(fetchImpl, urls.cohort, 'cohort/end', instance)
      const finalCohort = validateCohort(parseJson(finalResponse.body, 'cohort/end'), 'cohort/end')
      if (!sameCohort(cohort, finalCohort)) {
        throw new Error(`cohort/end: cohort changed from ${cohort.cohort_id} to ${finalCohort.cohort_id}`)
      }
      return finalCohort
    },
  }
}
