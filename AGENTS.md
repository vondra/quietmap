# AGENTS.md — conventions for coding agents

These conventions bind every contributor to the public Quiet Map product.
Ops/automation conventions live in the private repo and do not apply here.

## Quality gate

Before every commit, run `./scripts/check-fast.sh` (optional `node` or
`rust` selects one side) and read its complete raw output. It must pass
with zero compiler warnings, including Rust Clippy.

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
crates before running them. The server caches `libsource_reader.so` and
long-running scripts cache extractor binaries; `./start.sh` rebuilds and
restarts everything.

## Public-repo boundary

Never put internal hostnames, providers, infrastructure paths, or deployment
details in product files, including comments and docs.

## Data

`data/` is gitignored and may be irreplaceable. Inspect contents before any
`rm -rf`; compute numbers from data, never estimate them.
