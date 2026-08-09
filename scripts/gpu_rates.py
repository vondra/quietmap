#!/usr/bin/env python3
# gpu_rates.py — shared per-card GPU-strength table for the Vast offer-scoring workload model.
# Data lives in gpu-rates.json (PassMark G3D Mark, built by gpu_rates_build.py from the owner-supplied
# gpu-benchmarks.csv); this module loads it and resolves a GPU name to a rate. Imported by world/vast-offers.py.
# Real per-model box-timings.json measurements (see box_timing.model_ratios()) are the workload-specific
# complement — NOT folded in here; vast-offers blends the two by confidence. See gpu-rates.json "_doc".
import json
import os
import re

from cpu_rates import find_entry   # table-agnostic (any {"rates":[{"match":...}]} shape) -- one substring
                                    # matcher shared by both tables, not duplicated (AGENTS.md: one source
                                    # of truth; duplication across siblings is where drift bugs hide)

RATES_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "gpu-rates.json")


def load_table():
    """The parsed gpu-rates.json (dict with a 'rates' list)."""
    with open(RATES_PATH) as f:
        return json.load(f)


def _entry(name, table):
    """find_entry(), plus a WORD-BOUNDARY check on top: GPU model numbers collide under plain substring
    matching far more than CPU ones do -- e.g. 'A10' (a real, different, weaker card) is a character
    substring of 'A100 PCIE' with NO word boundary between them (found by testing this fix against live
    vast offers, not anticipated in the original design: 118 such boundary-violating pairs exist in this
    ~726-entry table alone, e.g. 'A40' in 'RTX A4000', 'GTX 680' in 'GTX 680M'). cpu_rates.find_entry()
    itself is NOT changed (shared with the CPU table, out of scope, and CPU model numbers collide far
    less) -- this boundary check is a GPU-only filter applied to its result.

    Lookarounds, NOT \\b: 11 real entries end in ')' ("RTX 2080 (Mobile)" etc, /gg Gemini) -- \\b requires
    a \\w<->\\W TRANSITION, so a match ending right at a ')' followed by end-of-string or another \\W
    character is \\W-to-\\W and \\b silently fails, wrongly rejecting a valid exact match. (?<!\\w)/(?!\\w)
    only check the RELEVANT side (not preceded/followed by a word char), which is what's actually needed
    and handles a trailing ')' correctly while still rejecting the 'A10' in 'A100 PCIE' case.

    Known accepted scope limit (/gg Codex): this checks WORD-CHARACTER boundaries, so a hyphen still
    counts as a valid boundary either way -- 'RTX 4070' would match inside a hypothetical 'RTX 4070-Ti' if
    both existed as distinct table rows. Checked against the actual data (both gpu-benchmarks.csv and
    vast's naming): no consumer card name uses a hyphen as a plain model-family/suffix separator today
    (hyphens only appear in datacenter form-factor suffixes, already handled by CANONICAL_PREFIX in
    gpu_rates_build.py, or in unrelated vGPU/monitor rows) -- a general separator-normalizing tokenizer
    would add real complexity for a currently-theoretical case, so this is accepted, not fixed, per
    AGENTS.md's Occam's razor (re-check if a future CSV refresh introduces hyphenated consumer names)."""
    e = find_entry(name, table)
    if e and not re.search(r"(?<!\w)" + re.escape(e["match"].lower()) + r"(?!\w)", (name or "").lower()):
        return None
    return e


def gpu_rate(name, table=None):
    """(g3dmark, known) for a GPU name. Unknown -> (0, False) -- deliberately NOT a conservative nonzero
    default like cpu_rate()'s: an unrecognized name on this fleet's curated gpu_name allowlist is
    disproportionately either a brand-new card missing from the CSV snapshot or a pre-Volta card the CUDA
    kernel can't run on at all -- zero is the safe failure mode for both, and matches GPU_SM's prior
    behaviour (no regression). A card with real box_timing.model_ratios() data still scores correctly
    even at (0, False) here -- vast-offers' blend formula falls through to the empirical ratio alone."""
    table = table or load_table()
    e = _entry(name, table)
    return (e["g3dmark"], True) if e else (0, False)


def model_key(name, table=None):
    """Canonical 'match' key for a GPU name, or None if unrecognized. Shared by vast-offers' physics lookup
    AND box_timing.model_ratios()'s bucketing, so an owned box's fuller nvidia-smi name ('NVIDIA GeForce
    RTX 4060 Ti') and vast's terser one ('RTX 4060 Ti') land in the same calibration bucket."""
    table = table or load_table()
    e = _entry(name, table)
    return e["match"] if e else None


def canonical_key(name, table=None):
    """model_key(), falling back to the raw normalized name when the GPU has NO PassMark G3D Mark
    coverage at all (e.g. 'A100 PCIE' -- missing from this CSV snapshot; see gpu_rates_build.py). Without
    this fallback, a card with real box_timing.model_ratios() samples but zero physics-table coverage
    would have NO canonical key to bucket its own real measurements under, orphaning them (found while
    verifying A100 PCIE against live data, after the word-boundary fix correctly stopped it from
    false-matching the unrelated 'A10' card). Exact-string equality (case/whitespace-insensitive) is
    enough here: it only needs to reunite a live offer's gpu_name with box-timings.json's stored gpu
    field for that SAME card family, not fuzzy-match anything new."""
    table = table or load_table()
    return model_key(name, table) or " ".join((name or "").split()).lower() or None


