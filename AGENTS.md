# Conventions for coding agents

Make noise visible, make quiet possible. The owner judges results; coding agents write the
code. Every change serves a correct, fast answer for the visitor anywhere on Earth. Read
`ARCHITECTURE.md` before anything else.

## Simplicity

- The smallest design that meets the budgets. A cache, index, second copy or coarser level is
  added only after the benchmark shows it is needed. Measure before optimising.
- One kind of data = one file type = one builder = one reader, with the same name everywhere.
- No flags, variants or compatibility layers: an old path is deleted in the same change. Edit
  originals; never create `-v2` files.
- A defect is a design lesson: rewrite the touched feature as you would from scratch, instead
  of adding a special case beside the old path. Prefer net deletion.
- Of two otherwise equal designs take the simpler one; standards permit error and inputs are
  incomplete.

## Code

- Rust for data and physics, TypeScript for the web, shell or Python only in `fetch/` and
  `bench/`.
- Code is the documentation: long, precise, greppable names; a one-line doc atop every file;
  comments only for why, the provenance of a constant, or a subtle invariant.
- About 300 lines per file. One fact in one place, one test per bug class, zero warnings.
- Physics lives once in `physics/`; a model change updates its tests and says so in the commit.
- Before every commit run `./scripts/check-fast.sh` and read its whole output.

## Data

- Compute numbers from data, never estimate them. Every benchmark difference is explained.
- Never delete data without the owner's word; never overwrite what is served.

## Public repository

This repository is public. No hostnames, disk paths, providers or deployment details in any
file, comment or doc: they belong in the private operations repository.
