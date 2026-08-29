# benchmarks/

Fixtures and per-server baselines consumed by the `/check-heatmap`,
`/check-popup` and `/check-world` skills.

Candidate-box CPU oracle tiles are deliberately not stored here or in git. They are content-addressed
under `/0db/data/runtime/benchmarks/box-qualification/v1/` by
`scripts/qualification-reference.mjs`; the identity covers exact oracle code, fixed-cell data manifest,
year, workload, compiler, native CPU target, and thread count. `scripts/benchmark-box.sh` syncs that
immutable reference to each candidate and saves only the resulting four-lane timing + qualification
identity in runtime `box-timings.json`.

`box-qualification-profile.json` is the machine-readable schema/workload identity for that profile.
Version 2 clocks the four lanes sequentially; only exact current-schema records may drive measured offer
scoring or cross-model ratios.

## Files

- **`popup-points.json`** — 115 curated points across Dobříš R4
  (`841e309ffffffff`) and LKPR/Ruzyne R4 (`841e355ffffffff`),
  scenario-labeled. Stable fixture, rarely edited.
- **`world-points.json`** — external-validation points anchored to
  measured/published reality (traffic counts, SHM/Defra noise maps, airport
  monitors), each with an audit-authored `regression_band`. Consumed by
  `/check-world` (breakdown in its SKILL.md); per-host results land in
  `world-baseline.<hostname>.json`.
- **`heatmap-generation-baseline.<hostname>.json`** — latest heatmap
  generation timing + telemetry summary for a given server. Overwritten by
  `check-heatmap/run.mjs --write-baseline`; history lives in git.
- **`heatmap-generation-baseline.<hostname>.<timestamp>/`** — optional raw
  run directory with logs and metadata from a full heatmap measurement. Useful
  for parser/debug work; the compact JSON baseline is what comparisons read.
- **`popup-baseline.<hostname>.json`** — latest 115-point popup results
  for a given server. Overwritten by `check-popup/run.mjs --write-baseline`.

## Naming convention

`<kind>-baseline.<hostname>.json` — one latest baseline per server per skill.
Cross-server timing comparisons are skipped automatically (different CPUs).

## Seeding a new server

First run on a new host:

```bash
node .claude/skills/check-heatmap/run.mjs --write-baseline
node .claude/skills/check-popup/run.mjs --write-baseline
```

Commit the resulting JSON files.

## See also

- `docs/validation/berm-d4-dobris.md` — 9 of the popup-points come from this
  physical berm scenario (scenario labels `d4_source` / `behind_berm`).
- `.claude/skills/check-heatmap/SKILL.md`
- `.claude/skills/check-popup/SKILL.md`
- `.claude/skills/check-world/SKILL.md`
