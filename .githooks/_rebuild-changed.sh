#!/usr/bin/env bash
# Internal helper called by post-merge and post-checkout hooks.
# Rebuilds ONLY the part of the system the pulled range actually touched:
#   engine/**.(rs|toml) or Cargo.lock  → cargo-rebuild the engine crates
#     (+ Fastify restart when a dlopen-linked crate changed)
#   frontend/** or server/**           → ./start.sh (frontend + compiled
#     server release + atomic restart; start.sh owns restart policy)
# Docs / pipeline / scripts-only pulls are a no-op — docs/about is read from
# the working tree at request time and needs no rebuild at all.
#
# Args: $1 = prev ref, $2 = new ref. Defaults to ORIG_HEAD..HEAD.

set -e

PREV="${1:-ORIG_HEAD}"
NEW="${2:-HEAD}"

# `pwd -P` resolves symlinks so REPO_ROOT lives in the same coordinate
# system as `readlink /proc/PID/cwd` (always canonical). Without this,
# a worktree reached via a symlinked path silently misses the restart.
REPO_ROOT="$(cd "$(git rev-parse --show-toplevel)" && pwd -P)"
cd "$REPO_ROOT"

# Print the PID, port and actual bind address of this checkout's listening
# Fastify. Capturing them together avoids racing independent scans and keeps a
# loopback-only development server loopback-only after an automatic restart.
find_our_fastify_listener() {
    local pid cwd endpoint port host
    for pid in $(pgrep -u "$(id -u)" -f 'src/server\.ts|dist/server\.js|scripts/start\.mjs' 2>/dev/null); do
        cwd=$(readlink "/proc/$pid/cwd" 2>/dev/null) || continue
        [ "$cwd" = "$REPO_ROOT/server" ] || continue
        # `pid=N` is followed by `,fd=…` in current ss but newer
        # builds may emit `pid=N)` (no fd field). Accept either.
        endpoint=$(ss -tlnHp 2>/dev/null \
              | awk -v p="pid=$pid[,)]" '$0~p {print $4; exit}')
        port=${endpoint##*:}
        host=${endpoint%:*}
        if [[ "$host" == \[*\] ]]; then
            host=${host#\[}
            host=${host%\]}
        fi
        # ss renders an IPv6 wildcard as `*`; Node expects `::` for the same
        # bind. Silent-skip any other uncertain address rather than widening
        # it to start.sh's 0.0.0.0 default.
        [ "$host" = "*" ] && host="::"
        if [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -gt 0 ] && [ "$port" -lt 65536 ]; then
            [ -n "$host" ] || continue
            printf '%s %s %s\n' "$pid" "$port" "$host"
            return 0
        fi
    done
    return 1
}

ALL_CHANGED=$(git diff --name-only "$PREV" "$NEW" 2>/dev/null || true)

# `Cargo.lock` is in the filter so a lock-only dependency bump still
# triggers an engine rebuild.
ENGINE_CHANGED=$(grep -E '^engine/.*(\.(rs|toml)|Cargo\.lock)$' <<<"$ALL_CHANGED" || true)

# frontend/dist* and server/dist are gitignored and never appear in the diff;
# any tracked frontend/ or server/ file (sources, public assets, package
# manifests, build config) invalidates the built-and-bundled web artifact,
# which only ./start.sh knows how to rebuild and atomically activate.
WEB_CHANGED=$(grep -E '^(frontend|server)/' <<<"$ALL_CHANGED" || true)

if [ -z "$ENGINE_CHANGED" ] && [ -z "$WEB_CHANGED" ]; then
    exit 0
fi

# Defer rebuild while a long engine job is running. Rebuilding mid-job
# (a) starves it of CPU — a 20-min airborne tile ran ~15× slower when a
# pull-triggered `cargo build` competed (2026-05-24), and (b) is pointless
# anyway: the running process keeps its in-memory binary, so it never picks
# up the fresh build. The operator rebuilds once the job finishes. This
# guard covers the web path too — start.sh cargo-builds source-reader, so
# it would compete with the running job just the same.
# `pgrep -o` returns one PID (the message below uses it) and the `if`
# keys off pgrep's own exit (0 = a job matched) — NOT `… | head`, whose
# exit is always 0 (head's), which made this guard fire unconditionally.
# Pattern covers every heavy engine job that build-heatmap.sh / the
# extract scripts launch (hyphenated bin names, so a `cargo` build of the
# same crate — `--crate-name build_heatmap_…`, underscores — doesn't match).
if RUNNING_PID=$(pgrep -of 'build-heatmap-aircraft|build-heatmap-surface|build-heatmap-combine|build-pyramid|aircraft-extract|osm-extract' 2>/dev/null); then
    echo "  → git hook: engine job running (pid $RUNNING_PID) — SKIPPING rebuild." >&2
    echo "    Rebuild after it finishes:  cargo build --release --manifest-path engine/Cargo.toml" >&2
    exit 0
fi

RESTART_REASON=""
[ -n "$WEB_CHANGED" ] && RESTART_REASON="web sources changed"

if [ -n "$ENGINE_CHANGED" ]; then
    echo "  → git hook: engine sources changed — rebuilding..." >&2

    # Build every workspace member. Glob over `engine/*` so a new crate
    # still rebuilds once it has a Cargo.toml; the workspace lockfile lives
    # at engine/Cargo.lock. Per-crate `cd` rebuilds crates outside the workspace.
    FAIL=0
    for crate in engine/*; do
        [ -f "$crate/Cargo.toml" ] || continue
        # noise-gpu's gpu-surface/e2-full bins live behind --features gpu (which needs
        # nvcc); add it only on a CUDA host so a CPU-only box still rebuilds the lib
        # without erroring, and a GPU box rebuilds the bins after a pull as before.
        feats=()
        if [ "$crate" = engine/noise-gpu ] && command -v nvcc >/dev/null 2>&1; then
            feats=(--features gpu)
        fi
        ( cd "$crate" && cargo build --release --locked --quiet "${feats[@]}" ) || {
            echo "    ✗ $crate build failed" >&2
            FAIL=1
        }
    done

    if [ "$FAIL" != "0" ]; then
        echo "  ⚠ some crates failed — see above" >&2
        exit 1
    fi

    # Per memory feedback_napi_worker_recycle.md: drop dlopen worker copies
    # so Fastify's next worker spawn picks up the fresh libsource_reader.so
    # (Fastify caches the .so via its first dlopen, so we also bounce it
    # below). Only AFTER the FAIL gate — a partial build must not strand
    # workers against a .so that never finished rebuilding.
    # Keep the process-stable shared path in place while the managed server stays
    # online. The parent refreshes it atomically before every Worker spawn. Only
    # obsolete numeric per-thread/per-slot copies consume extra static TLS paths.
    rm -f engine/target/release/libsource_reader.worker-[0-9]*.node \
          engine/target/release/libsource_reader.worker-tid-[0-9]*.node \
          engine/target/release/libsource_reader.worker-slot-[0-9]*.node

    # Fastify dlopens source-reader → noise-compute + raster-reader
    # (statically linked), so a change to any of the three invalidates the
    # running .so. Pulls touching only tile-painter / aircraft-extract /
    # osm-extract don't feed the popup process and skip the bounce.
    DLOPEN_RE='engine/(source-reader|noise-compute|raster-reader)/'
    if [ -z "$RESTART_REASON" ] && [[ "$ENGINE_CHANGED" =~ $DLOPEN_RE ]]; then
        RESTART_REASON="linked crate rebuilt"
    fi
    echo "  ✓ engine binaries rebuilt" >&2
fi

# Auto-restart via start.sh if the served artifact went stale (web sources
# changed, and/or a dlopen-linked crate rebuilt) and our worktree's server is
# currently up. Port is auto-detected from the listening Node process in this
# checkout's server directory. start.sh is the single owner of
# managed-vs-detached restart policy, exact process checks, locking, builds
# (frontend + compiled server + source-reader) and health verification.
if [ -n "$RESTART_REASON" ] && read -r OUR_PID OUR_PORT OUR_HOST < <(find_our_fastify_listener); then
    echo "  → scheduling Fastify restart on $OUR_HOST:$OUR_PORT (pid $OUR_PID; $RESTART_REASON)" >&2
    mkdir -p "$REPO_ROOT/logs"
    nohup env HOST="$OUR_HOST" PORT="$OUR_PORT" bash -c "cd '$REPO_ROOT' && ./start.sh" \
        > "$REPO_ROOT/logs/hook-restart.log" 2>&1 &
    disown
fi
