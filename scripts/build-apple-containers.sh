#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
container_bin="${SRS_APPLE_CONTAINER_BIN:-container}"
image="${SRS_APPLE_CONTAINER_IMAGE:-docker.io/library/rust:1.95.0-bookworm}"
arch="${SRS_APPLE_CONTAINER_ARCH:-amd64}"
memory="${SRS_APPLE_CONTAINER_MEMORY:-24g}"
cpus="${SRS_APPLE_CONTAINER_CPUS:-}"
dns="${SRS_APPLE_CONTAINER_DNS:-1.1.1.1}"

linux_build_dir="/work/target/apple-containers/rust-build"
linux_sld_target_dir="/work/target/apple-containers/sld"
linux_cargo_home="/work/target/apple-containers/cargo-home"

run_inside_linux() {
    if [[ "$(uname -s)" != "Linux" ]]; then
        printf '%s must run inside a Linux container\n' "$0 --inside-linux" >&2
        exit 2
    fi

    # The Rust image has rustup, but Rust bootstrap still needs the native build tools.
    apt-get update
    apt-get install -y \
        build-essential \
        cmake \
        curl \
        git \
        libssl-dev \
        ninja-build \
        pkg-config \
        python3 \
        xz-utils

    exec env \
        SRS_CARGO_HOME="$linux_cargo_home" \
        SRS_SLD_BOOTSTRAP_TOOLCHAIN="${SRS_SLD_BOOTSTRAP_TOOLCHAIN:-1.95.0}" \
        SRS_SLD_TARGET_DIR="$linux_sld_target_dir" \
        "$root/build.sh" \
        --build-dir "$linux_build_dir" \
        "$@"
}

if [[ "${1:-}" == "--inside-linux" ]]; then
    shift
    run_inside_linux "$@"
fi

if ! command -v "$container_bin" >/dev/null 2>&1; then
    printf 'missing Apple container CLI: %s\n' "$container_bin" >&2
    exit 2
fi

run_args=(
    run
    --rm
    --arch "$arch"
    --volume "$root:/work"
    --workdir /work
)

if [[ -n "$memory" ]]; then
    run_args+=(--memory "$memory")
fi
if [[ -n "$cpus" ]]; then
    run_args+=(--cpus "$cpus")
fi
if [[ -n "$dns" ]]; then
    run_args+=(--dns "$dns")
fi
if [[ "$arch" == "amd64" || "$arch" == "x86_64" ]]; then
    run_args+=(--rosetta)
fi

exec "$container_bin" "${run_args[@]}" "$image" \
    bash /work/scripts/build-apple-containers.sh --inside-linux "$@"
