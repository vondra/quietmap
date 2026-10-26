# Conventions for coding agents

Make noise visible, make quiet possible. The owner judges results; coding agents write the
code. Every change serves a correct, fast answer for the visitor anywhere on Earth. Read the About
pages first (`frontend/src/about/`): `data-preparation.md`, `noise-at-a-point.md` and
`the-noise-map.md` tell how the map is computed, each ending in its code.

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
- How the map works is told twice only: on the About pages, for every visitor down to the code,
  and in the code itself. No other document repeats it; a change that makes a page wrong fixes
  the page in the same commit.
- Physics lives once in `physics/`; a model change updates its tests and says so in the commit.
- Before every commit run `./scripts/check-fast.sh` and read its whole output.

## UI style

One look for the whole web, quiet around the map and its noise colours:

- Colours only from the tokens in `frontend/src/index.css`: `:root`, and its dark scheme, which
  follows the system's; a literal colour only for a mark that means something (a map layer, a
  diagram's key, the locate blue), with its `dark:` variant where it sits on a card. The base map
  stays light in the dark scheme, as the noise colours darken with loudness.
- Type: Geist. 14 px (`text-sm`) for controls, the search's rows and buttons; 12 px (`text-xs`) for
  what is secondary and the popup's source rows (four columns in 360 px); on a touch screen names and
  text fields 16 px (iOS zooms into smaller fields); 11 and 10 px only in what a popup's row opens;
  larger only for the popup's level. Weights 400, 500 for names and controls, 600 for numbers and
  captions. Sentence case, no all-caps labels.
- Buttons, chips and floating surfaces from `frontend/src/components/ui/` (`button.ts`,
  `check-chip.tsx`); controls 32 px high in a card, tabs 36, a phone's floating controls 44.
- A row: a fixed icon column, the text left-aligned, numbers tabular at the right, its actions at the
  end as wide in every row, so that nothing moves on hover or between rows. Icons: lucide, 16 px, 20 px
  for a touch screen's actions.
- An action where and when the visitor needs it: the account at the top right, the microphone and the
  camera on a touch screen at the open place, nothing explained in more than two sentences.

## Data

- Compute numbers from data, never estimate them. Every benchmark difference is explained.
- Never delete data without the owner's word; never overwrite what is served.

## Public repository

This repository is public. No hostnames, disk paths, providers or deployment details in any
file, comment or doc, and no timings on our own machines: they belong in the private operations
repository. Everything else that tells how the map computes and why we believe it belongs here.
