#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sld_dir="${SRS_SLD_DIR:-$root/repos/sld}"
toolchain="${SRS_TOOLCHAIN:-srs}"
target_dir="${SRS_SLD_TARGET_DIR:-$root/target/sld}"
cargo_home="${SRS_CARGO_HOME:-$root/cargo-home}"

if [[ ! -f "$sld_dir/Cargo.toml" ]]; then
    printf 'missing sld checkout at %s\n' "$sld_dir" >&2
    exit 2
fi

mkdir -p "$target_dir" "$cargo_home"
cd "$sld_dir"

exec env \
    CARGO_HOME="$cargo_home" \
    CARGO_TARGET_DIR="$target_dir" \
    cargo +"$toolchain" build --locked -p sld-linker --profile opt
