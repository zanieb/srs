#!/usr/bin/env bash
set -euo pipefail

bin_dir="$(cd "$(dirname "$0")" && pwd)"
real_cargo="$bin_dir/cargo-srs-real"

if [[ ! -x "$real_cargo" ]]; then
    printf 'missing SRS Cargo binary at %s\n' "$real_cargo" >&2
    exit 2
fi

# Use content checksums for source freshness so unchanged touches stay fresh
# and content edits are detected even when their mtimes are preserved.
if [[ "${SRS_CARGO_CHECKSUM_FRESHNESS:-1}" != "0" ]]; then
    checksum_freshness_args=(-Z checksum-freshness)
    export CARGO_UNSTABLE_CHECKSUM_FRESHNESS=true
else
    checksum_freshness_args=()
    export CARGO_UNSTABLE_CHECKSUM_FRESHNESS=false
fi

# Target artifacts link through sld by default. Preserve an explicit override
# while making incremental link reuse the normal SRS developer-loop behavior.
export SLD_INCREMENTAL="${SLD_INCREMENTAL:-1}"

append_target_rustflag() {
    local rustflag="$1"
    export SRS_ENCODED_TARGET_RUSTFLAGS="${SRS_ENCODED_TARGET_RUSTFLAGS:+${SRS_ENCODED_TARGET_RUSTFLAGS}$'\x1f'}${rustflag}"
}

# Reuse verified ordinary-library outputs across SRS worktrees. Hardlink
# materialization avoids duplicating restored artifact allocation and Cargo
# detaches links before rebuilding. Set SRS_CARGO_ARTIFACT_CACHE=0 to disable
# this while investigating a build.
if [[ "${SRS_CARGO_ARTIFACT_CACHE:-1}" != "0" ]]; then
    # Cargo subcommands such as Clippy invoke the Cargo path from `CARGO`
    # directly rather than re-entering this wrapper. Propagate the feature as
    # config so those nested Cargo processes keep the same cache policy.
    export CARGO_UNSTABLE_ARTIFACT_CACHE=true
    export CARGO_BUILD_ARTIFACT_CACHE_DIR="${CARGO_BUILD_ARTIFACT_CACHE_DIR:-${SRS_CARGO_ARTIFACT_CACHE_DIR:-${CARGO_HOME:-$HOME/.cargo}/srs-artifact-cache-v2}}"
    if [[ -n "${SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION:-}" ]]; then
        export CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION="${CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION:-$SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION}"
    fi
    if [[ -n "${SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE:-}" ]]; then
        export CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE="${CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE:-$SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE}"
    fi
    artifact_cache_args=(-Z artifact-cache)
else
    export CARGO_UNSTABLE_ARTIFACT_CACHE=false
    artifact_cache_args=(-Z artifact-cache --config build.artifact-cache=false)
fi

# Build scripts and proc macros execute on the build host. Keep those helpers
# on LLVM. On Apple silicon, start the integrated incremental-link lane with
# LLVM target artifacts too, keep host-loadable artifacts off the experimental
# linker, and request root-only signed incremental links for normal target
# executables. Cranelift remains available as an explicit follow-up lane.
sld_native_incremental_args=()
host_rustflags='["-Zcodegen-backend=llvm"]'
if [[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" ]]; then
    host_rustflags='["-Zcodegen-backend=llvm","-C","linker=/usr/bin/clang"]'
    export SRS_TARGET_CODEGEN_BACKEND="${SRS_TARGET_CODEGEN_BACKEND:-llvm}"
    append_target_rustflag "-Zcodegen-backend=${SRS_TARGET_CODEGEN_BACKEND}"
    export SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS="${SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS:-1}"
    case "$SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS" in
        0) append_target_rustflag "-Zpreserve-duplicate-constants=no" ;;
        1) append_target_rustflag "-Zpreserve-duplicate-constants=yes" ;;
        *)
            printf 'invalid SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS: %s\n' \
                "$SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS" >&2
            exit 2
            ;;
    esac
    if [[ "$SLD_INCREMENTAL" != "0" ]]; then
        export SLD_INCREMENTAL_PADDING_PERCENT="${SLD_INCREMENTAL_PADDING_PERCENT:-1}"
        export SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS="${SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS:-1}"
        export SLD_RUSTC_WORK_PRODUCT_PROVENANCE="${SLD_RUSTC_WORK_PRODUCT_PROVENANCE:-1}"
        sld_native_incremental_args=(-Z sld-native-incremental)
    fi
fi

if [[ "${#sld_native_incremental_args[@]}" -gt 0 ]]; then
    exec "$real_cargo" \
        ${checksum_freshness_args[@]+"${checksum_freshness_args[@]}"} \
        "${artifact_cache_args[@]}" \
        "${sld_native_incremental_args[@]}" \
        -Z host-config \
        -Z target-applies-to-host \
        --config 'target-applies-to-host=false' \
        --config "host.rustflags=$host_rustflags" \
        "$@"
fi

exec "$real_cargo" \
    ${checksum_freshness_args[@]+"${checksum_freshness_args[@]}"} \
    "${artifact_cache_args[@]}" \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config "host.rustflags=$host_rustflags" \
    "$@"
