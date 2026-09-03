#!/usr/bin/env python3
"""Per-layer code_ver for the world build's incremental-regen stamps.
code_ver[L] = a CONTENT set-hash over SHARED ∪ L's EXCLUSIVE files, where SHARED = the
heatmap COMPUTE closure (the 4 compute crates' production *.rs/*.cu/*.cuh/Cargo.toml + the global
build config .cargo/config.toml / rust-toolchain, engine/Cargo.toml workspace profile, and the
selected model-role spec) MINUS every
layer's exclusive files. Cargo.lock is
EXCLUDED — it is host/feature-dependent (a gpu build adds CUDA deps), so hashing it cv-gated every
gpu-line box off the cpu-only planner's cv (see closure_files). A content hash,
NOT a max-mtime: a backdated git checkout or an rsync -a that preserves an old mtime still flips it
(/gg: codex). SHARED-by-subtraction is the safety property: no production file can be silently missed,
so editing shared physics rebuilds ALL layers (safe over-invalidation) while editing a layer-exclusive
loader/compute/emission rebuilds ONLY that layer. When unsure whether a file is single-layer, leave it
OUT of the exclusives → it lands in SHARED → over-invalidates safely.

Usage:
  layer-codever.py <engine_dir> <layer>=<space-sep paths> ...   → prints CODE_VER[layer]=<epoch>
  layer-codever.py <engine_dir> --check <layer>=<paths> ...      → tripwire: exit 1 if an exclusive
      path is missing, lives outside the closure, or two layers claim the same file (the disjoint
      partition that makes stale tiles impossible). Run it as a gate before relying on the stamps.
"""
import os, sys, hashlib

# the heatmap COMPUTE crates — NOT aircraft-extract / osm-extract (data prep; their edits change DATA,
# bumping data_ver via arrow mtimes, not code). Validator-only bins are dropped (not production output).
CRATES = ("tile-painter", "noise-compute", "noise-gpu", "raster-reader")
EXCLUDE_BINS = {
    "compare_floats.rs", "compare_hm3.rs", "e2_airborne.rs",
    "tile_store_fsck.rs",
}
# Global build inputs OUTSIDE the crates that still change produced tiles (e.g. rustflags=target-cpu).
GLOBAL_BUILD = (
    ".cargo/config.toml", ".cargo/config", "rust-toolchain.toml", "rust-toolchain",
    "scripts/model-role-spec.json",
    # Workspace [profile.release] lives here; member profiles are ignored. Omit this
    # and a profile-only edit leaves every CODE_VER[layer] unchanged.
    "engine/Cargo.toml",
)
REQUIRED_GLOBAL_BUILD = ("scripts/model-role-spec.json",)

# The layer→exclusive-source partition (paths relative to engine/). The SINGLE source of truth for which
# files are a layer's own loader/compute/emission — everything else in the closure is SHARED (over-
# invalidates safely). A bare `layer-codever.py engine` (no <layer>=<paths> args) uses this map, so the
# hub gets identical code_vers by calling the authority instead of re-encoding the paths (parity by
# construction). Keep DISJOINT (--check enforces it): two layers claiming one file = stale tiles.
DEFAULT_EXCL = {
    # normalize/road.rs + the defaults cascade + its generated tables are road-only (grep-proven
    # 2026-07-03: the sole cross-crate consumer is source_loader_road.rs, already in this set) —
    # keeping them SHARED re-staled the whole world on every road-emission tweak.
    "road": "tile-painter/src/source_loader_road.rs noise-compute/src/compute/roads.rs noise-compute/src/emission/road.rs"
            " noise-compute/src/normalize/road.rs noise-compute/src/defaults.rs"
            " noise-compute/src/city_consts_generated.rs noise-compute/src/country_defaults_generated.rs"
            " noise-compute/src/region_defaults_generated.rs"
            " noise-compute/src/country_speed_defaults_generated.rs",
    "rail": "tile-painter/src/source_loader_rail.rs noise-compute/src/compute/railways.rs noise-compute/src/emission/railway.rs",
    "industrial": "noise-compute/src/emission/industrial.rs noise-compute/src/emission/wind.rs",
    # emission/leisure.rs is building-only (grep-proven 2026-07-16: its callers are
    # normalize/points.rs' LEISURE rows — which land in the building tile, layer-spec folds
    # leisure.arrow into building — and popup labels in source_names.rs, not painted output).
    "building": "tile-painter/src/source_loader_building.rs noise-compute/src/emission/settlement.rs"
                " noise-compute/src/emission/leisure.rs",
    "aircraft-ground": "tile-painter/src/source_loader_traffic.rs tile-painter/src/ground_ops.rs noise-compute/src/emission/airport_traffic.rs noise-compute/src/emission/gse.rs noise-compute/src/emission/aircraft/ground_ops.rs noise-compute/src/compute/aircraft_v6/airport_traffic",
    # `gpu_airborne.rs` was split into the gpu_airborne/ submodules in b7322f93, but the
    # partition kept only the entry file. Those submodules and the shared-library airborne.rs
    # therefore fell into SHARED: an aircraft-only stream optimisation changed road/cruise/ground
    # cv and deploy tried to replan every active goal (2026-07-19). Own the whole submodule
    # directory so future files cannot repeat the omission. Existing stamps survive this one-time
    # partition hash rotation through their exact per-layer OUTPUT_VER proof in world-stamps.py.
    "aircraft-airborne": "tile-painter/src/source_loader_airborne.rs tile-painter/src/airborne.rs"
                         " tile-painter/src/airborne_screening.rs"
                         " noise-compute/src/compute/aircraft_v6/airborne"
                         " noise-compute/src/emission/aircraft/horizon.rs"
                         " noise-compute/src/emission/aircraft/screening.rs"
                         " noise-compute/src/emission/aircraft/screening"
                         " noise-gpu/src/airborne.rs noise-gpu/src/gpu_airborne.rs"
                         " noise-gpu/src/airborne_building_horizon.rs"
                         " noise-gpu/src/airborne_terrain_horizon.rs"
                         " noise-gpu/src/gpu_airborne noise-gpu/kernels/airborne.cu"
                         " noise-gpu/kernels/airborne_screening.cuh"
                         " noise-gpu/kernels/airborne_building_horizon.cuh"
                         " noise-gpu/kernels/airborne_terrain_horizon.cuh",
    "aircraft-cruise": "tile-painter/src/cruise.rs noise-compute/src/compute/aircraft_v6/cruise.rs",
}


