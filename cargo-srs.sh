#!/usr/bin/env bash
set -euo pipefail

bin_dir="$(cd "$(dirname "$0")" && pwd)"
real_cargo="$bin_dir/cargo-srs-real"

if [[ ! -x "$real_cargo" ]]; then
    printf 'missing SRS Cargo binary at %s\n' "$real_cargo" >&2
    exit 2
fi

# Target artifacts link through sld by default. Preserve an explicit override
# while making incremental link reuse the normal SRS developer-loop behavior.
export SLD_INCREMENTAL="${SLD_INCREMENTAL:-1}"

# Reuse verified ordinary-library outputs across SRS worktrees. Hardlink
# materialization avoids duplicating restored artifact allocation and Cargo
# detaches links before rebuilding. Set SRS_CARGO_ARTIFACT_CACHE=0 to disable
# this while investigating a build.
artifact_cache_args=()
if [[ "${SRS_CARGO_ARTIFACT_CACHE:-1}" != "0" ]]; then
    export CARGO_BUILD_ARTIFACT_CACHE_DIR="${CARGO_BUILD_ARTIFACT_CACHE_DIR:-${SRS_CARGO_ARTIFACT_CACHE_DIR:-${CARGO_HOME:-$HOME/.cargo}/srs-artifact-cache-v2}}"
    export CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION="${CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION:-${SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION:-hardlink}}"
    export CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE="${CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE:-${SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE:-10GB}}"
    artifact_cache_args=(-Z artifact-cache)
fi

# Build scripts and proc macros execute on the build host. Keep those helpers
# on LLVM while SRS target artifacts follow rustc's Cranelift default.
exec "$real_cargo" \
    "${artifact_cache_args[@]}" \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm"]' \
    "$@"
