#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch_root="${SRS_WRAPPER_TEST_TMPDIR:-${SRS_TEST_TMPDIR:-$HOME/code/tmp}}"
mkdir -p "$scratch_root"
scratch="$(mktemp -d "$scratch_root/srs-cargo-wrapper.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

cp "$root/cargo-srs.sh" "$scratch/cargo-srs"
cat > "$scratch/cargo-srs-real" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "$SRS_WRAPPER_ARGS"
env | sort > "$SRS_WRAPPER_ENV"
EOF
chmod +x "$scratch/cargo-srs" "$scratch/cargo-srs-real"

mkdir "$scratch/bin"
cat > "$scratch/bin/uname" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
    -s) printf '%s\n' "$SRS_TEST_UNAME_S" ;;
    -m) printf '%s\n' "$SRS_TEST_UNAME_M" ;;
    *) exit 2 ;;
esac
EOF
chmod +x "$scratch/bin/uname"

run_wrapper() {
    local name="$1"
    shift
    SRS_WRAPPER_ARGS="$scratch/$name.args" \
    SRS_WRAPPER_ENV="$scratch/$name.env" \
    HOME="$scratch/home" \
    CARGO_HOME="$scratch/cargo-home" \
    PATH="$scratch/bin:$PATH" \
    "$scratch/cargo-srs" "$@"
}

assert_args() {
    local name="$1"
    shift
    diff -u <(printf '%s\n' "$@") "$scratch/$name.args"
}

assert_env_contains() {
    local name="$1"
    local value="$2"
    grep -Fxq -- "$value" "$scratch/$name.env"
}

assert_env_does_not_contain() {
    local name="$1"
    local key="$2"
    ! grep -Eq "^${key}=" "$scratch/$name.env"
}

unset SLD_INCREMENTAL
unset SLD_INCREMENTAL_PADDING_PERCENT
unset SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS
unset SLD_RUSTC_WORK_PRODUCT_PROVENANCE
unset SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS
unset SRS_TARGET_CODEGEN_BACKEND
unset SRS_ENCODED_TARGET_RUSTFLAGS
unset CARGO_ENCODED_RUSTFLAGS
unset RUSTFLAGS
unset SRS_CARGO_ARTIFACT_CACHE
unset SRS_CARGO_CHECKSUM_FRESHNESS
unset SRS_CARGO_ARTIFACT_CACHE_DIR
unset SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION
unset SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE
unset CARGO_BUILD_ARTIFACT_CACHE_DIR
unset CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION
unset CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE
unset CARGO_UNSTABLE_ARTIFACT_CACHE
unset CARGO_UNSTABLE_CHECKSUM_FRESHNESS

SRS_TEST_UNAME_S=Darwin SRS_TEST_UNAME_M=arm64 run_wrapper darwin-default build
assert_args darwin-default \
    -Z checksum-freshness \
    -Z artifact-cache \
    -Z sld-native-incremental \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm","-C","linker=/usr/bin/clang"]' \
    build
assert_env_contains darwin-default "SLD_INCREMENTAL=1"
assert_env_contains darwin-default "SLD_INCREMENTAL_PADDING_PERCENT=1"
assert_env_contains darwin-default "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS=1"
assert_env_contains darwin-default "SLD_RUSTC_WORK_PRODUCT_PROVENANCE=1"
assert_env_contains darwin-default "SRS_TARGET_CODEGEN_BACKEND=llvm"
assert_env_contains darwin-default "SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS=1"
assert_env_contains darwin-default $'SRS_ENCODED_TARGET_RUSTFLAGS=-Zcodegen-backend=llvm\x1f-Zpreserve-duplicate-constants=yes'
assert_env_contains darwin-default "CARGO_BUILD_ARTIFACT_CACHE_DIR=$scratch/cargo-home/srs-artifact-cache-v2"
assert_env_contains darwin-default "CARGO_UNSTABLE_ARTIFACT_CACHE=true"
assert_env_contains darwin-default "CARGO_UNSTABLE_CHECKSUM_FRESHNESS=true"
assert_env_does_not_contain darwin-default "CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION"
assert_env_does_not_contain darwin-default "CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE"
assert_env_does_not_contain darwin-default "RUSTFLAGS"

