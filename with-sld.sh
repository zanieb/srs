#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sld_bin="${SRS_SLD_BIN:-$root/target/sld/opt/sld}"

if [[ "$#" -eq 0 ]]; then
    printf 'usage: %s command [args...]\n' "$0" >&2
    exit 2
fi

if [[ ! -x "$sld_bin" ]]; then
    printf 'missing sld binary at %s; run %s/build-sld.sh first\n' "$sld_bin" "$root" >&2
    exit 2
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
    printf '%s currently wraps the Darwin sld invocation only\n' "$0" >&2
    exit 2
fi

sld_rustflags="-C linker=$sld_bin -C link-arg=-flavor -C link-arg=darwin"
if [[ -n "${RUSTFLAGS:-}" ]]; then
    sld_rustflags="$RUSTFLAGS $sld_rustflags"
fi

exec env RUSTFLAGS="$sld_rustflags" "$@"
