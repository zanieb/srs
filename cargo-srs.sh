#!/usr/bin/env bash
set -euo pipefail

bin_dir="$(cd "$(dirname "$0")" && pwd)"
real_cargo="$bin_dir/cargo-srs-real"

if [[ ! -x "$real_cargo" ]]; then
    printf 'missing SRS Cargo binary at %s\n' "$real_cargo" >&2
    exit 2
fi

# Build scripts and proc macros execute on the build host. Keep those helpers
# on LLVM while SRS target artifacts follow rustc's Cranelift default.
exec "$real_cargo" \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm"]' \
    "$@"
