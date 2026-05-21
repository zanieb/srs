#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
name="${1:-srs}"
rust_dir="${SRS_RUST_DIR:-$root/repos/rust}"
toolchain_dir="${2:-}"
cargo_bin="${3:-}"
sld_bin="${SRS_SLD_BIN:-$root/target/sld/opt/sld}"

if [[ -z "$toolchain_dir" ]]; then
    host_stage2="$rust_dir/build/host/stage2"
    if [[ -x "$host_stage2/bin/rustc" ]]; then
        toolchain_dir="$host_stage2"
    else
        shopt -s nullglob
        stage2_dirs=("$rust_dir"/build/*/stage2)
        shopt -u nullglob

        if [[ "${#stage2_dirs[@]}" -ne 1 ]]; then
            printf 'expected exactly one stage2 toolchain under %s/build; pass the toolchain path explicitly\n' "$rust_dir" >&2
            exit 2
        fi

        toolchain_dir="${stage2_dirs[0]}"
    fi
fi

if [[ ! -x "$toolchain_dir/bin/rustc" ]]; then
    printf 'missing rustc in toolchain directory %s\n' "$toolchain_dir" >&2
    exit 2
fi

if [[ -z "$cargo_bin" ]]; then
    host_cargo="$rust_dir/build/host/stage2-tools-bin/cargo"
    if [[ -x "$host_cargo" ]]; then
        cargo_bin="$host_cargo"
    else
        shopt -s nullglob
        cargo_bins=("$rust_dir"/build/*/stage2-tools-bin/cargo)
        shopt -u nullglob

        if [[ "${#cargo_bins[@]}" -ne 1 ]]; then
            printf 'expected exactly one stage2 Cargo under %s/build; pass the Cargo path explicitly\n' "$rust_dir" >&2
            exit 2
        fi

        cargo_bin="${cargo_bins[0]}"
    fi
fi

if [[ ! -x "$cargo_bin" ]]; then
    printf 'missing built Cargo at %s\n' "$cargo_bin" >&2
    exit 2
fi

if [[ ! -x "$sld_bin" ]]; then
    printf 'missing sld binary at %s; run %s/build-sld.sh first\n' "$sld_bin" "$root" >&2
    exit 2
fi

# A linked Rust bootstrap sysroot does not include Cargo by default. Keep the
# built Cargo binary next to an SRS wrapper that separates host helpers from
# normal Cranelift target artifacts.
ln -sf "$cargo_bin" "$toolchain_dir/bin/cargo-srs-real"
ln -sf "$root/cargo-srs.sh" "$toolchain_dir/bin/cargo"

# Rustc prepends this per-target tools directory to PATH before spawning the
# configured linker. Keep the baked-in default linker name relocatable.
host="$("$toolchain_dir/bin/rustc" --print host-tuple)"
tools_bin="$toolchain_dir/lib/rustlib/$host/bin"
mkdir -p "$tools_bin"
ln -sf "$sld_bin" "$tools_bin/sld"

rustup toolchain link "$name" "$toolchain_dir"
printf 'linked rustup toolchain %s -> %s\n' "$name" "$toolchain_dir"
printf 'attached Cargo wrapper %s -> %s\n' "$toolchain_dir/bin/cargo" "$root/cargo-srs.sh"
printf 'attached sld linker %s -> %s\n' "$tools_bin/sld" "$sld_bin"
