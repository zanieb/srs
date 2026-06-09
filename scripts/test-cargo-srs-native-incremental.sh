#!/usr/bin/env bash
set -euo pipefail

verify_native_sld_replay=0
uname_s="${SRS_TEST_UNAME_S:-$(uname -s)}"
uname_m="${SRS_TEST_UNAME_M:-$(uname -m)}"
if [[ "$uname_s" == "Darwin" && "$uname_m" == "arm64" ]]; then
    verify_native_sld_replay=1
fi

scratch_root="${SRS_TEST_TMPDIR:-$HOME/code/tmp}"
toolchain="${SRS_TEST_TOOLCHAIN:-srs}"
mkdir -p "$scratch_root"
scratch="$(mktemp -d "$scratch_root/srs-native-incremental.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

mkdir -p "$scratch/src" "$scratch/replay-lib/src"
cat > "$scratch/Cargo.toml" <<'EOF'
[package]
name = "srs-native-incremental-replay"
version = "0.1.0"
edition = "2021"

[dependencies]
replay-lib = { path = "replay-lib" }
EOF
cat > "$scratch/src/main.rs" <<'EOF'
fn main() {
    println!("value={}", replay_lib::value());
}
EOF
cat > "$scratch/replay-lib/Cargo.toml" <<'EOF'
[package]
name = "replay-lib"
version = "0.1.0"
edition = "2021"
EOF
cat > "$scratch/replay-lib/src/lib.rs" <<'EOF'
pub static VALUE: i32 = 41;
pub const UNUSED_METADATA: &str = "before";

pub fn value() -> i32 {
    VALUE
}
EOF

target_dir="$scratch/target"
binary="$target_dir/debug/srs-native-incremental-replay"
# CI disables rustc incremental caches for large workload builds. This replay
# specifically needs them so rustc can publish reusable work-product digests.
cargo_incremental="${SRS_TEST_CARGO_INCREMENTAL:-1}"

CARGO_INCREMENTAL="$cargo_incremental" CARGO_TARGET_DIR="$target_dir" \
    cargo +"$toolchain" build --manifest-path "$scratch/Cargo.toml"
if [[ "$("$binary")" != "value=41" ]]; then
    printf 'seed cargo +%s binary did not print value=41\n' "$toolchain" >&2
    exit 1
fi