SLD_INCREMENTAL=1 \
SLD_INCREMENTAL_PADDING_PERCENT=7 \
SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS=custom \
SLD_RUSTC_WORK_PRODUCT_PROVENANCE=custom \
SRS_TARGET_CODEGEN_BACKEND=cranelift \
SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS=0 \
RUSTFLAGS="-C debuginfo=0 -Zpreserve-duplicate-constants=yes" \
SRS_TEST_UNAME_S=Darwin \
SRS_TEST_UNAME_M=arm64 \
    run_wrapper darwin-overrides build
assert_env_contains darwin-overrides "SLD_INCREMENTAL_PADDING_PERCENT=7"
assert_env_contains darwin-overrides "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS=custom"
assert_env_contains darwin-overrides "SLD_RUSTC_WORK_PRODUCT_PROVENANCE=custom"
assert_env_contains darwin-overrides "SRS_TARGET_CODEGEN_BACKEND=cranelift"
assert_env_contains darwin-overrides "SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS=0"
assert_env_contains darwin-overrides "RUSTFLAGS=-C debuginfo=0 -Zpreserve-duplicate-constants=yes"
assert_env_contains darwin-overrides $'SRS_ENCODED_TARGET_RUSTFLAGS=-Zcodegen-backend=cranelift\x1f-Zpreserve-duplicate-constants=no'

CARGO_ENCODED_RUSTFLAGS="" SRS_TEST_UNAME_S=Darwin SRS_TEST_UNAME_M=arm64 \
    run_wrapper darwin-encoded-empty build
assert_env_contains darwin-encoded-empty "CARGO_ENCODED_RUSTFLAGS="
assert_env_contains darwin-encoded-empty $'SRS_ENCODED_TARGET_RUSTFLAGS=-Zcodegen-backend=llvm\x1f-Zpreserve-duplicate-constants=yes'
assert_env_does_not_contain darwin-encoded-empty "RUSTFLAGS"

CARGO_ENCODED_RUSTFLAGS=$'-Cdebuginfo=0\x1f-Zpreserve-duplicate-constants=yes' \
SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS=0 \
SRS_TEST_UNAME_S=Darwin \
SRS_TEST_UNAME_M=arm64 \
    run_wrapper darwin-encoded-overrides build
assert_env_contains darwin-encoded-overrides $'CARGO_ENCODED_RUSTFLAGS=-Cdebuginfo=0\x1f-Zpreserve-duplicate-constants=yes'
assert_env_contains darwin-encoded-overrides $'SRS_ENCODED_TARGET_RUSTFLAGS=-Zcodegen-backend=llvm\x1f-Zpreserve-duplicate-constants=no'
assert_env_does_not_contain darwin-encoded-overrides "RUSTFLAGS"

SLD_INCREMENTAL=0 SRS_TEST_UNAME_S=Darwin SRS_TEST_UNAME_M=arm64 \
    run_wrapper darwin-disabled build
assert_args darwin-disabled \
    -Z checksum-freshness \
    -Z artifact-cache \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm","-C","linker=/usr/bin/clang"]' \
    build
assert_env_contains darwin-disabled "SLD_INCREMENTAL=0"
assert_env_contains darwin-disabled "SRS_TARGET_CODEGEN_BACKEND=llvm"
assert_env_contains darwin-disabled "SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS=1"
assert_env_contains darwin-disabled $'SRS_ENCODED_TARGET_RUSTFLAGS=-Zcodegen-backend=llvm\x1f-Zpreserve-duplicate-constants=yes'
assert_env_does_not_contain darwin-disabled "RUSTFLAGS"
assert_env_does_not_contain darwin-disabled "SLD_INCREMENTAL_PADDING_PERCENT"
assert_env_does_not_contain darwin-disabled "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS"
assert_env_does_not_contain darwin-disabled "SLD_RUSTC_WORK_PRODUCT_PROVENANCE"

SRS_TEST_UNAME_S=Linux SRS_TEST_UNAME_M=x86_64 run_wrapper linux-default build
assert_args linux-default \
    -Z checksum-freshness \
    -Z artifact-cache \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm"]' \
    build
