// cohort-client.mjs — the ONE client of /api/validation/cohort. Both QA runners (the
// /check-world skill's run.mjs and pipeline/validation/delta-table.ts) had grown verbatim
// copies of this fetch + shape validation, so a server-side contract bump (schema_version,
// TTL bounds) had to be patched twice or one gate silently diverged — the exact
// fix-one-miss-the-rest class AGENTS.md's one-source-of-truth rule targets (/simplify
// 2026-07-15). Shared-module shape mirrors snapshot-loader.mjs (.mjs + .d.mts) so the TS
// runner and the skill's plain-JS runner import the same file.

const SHA256_RE = /^[a-f0-9]{64}$/

/// Validate the cohort response shape; returns the value or throws with the caller's label.
export function validateModelCohort(value, label) {
  if (value?.schema_version !== 1
    || !SHA256_RE.test(value.cohort_id ?? '')
    || !Number.isInteger(value.cache_ttl_ms) || value.cache_ttl_ms < 0 || value.cache_ttl_ms > 60_000
    || !SHA256_RE.test(value.runtime_sha256 ?? '')
    || !SHA256_RE.test(value.prepared_sha256 ?? '')) {
    throw new Error(`${label}: malformed validation cohort response`)
  }
  return value
}

/// Fetch + validate the model/data cohort. `onResponse(response, label)` is the per-runner
/// instance-coherence hook (each runner pins the server instance it started against).
export async function fetchModelCohort({ server, timeoutMs, label, onResponse }) {
  const response = await fetch(`${server}/api/validation/cohort`, {
    signal: AbortSignal.timeout(timeoutMs),
  })
  if (onResponse) onResponse(response, label)
  if (!response.ok) throw new Error(`${label}: HTTP ${response.status}`)
  return validateModelCohort(await response.json(), label)
}
