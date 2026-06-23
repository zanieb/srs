#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
rust_dir="${SRS_RUST_DIR:-$root/rust}"
sld_target_dir="${SRS_SLD_TARGET_DIR:-$root/target/sld}"
sld_profile="${SRS_SLD_PROFILE:-opt}"
sld_bin="$sld_target_dir/$sld_profile/sld"

if [[ ! -x "$rust_dir/x" ]]; then
    printf 'missing Rust checkout at %s\n' "$rust_dir" >&2
    exit 2
fi

# Build the pinned linker before the toolchain so the installer can attach the
# same binary that SRS was built against.
SRS_SLD_PROFILE="$sld_profile" \
    SRS_TOOLCHAIN="${SRS_SLD_BOOTSTRAP_TOOLCHAIN:-stable}" \
    "$root/build-sld.sh"
if [[ ! -x "$sld_bin" ]]; then
    printf 'missing bootstrap sld binary at %s\n' "$sld_bin" >&2
    exit 2
fi

export PATH="$(dirname "$sld_bin"):$PATH"

cd "$rust_dir"
exec ./x build \
    --config "$root/bootstrap.toml" \
    "$@" \
    --stage 2 \
    compiler/rustc \
    rustc_codegen_cranelift \
    library/std \
    src/tools/cargo \
    src/tools/clippy \
    src/tools/rustdoc