if [[ "$verify_native_sld_replay" == 1 ]]; then
    shopt -s nullglob
    incremental_logs=("$target_dir"/debug/deps/*.incr/log)
    shopt -u nullglob
    if [[ "${#incremental_logs[@]}" -ne 1 ]]; then
        printf 'expected one private root SLD incremental log, found %s\n' \
            "${#incremental_logs[@]}" >&2
        find "$target_dir" -path '*.incr/log' -print >&2
        exit 1
    fi

    incremental_log="${incremental_logs[0]}"
    log_offset="$(wc -c < "$incremental_log")"
fi
cat > "$scratch/replay-lib/src/lib.rs" <<'EOF'
pub static VALUE: i32 = 42;
pub const UNUSED_METADATA: &str = "before";

pub fn value() -> i32 {
    VALUE
}
EOF

CARGO_INCREMENTAL="$cargo_incremental" CARGO_TARGET_DIR="$target_dir" \
    cargo +"$toolchain" build --manifest-path "$scratch/Cargo.toml"
if [[ "$("$binary")" != "value=42" ]]; then
    printf 'rebuilt cargo +%s binary did not print value=42\n' "$toolchain" >&2
    exit 1
fi

if [[ "$verify_native_sld_replay" == 1 ]]; then
    rebuild_log="$scratch/rebuild.log"
    dd if="$incremental_log" bs=1 skip="$log_offset" 2>/dev/null > "$rebuild_log"
    if ! grep -Fq 'patched 1 changed input file before loading inputs' "$rebuild_log"; then
        printf 'cargo +%s rebuild did not patch its changed rlib input\n' "$toolchain" >&2
        cat "$rebuild_log" >&2
        exit 1
    fi
    if ! grep -Eq 'patched [1-9][0-9]* changed input sections before loading inputs' "$rebuild_log"; then
        printf 'cargo +%s rebuild did not patch changed input sections\n' "$toolchain" >&2
        cat "$rebuild_log" >&2
        exit 1
    fi
    log_offset="$(wc -c < "$incremental_log")"
fi

cat > "$scratch/src/main.rs" <<'EOF'
fn main() {
    println!("VALUE={}", replay_lib::value());
}
EOF

CARGO_INCREMENTAL="$cargo_incremental" CARGO_TARGET_DIR="$target_dir" \
    cargo +"$toolchain" build --manifest-path "$scratch/Cargo.toml"
if [[ "$("$binary")" != "VALUE=42" ]]; then
    printf 'root-edited cargo +%s binary did not print VALUE=42\n' "$toolchain" >&2
    exit 1
fi

if [[ "$verify_native_sld_replay" == 1 ]]; then
    root_rebuild_log="$scratch/root-rebuild.log"
    dd if="$incremental_log" bs=1 skip="$log_offset" 2>/dev/null > "$root_rebuild_log"
    if grep -Eq \
        '^changed-input patch unavailable before loading inputs:|^full relink: input file changed:' \
        "$root_rebuild_log"
    then
        printf 'cargo +%s root rebuild fell back from changed-input patching\n' "$toolchain" >&2
        cat "$root_rebuild_log" >&2
        exit 1
    fi
    if ! grep -Fq 'patched 1 changed input file before loading inputs' "$root_rebuild_log"; then
        printf 'cargo +%s root rebuild did not patch its changed CGU input\n' "$toolchain" >&2
        cat "$root_rebuild_log" >&2
        exit 1
    fi
    if ! grep -Eq 'patched [1-9][0-9]* changed input sections before loading inputs' \
        "$root_rebuild_log"
    then
        printf 'cargo +%s root rebuild did not patch changed input sections\n' "$toolchain" >&2
        cat "$root_rebuild_log" >&2
        exit 1
    fi

    shopt -s nullglob
    replay_rlibs=("$target_dir"/debug/deps/libreplay_lib-*.rlib)
    shopt -u nullglob
    if [[ "${#replay_rlibs[@]}" -ne 1 ]]; then
        printf 'expected one replay-lib rlib, found %s\n' "${#replay_rlibs[@]}" >&2
        exit 1
    fi
    replay_rlib="${replay_rlibs[0]}"
    cp "$binary" "$scratch/binary-before-metadata-edit"
    cp "$replay_rlib" "$scratch/rlib-before-metadata-edit"
    log_offset="$(wc -c < "$incremental_log")"
fi

cat > "$scratch/replay-lib/src/lib.rs" <<'EOF'
pub static VALUE: i32 = 42;
pub const UNUSED_METADATA: &str = "after!";

pub fn value() -> i32 {
    VALUE
}
EOF

CARGO_INCREMENTAL="$cargo_incremental" CARGO_TARGET_DIR="$target_dir" \
    cargo +"$toolchain" build --manifest-path "$scratch/Cargo.toml"
if [[ "$("$binary")" != "VALUE=42" ]]; then
    printf 'metadata-edited cargo +%s binary did not print VALUE=42\n' "$toolchain" >&2
    exit 1
fi

if [[ "$verify_native_sld_replay" == 1 ]]; then
    metadata_rebuild_log="$scratch/metadata-rebuild.log"
    dd if="$incremental_log" bs=1 skip="$log_offset" 2>/dev/null > "$metadata_rebuild_log"
    if cmp -s "$scratch/rlib-before-metadata-edit" "$replay_rlib"; then
        printf 'cargo +%s metadata edit did not rewrite replay-lib rlib\n' "$toolchain" >&2
        exit 1
    fi
    if ! cmp -s "$scratch/binary-before-metadata-edit" "$binary"; then
        printf 'cargo +%s metadata-only edit changed executable bytes\n' "$toolchain" >&2
        exit 1
    fi
    if grep -Eq \
        '^changed-input patch unavailable before loading inputs:|^full relink: input file changed:' \
        "$metadata_rebuild_log"
    then
        printf 'cargo +%s metadata-only edit fell back from incremental reuse\n' \
            "$toolchain" >&2
        cat "$metadata_rebuild_log" >&2
        exit 1
    fi
    if grep -Fq \
        'reused 1 unchanged Rust archive input file by rustc link-content digest before loading inputs' \
        "$metadata_rebuild_log"
    then
        if ! grep -Fq 'reused existing output before loading inputs' "$metadata_rebuild_log"; then
            printf 'cargo +%s metadata-only digest reuse did not reuse the executable\n' \
                "$toolchain" >&2
            cat "$metadata_rebuild_log" >&2
            exit 1
        fi
    else
        if ! grep -Fq 'patched 1 changed input file before loading inputs' \
            "$metadata_rebuild_log"
        then
            printf 'cargo +%s metadata-only edit did not update its changed rlib input\n' \
                "$toolchain" >&2
            cat "$metadata_rebuild_log" >&2
            exit 1
        fi
        if ! grep -Eq 'patched [1-9][0-9]* changed input sections before loading inputs' \
            "$metadata_rebuild_log"
        then
            printf 'cargo +%s metadata-only edit did not refresh its relocation records\n' \
                "$toolchain" >&2
            cat "$metadata_rebuild_log" >&2
            exit 1
        fi
    fi
    log_offset="$(wc -c < "$incremental_log")"
fi

cat > "$scratch/replay-lib/src/lib.rs" <<'EOF'
pub static VALUE: i32 = 43;
pub const UNUSED_METADATA: &str = "after!";

pub fn value() -> i32 {
    VALUE
}
EOF

CARGO_INCREMENTAL="$cargo_incremental" CARGO_TARGET_DIR="$target_dir" \
    cargo +"$toolchain" build --manifest-path "$scratch/Cargo.toml"
if [[ "$("$binary")" != "VALUE=43" ]]; then
    printf 'post-metadata cargo +%s binary did not print VALUE=43\n' "$toolchain" >&2
    exit 1
fi

if [[ "$verify_native_sld_replay" == 1 ]]; then
    post_metadata_rebuild_log="$scratch/post-metadata-rebuild.log"
    dd if="$incremental_log" bs=1 skip="$log_offset" 2>/dev/null > "$post_metadata_rebuild_log"
    if grep -Eq \
        '^changed-input patch unavailable before loading inputs:|^full relink: input file changed:' \
        "$post_metadata_rebuild_log"
    then
        printf 'cargo +%s post-metadata __text edit fell back from changed-input patching\n' \
            "$toolchain" >&2
        cat "$post_metadata_rebuild_log" >&2
        exit 1
    fi
    if ! grep -Fq 'patched 1 changed input file before loading inputs' \
        "$post_metadata_rebuild_log"
    then
        printf 'cargo +%s post-metadata __text edit did not patch its changed rlib input\n' \
            "$toolchain" >&2
        cat "$post_metadata_rebuild_log" >&2
        exit 1
    fi
    if ! grep -Eq 'patched [1-9][0-9]* changed input sections before loading inputs' \
        "$post_metadata_rebuild_log"
    then
        printf 'cargo +%s post-metadata __text edit did not patch changed input sections\n' \
            "$toolchain" >&2
        cat "$post_metadata_rebuild_log" >&2
        exit 1
    fi

    printf 'cargo +%s native incremental replay passed\n' "$toolchain"
else
    printf 'cargo +%s installed incremental replay passed\n' "$toolchain"
fi