assert_env_contains linux-default "SLD_INCREMENTAL=1"
assert_env_contains linux-default "CARGO_BUILD_ARTIFACT_CACHE_DIR=$scratch/cargo-home/srs-artifact-cache-v2"
assert_env_does_not_contain linux-default "SLD_INCREMENTAL_PADDING_PERCENT"
assert_env_does_not_contain linux-default "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS"
assert_env_does_not_contain linux-default "SLD_RUSTC_WORK_PRODUCT_PROVENANCE"
assert_env_does_not_contain linux-default "SRS_TARGET_CODEGEN_BACKEND"
assert_env_does_not_contain linux-default "SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS"
assert_env_does_not_contain linux-default "RUSTFLAGS"

SRS_CARGO_ARTIFACT_CACHE_STATS=1 \
SRS_TEST_UNAME_S=Linux \
SRS_TEST_UNAME_M=x86_64 \
    run_wrapper linux-clippy clippy --workspace --all-targets
assert_args linux-clippy \
    -Z checksum-freshness \
    -Z artifact-cache \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm"]' \
    clippy --workspace --all-targets
assert_env_contains linux-clippy "SRS_CARGO_ARTIFACT_CACHE_STATS=1"

SRS_CARGO_ARTIFACT_CACHE_STATS=1 \
SRS_TEST_UNAME_S=Linux \
SRS_TEST_UNAME_M=x86_64 \
    run_wrapper linux-nextest nextest run --profile ci-linux
assert_args linux-nextest \
    -Z checksum-freshness \
    -Z artifact-cache \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm"]' \
    nextest run --profile ci-linux
assert_env_contains linux-nextest "SRS_CARGO_ARTIFACT_CACHE_STATS=1"

SRS_CARGO_ARTIFACT_CACHE=0 \
SRS_TEST_UNAME_S=Linux \
SRS_TEST_UNAME_M=x86_64 \
    run_wrapper cache-disabled build
assert_args cache-disabled \
    -Z checksum-freshness \
    -Z artifact-cache \
    --config build.artifact-cache=false \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm"]' \
    build
assert_env_does_not_contain cache-disabled "CARGO_BUILD_ARTIFACT_CACHE_DIR"
assert_env_does_not_contain cache-disabled "CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION"
assert_env_does_not_contain cache-disabled "CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE"
assert_env_contains cache-disabled "CARGO_UNSTABLE_ARTIFACT_CACHE=false"
assert_env_contains cache-disabled "CARGO_UNSTABLE_CHECKSUM_FRESHNESS=true"

SRS_CARGO_CHECKSUM_FRESHNESS=0 \
SRS_TEST_UNAME_S=Linux \
SRS_TEST_UNAME_M=x86_64 \
    run_wrapper checksum-disabled build
assert_args checksum-disabled \
    -Z artifact-cache \
    -Z host-config \
    -Z target-applies-to-host \
    --config 'target-applies-to-host=false' \
    --config 'host.rustflags=["-Zcodegen-backend=llvm"]' \
    build
assert_env_contains checksum-disabled "CARGO_UNSTABLE_ARTIFACT_CACHE=true"
assert_env_contains checksum-disabled "CARGO_UNSTABLE_CHECKSUM_FRESHNESS=false"

SRS_CARGO_ARTIFACT_CACHE_DIR="$scratch/srs-cache" \
SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION=copy \
SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE=100GiB \
SRS_TEST_UNAME_S=Linux \
SRS_TEST_UNAME_M=x86_64 \
    run_wrapper cache-alias build
assert_env_contains cache-alias "CARGO_BUILD_ARTIFACT_CACHE_DIR=$scratch/srs-cache"
assert_env_contains cache-alias "CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION=copy"
assert_env_contains cache-alias "CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE=100GiB"

SRS_CARGO_ARTIFACT_CACHE_DIR="$scratch/srs-cache" \
SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION=copy \
SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE=100GiB \
CARGO_BUILD_ARTIFACT_CACHE_DIR="$scratch/cargo-cache" \
CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION=hardlink \
CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE=200GiB \
SRS_TEST_UNAME_S=Linux \
SRS_TEST_UNAME_M=x86_64 \
    run_wrapper cache-override build
assert_env_contains cache-override "CARGO_BUILD_ARTIFACT_CACHE_DIR=$scratch/cargo-cache"
assert_env_contains cache-override "CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION=hardlink"
assert_env_contains cache-override "CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE=200GiB"

printf 'SRS Cargo wrapper smoke test passed\n'