# ── selftest: `python3 scripts/gpu_rates.py --selftest` (mirrors queue.mjs --selftest's convention) ──
if __name__ == "__main__":
    import sys
    if "--selftest" in sys.argv[1:]:
        ok_n = [0]

        def ok(name, cond):
            print(f"{'PASS' if cond else 'FAIL'} {name}")
            ok_n[0] += 0 if cond else 1

        t = load_table()
        ok("table parses", isinstance(t.get("rates"), list))
        ok("entry count in ~700-750 range", 700 <= len(t["rates"]) <= 750)
        matches = [e["match"] for e in t["rates"]]
        ok("no duplicate match keys", len(matches) == len(set(matches)))

        g4060, known4060 = gpu_rate("RTX 4060 Ti", t)
        ok("gpu_rate(RTX 4060 Ti) known", known4060 and g4060 > 0)
        g2080, known2080 = gpu_rate("RTX 2080 Ti", t)
        ok("gpu_rate(RTX 2080 Ti) known", known2080 and g2080 > 0)
        ok("nonsense card unknown", gpu_rate("nonsense-card-xyz-9000", t) == (0, False))

        # The naming-direction regression guard (the single most important assertion here — see
        # gpu_rates_build.py's header comment: this fails SILENTLY, no exception, if it regresses).
        ok("fuller nvidia-smi name resolves to the same entry as vast's terser one",
           gpu_rate("NVIDIA GeForce RTX 4060 Ti", t) == gpu_rate("RTX 4060 Ti", t))

        # Suffix traps found in /gg review (Gemini): memory-size and datacenter form-factor suffixes
        # must collapse to their bare canonical entry; laptop/mobile variants must NOT collapse.
        ok("16GB-suffixed 4060 Ti collapses to the bare canonical entry",
           gpu_rate("GeForce RTX 4060 Ti 16GB", t) == gpu_rate("RTX 4060 Ti", t))
        ok("PCIE-suffixed V100 collapses to the bare canonical entry",
           gpu_rate("Tesla V100-PCIE-16GB", t) == gpu_rate("Tesla V100", t))
        ok("SXM-suffixed V100 collapses to the bare canonical entry",
           gpu_rate("Tesla V100-SXM2-16GB", t) == gpu_rate("Tesla V100", t))
        laptop_key = model_key("GeForce RTX 5090 Laptop GPU", t)
        ok("laptop variant does NOT collapse into its desktop counterpart",
           laptop_key is not None and laptop_key != model_key("RTX 5090", t))

        # A100 PCIE must NOT accidentally resolve via a false substring match onto the CSV's lone
        # A100-SXM4 row (the plan deliberately leaves A100 PCIE's physics leg unknown — see vast-offers.py's
        # empirical-only fallback branch, which is what actually carries this card).
        ok("A100 PCIE does not false-match the A100 SXM4 CSV row",
           model_key("A100 PCIE", t) != model_key("A100-SXM4-40GB", t))

        # Word-boundary regression guard — found by testing this fix against LIVE vast offers (not
        # anticipated by the original design): 'A10' (a real, different, weaker card) is a plain
        # character-substring of 'A100 PCIE' with no boundary between them. gpu_rate()/model_key() must
        # reject this, not silently score A100 PCIE off the wrong, weaker card.
        ok("A100 PCIE does not false-match the unrelated 'A10' card",
           gpu_rate("A100 PCIE", t) == (0, False))
        ok("a real standalone A10 offer still resolves correctly",
           gpu_rate("NVIDIA A10", t)[1] is True)

        # canonical_key()'s raw-name fallback: A100 PCIE has zero G3DMark coverage (model_key -> None) but
        # must still get a STABLE, non-None bucket key so box_timing.model_ratios() can accumulate its
        # real box-timings.json samples instead of silently discarding them (found while verifying against
        # live data — a card with real measurements but no physics-table row must not lose that data).
        ok("canonical_key falls back to a stable key when G3DMark doesn't recognize the card",
           model_key("A100 PCIE", t) is None and canonical_key("A100 PCIE", t) is not None)
        ok("canonical_key's fallback is case/whitespace-insensitive (reunites live offer with stored box)",
           canonical_key("A100 PCIE", t) == canonical_key("  a100   pcie ", t))

        # \b-vs-lookaround regression guard (/gg Gemini): a match key ending in ')' (11 real "(Mobile)"
        # entries in this table) must still resolve on an EXACT name match -- \b silently fails here
        # because ')' is a non-word char, so the transition at end-of-string is \W-to-\W, not a boundary.
        ok("a match key ending in ')' still resolves on an exact name match",
           gpu_rate("GeForce RTX 2080 (Mobile)", t)[1] is True)

        print("SELFTEST FAILED" if ok_n[0] else "SELFTEST OK")
        sys.exit(1 if ok_n[0] else 0)
