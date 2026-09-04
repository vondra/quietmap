# AGENTS.md — conventions for coding agents

These conventions bind every contributor to the public Quiet Map product.
Ops/automation conventions live in the private repo and do not apply here.

## Quality gate

Before every commit, run `cargo test` and `cargo clippy -- -D warnings`
from `engine/` and read the complete raw output. Both must pass with zero
warnings. (When `scripts/check-fast.sh` lands, it becomes the gate.)

## Simplicity budget

Every change pays rent. Before adding code, read the complete touched feature
and its direct callers. In that same area remove or consolidate obsolete
branches, dormant flags/env knobs, fallbacks, compatibility shims, duplicate
truths/tests/docs, completed migration bridges, and dead paths.
Ship the smallest complete design; never speculative scaffolding.

- This is active development: activate the selected behavior and delete the old
  path in the same logical wave. Preserve compatibility only for irreplaceable
  data as one explicit migration that then disappears.
- Prefer fewer concepts and net deletion. Handoffs report added/deleted/net LOC
  and the concepts removed. Production-code growth must name the visitor, data,
  or operability capability it buys and why a smaller design cannot provide it.
- Do not game LOC: descriptive names, one test per bug class, and checks that
  protect visitors or irreplaceable data stay.
- A verification gate may block commit or release only to protect a visitor or
  irreplaceable data; it never blocks a dev preview, measurement, or packaging an experiment.
- One correctness fact lives in one place. A hash verifies bytes only against
  an independently anchored expected identity; a file never proves itself.
- Delete completed plans and stale research after moving any lasting invariant
  into code or the current specification; git history is the archive.

## Code style

- **Code is the documentation.** Use long, precise, greppable names
  (`write_rail_trains_with_priority_gate`, not `wrt`).
- Put a one-line module doc atop every file (`//!`, `///`, JSDoc); each
  crate root (`lib.rs`) maps its submodules.
- Comments carry only what code cannot: why this approach wins, cited
  provenance of constants, or subtle invariants.
- File size: aim for ~300 lines.
- Correctness-critical acoustics live once in `engine/noise-compute`;
  per-case files remain thin data plus a call. A model change must update
  `engine/noise-compute/SPEC.md`.
- Edit originals — never create `-v2` / parallel variants.
- Standards permit error and inputs are incomplete. When two variants are
  otherwise equal, choose the simpler one.

## Native binaries after pulls

After every `git pull` or source sync touching `engine/`, rebuild native
crates before running them (`cargo build --release --manifest-path
engine/Cargo.toml`; binaries land in `engine/target/release/`).

## Public-repo boundary

Never put internal hostnames, providers, infrastructure paths, or deployment
details in product files, including comments and docs.

## Data

`data/` is gitignored and may be irreplaceable. Inspect contents before any
`rm -rf`; compute numbers from data, never estimate them.

## Disks (this box)

- `readmostly1` = finished sources, read-only inputs (`r260904/source`).
  `readmostly2` = finished web data (`r260904/prepared`), written once as a
  bulk promotion, then only read.
- All work happens on mixeduse: heavy temp/spill on `mixeduse1` (shares its
  disk with `/tmp`), intermediates on `mixeduse2/r260904/work`. Split reads
  and writes across disks for throughput: sources on one, temp on another,
  output on a third where the job allows.
- Never write intermediates to a readmostly disk. Keep the finished tree on
  mixeduse2 as the backup copy; readmostly2 is the served copy.

## Grid and releases

- The compute unit is the Web-Mercator z9 tile (`engine/grid` owns the math).
  No H3 anywhere: no `h3` dependency, no hex vocabulary, no fallback.
- Prepared vectors store global int32 z30-pixel coordinates (3.7 cm quantum),
  heights int16 metres. Identity across sources is the builder's job in code,
  never an epsilon on read. Use i64 for differences of grid coordinates.
- One release name (`r260904[b..]`) spans source/prepared/tiles; a box uses
  only what it tested itself, then switches its `current`.