def closure_files(engine):
    out = set()
    for cr in CRATES:
        for dp, _, fns in os.walk(os.path.join(engine, cr)):
            if "/target/" in dp or dp.rstrip("/").endswith("/tests") or "/tests/" in dp:
                continue
            for fn in fns:
                if fn in EXCLUDE_BINS:
                    continue
                # Cargo.lock is DELIBERATELY excluded (2026-07-13): it is not source, and it is
                # host/feature-dependent — a gpu-feature build on noise-gpu resolves
                # extra CUDA deps into noise-gpu/Cargo.lock, so every gpu-line box hashed a DIFFERENT
                # cv than the (cpu-only) planner and the hub cv-gate refused ALL its claims — the fast
                # GPU boxes could never build. The recipe (source) is what changes output; a dep pin is
                # captured by the crates' Cargo.toml + the source that uses it. So hash *.rs/*.cu + Cargo.toml
                # only. (If a silent within-range dep bump ever needs to re-stale, bump a source file.)
                if fn.endswith((".rs", ".cu", ".cuh")) or fn == "Cargo.toml":
                    out.add(os.path.join(dp, fn))
    repo = os.path.dirname(os.path.abspath(engine))
    for g in GLOBAL_BUILD:
        p = os.path.join(repo, g)
        if os.path.exists(p):
            out.add(p)   # SHARED (not any layer's exclusive) → an edit rebuilds every layer
    return out


def expand(engine, spec):
    """A spec is a file or a dir (recurse over its .rs/.cu/.cuh); paths relative to engine/. A missing
    path is returned as-is so --check can flag it (a stale mapping after a rename)."""
    p = os.path.join(engine, spec)
    if os.path.isdir(p):
        return {os.path.join(dp, fn) for dp, _, fns in os.walk(p)
                for fn in fns if fn.endswith((".rs", ".cu", ".cuh"))}
    return {p}


_sig = {}


def file_sig(f):
    """Content hash of one file (computed once). Content, not mtime, so a backdated/preserved-mtime
    change still flips it — the data side already set-hashes, the code side must match that rigor."""
    if f not in _sig:
        try:
            with open(f, "rb") as fh:
                _sig[f] = hashlib.sha1(fh.read()).hexdigest()[:16]
        except OSError:
            _sig[f] = "0"
    return _sig[f]


def set_hash(files, repo):
    """Hash repository-relative paths so identical sources produce one code version.

    Absolute paths made byte-identical 2026-07-02 checkouts disagree, so the Hub rejected every
    worker claim. Output-version blessing keeps seals current across this one path-key correction.
    """
    rel = lambda f: os.path.relpath(f, repo)   # noqa: E731 — tiny local key fn
    return hashlib.sha1("\n".join(f"{rel(f)}\t{file_sig(f)}" for f in sorted(files, key=rel)).encode()).hexdigest()[:16]


def main():
    engine = sys.argv[1]
    check = "--check" in sys.argv
    specs = [a for a in sys.argv[2:] if a != "--check"]
    if not specs:   # no explicit <layer>=<paths> → the built-in partition (the hub + a bare call rely on this)
        specs = [f"{name}={paths}" for name, paths in DEFAULT_EXCL.items()]
    excl = {}
    for a in specs:
        name, _, paths = a.partition("=")
        fs = set()
        for s in paths.split():
            fs |= expand(engine, s)
        excl[name] = fs

    allf = closure_files(engine)
    if check:
        ok = True
        seen = {}
        repo = os.path.dirname(os.path.abspath(engine))
        global_paths = {os.path.join(repo, relative): relative for relative in GLOBAL_BUILD}
        for relative in REQUIRED_GLOBAL_BUILD:
            path = os.path.join(repo, relative)
            if not os.path.exists(path):
                print(f"MISSING global build input {relative}", file=sys.stderr); ok = False
        for L, fs in excl.items():
            for f in fs:
                if not os.path.exists(f):
                    print(f"MISSING exclusive {f} (layer {L})", file=sys.stderr); ok = False
                elif f not in allf:
                    print(f"OUT-OF-CLOSURE exclusive {f} (layer {L})", file=sys.stderr); ok = False
                if f in seen and seen[f] != L:
                    print(f"DUPLICATE {f}: layers {seen[f]} and {L}", file=sys.stderr); ok = False
                absolute = os.path.abspath(f)
                if absolute in global_paths:
                    print(f"GLOBAL input classified exclusive: {global_paths[absolute]} (layer {L})", file=sys.stderr); ok = False
                seen[f] = L
        sys.exit(0 if ok else 1)

    shared = (allf - set().union(*excl.values())) if excl else allf
    for L, fs in excl.items():
        # content set-hash over SHARED ∪ L's exclusives: a shared-physics edit flips every layer's hash;
        # a layer-exclusive edit flips only that layer's. (file_sig caches → each file is read once.)
        print(f"CODE_VER[{L}]={set_hash(shared | fs, os.path.dirname(os.path.abspath(engine)))}")


main()
