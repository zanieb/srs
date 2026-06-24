#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
scratch_root="${SRS_TEST_TMPDIR:-$HOME/code/tmp}"
mkdir -p "$scratch_root"
scratch="$(mktemp -d "$scratch_root/srs-install-snapshot.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

toolchain_dir="$scratch/stage2"
cargo_bin="$scratch/cargo"
sld_bin="$scratch/sld"
rustup_bin="$scratch/rustup"
rustup_links="$scratch/rustup-links"
physical_install_root="$scratch/physical-install-root"
install_root="$scratch/install-root"
name="srs-snapshot-smoke"
snapshot_dir="$install_root/$name"
host="fake-host"

write_fake_tool_bins() {
    local version="$1"

    for bin in cargo-clippy cargo-fmt clippy-driver rustfmt; do
        cat > "$toolchain_dir/bin/$bin" <<EOF
#!/usr/bin/env bash
set -euo pipefail
printf 'fake $bin $version\n'
EOF
        chmod +x "$toolchain_dir/bin/$bin"
    done
}

mkdir -p \
    "$toolchain_dir/bin" \
    "$toolchain_dir/lib/rustlib/rustc-src" \
    "$toolchain_dir/lib/rustlib/src" \
    "$rustup_links" \
    "$physical_install_root" \
    "$scratch/rust-source"
ln -s "$physical_install_root" "$install_root"
ln -s "$scratch/rust-source" "$toolchain_dir/lib/rustlib/rustc-src/rust"
ln -s "$scratch/rust-source" "$toolchain_dir/lib/rustlib/src/rust"

cat > "$toolchain_dir/bin/rustc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--print" && "${2:-}" == "host-tuple" ]]; then
    if [[ -n "${SRS_TEST_COLLIDE_BACKUP_BASE:-}" ]]; then
        mkdir -p "$SRS_TEST_COLLIDE_BACKUP_BASE.$PPID"
    fi
    if [[ -n "${SRS_TEST_RETARGET_INSTALL_ROOT:-}" ]]; then
        rm "$SRS_TEST_RETARGET_INSTALL_ROOT"
        ln -s "$SRS_TEST_RETARGET_INSTALL_ROOT_TO" "$SRS_TEST_RETARGET_INSTALL_ROOT"
    fi
    printf 'fake-host\n'
    exit 0
fi
printf 'fake rustc A\n'
EOF
chmod +x "$toolchain_dir/bin/rustc"
write_fake_tool_bins A

cat > "$cargo_bin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'fake cargo A\n'
EOF
chmod +x "$cargo_bin"

cat > "$sld_bin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'fake sld A\n'
EOF
chmod +x "$sld_bin"

cat > "$rustup_bin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$#" -ne 4 || "$1" != "toolchain" || "$2" != "link" ]]; then
    printf 'unexpected fake rustup invocation\n' >&2
    exit 2
fi
if [[ "${SRS_TEST_RUSTUP_FAIL:-0}" == "1" ]]; then
    printf 'requested fake rustup failure\n' >&2
    exit 1
fi
ln -sfn "$4" "$SRS_TEST_RUSTUP_LINKS/$3"
EOF
chmod +x "$rustup_bin"

install_snapshot() {
    env \
        SRS_INSTALL_ROOT="$install_root" \
        SRS_INSTALL_REPLACE="${SRS_INSTALL_REPLACE:-0}" \
        SRS_RUSTUP_BIN="$rustup_bin" \
        SRS_SLD_BIN="$sld_bin" \
        SRS_TEST_COLLIDE_BACKUP_BASE="${SRS_TEST_COLLIDE_BACKUP_BASE:-}" \
        SRS_TEST_RETARGET_INSTALL_ROOT="${SRS_TEST_RETARGET_INSTALL_ROOT:-}" \
        SRS_TEST_RETARGET_INSTALL_ROOT_TO="${SRS_TEST_RETARGET_INSTALL_ROOT_TO:-}" \
        SRS_TEST_RUSTUP_FAIL="${SRS_TEST_RUSTUP_FAIL:-0}" \
        SRS_TEST_RUSTUP_LINKS="$rustup_links" \
        "$root/install.sh" "$name" "$toolchain_dir" "$cargo_bin"
}

if env \
    SRS_INSTALL_ROOT="$install_root" \
    SRS_RUSTUP_BIN="$rustup_bin" \
    SRS_SLD_BIN="$sld_bin" \
    SRS_TEST_RUSTUP_LINKS="$rustup_links" \
    "$root/install.sh" --help "$toolchain_dir" "$cargo_bin" > "$scratch/option-like-name.log" 2>&1
then
    printf 'installer unexpectedly accepted an option-like toolchain name\n' >&2
    exit 1
fi
if ! grep -q 'invalid SRS toolchain name' "$scratch/option-like-name.log"; then
    printf 'installer did not explain the option-like toolchain name refusal\n' >&2
    exit 1
fi
if env \
    SRS_INSTALL_ROOT="$install_root" \
    SRS_RUSTUP_BIN="$rustup_bin" \
    SRS_SLD_BIN="$sld_bin" \
    SRS_TEST_RUSTUP_LINKS="$rustup_links" \
    "$root/install.sh" .private-label "$toolchain_dir" "$cargo_bin" > "$scratch/private-name.log" 2>&1
then
    printf 'installer unexpectedly accepted a private-prefix toolchain name\n' >&2
    exit 1
fi
if ! grep -q 'invalid SRS toolchain name' "$scratch/private-name.log"; then
    printf 'installer did not explain the private-prefix toolchain name refusal\n' >&2
    exit 1
fi

install_snapshot
snapshot_dir="$physical_install_root/$name"

if [[ "$(readlink "$rustup_links/$name")" != "$snapshot_dir" ]]; then
    printf 'fake rustup link did not point at installed snapshot\n' >&2
    exit 1
fi

for copied_path in \
    "$snapshot_dir/bin/cargo" \
    "$snapshot_dir/bin/cargo-srs-real" \
    "$snapshot_dir/lib/rustlib/$host/bin/sld"
do
    if [[ -L "$copied_path" ]]; then
        printf 'installed attachment remained a symlink: %s\n' "$copied_path" >&2
        exit 1
    fi
done

for omitted_path in \
    "$snapshot_dir/lib/rustlib/rustc-src/rust" \
    "$snapshot_dir/lib/rustlib/src/rust"
do
    if [[ -e "$omitted_path" || -L "$omitted_path" ]]; then
        printf 'installed snapshot retained mutable rust-src symlink: %s\n' "$omitted_path" >&2
        exit 1
    fi
done

before="$(cksum \
    "$snapshot_dir/bin/rustc" \
    "$snapshot_dir/bin/cargo-clippy" \
    "$snapshot_dir/bin/cargo-fmt" \
    "$snapshot_dir/bin/clippy-driver" \
    "$snapshot_dir/bin/rustfmt" \
    "$snapshot_dir/bin/cargo" \
    "$snapshot_dir/bin/cargo-srs-real" \
    "$snapshot_dir/lib/rustlib/$host/bin/sld")"

rm -rf "$toolchain_dir"
rm -f "$cargo_bin" "$sld_bin"

if [[ "$("$snapshot_dir/bin/rustc")" != "fake rustc A" ]]; then
    printf 'installed rustc stopped working after source cleanup\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installed Cargo stopped working after source cleanup\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/cargo-clippy")" != "fake cargo-clippy A" ]]; then
    printf 'installed cargo-clippy stopped working after source cleanup\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/cargo-fmt")" != "fake cargo-fmt A" ]]; then
    printf 'installed cargo-fmt stopped working after source cleanup\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/clippy-driver")" != "fake clippy-driver A" ]]; then
    printf 'installed clippy-driver stopped working after source cleanup\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/rustfmt")" != "fake rustfmt A" ]]; then
    printf 'installed rustfmt stopped working after source cleanup\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/lib/rustlib/$host/bin/sld")" != "fake sld A" ]]; then
    printf 'installed sld stopped working after source cleanup\n' >&2
    exit 1
fi

after="$(cksum \
    "$snapshot_dir/bin/rustc" \
    "$snapshot_dir/bin/cargo-clippy" \
    "$snapshot_dir/bin/cargo-fmt" \
    "$snapshot_dir/bin/clippy-driver" \
    "$snapshot_dir/bin/rustfmt" \
    "$snapshot_dir/bin/cargo" \
    "$snapshot_dir/bin/cargo-srs-real" \
    "$snapshot_dir/lib/rustlib/$host/bin/sld")"
if [[ "$before" != "$after" ]]; then
    printf 'installed snapshot changed after source cleanup\n' >&2
    exit 1
fi

if install_snapshot > "$scratch/reinstall.log" 2>&1; then
    printf 'installer unexpectedly replaced an existing label without opt-in\n' >&2
    exit 1
fi
if ! grep -q 'SRS toolchain snapshot already exists' "$scratch/reinstall.log"; then
    printf 'installer did not explain the existing-label refusal\n' >&2
    exit 1
fi

lock_file="$physical_install_root/.${name}.lock"
transaction_dir="$physical_install_root/.${name}.transaction"
exec 8> "$lock_file"
case "$(uname -s)" in
    Darwin) lockf -s -t 0 8 ;;
    Linux) flock -n 8 ;;
    *)
        printf 'unsupported smoke-test host: %s\n' "$(uname -s)" >&2
        exit 1
        ;;
esac
if SRS_INSTALL_REPLACE=1 install_snapshot > "$scratch/concurrent-install.log" 2>&1; then
    printf 'installer unexpectedly allowed concurrent installation of the same label\n' >&2
    exit 1
fi
if ! grep -q 'installation is already in progress' "$scratch/concurrent-install.log"; then
    printf 'installer did not explain the concurrent same-label refusal\n' >&2
    exit 1
fi
exec 8>&-
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installer changed snapshot while refusing concurrent same-label installation\n' >&2
    exit 1
fi

lock_sentinel="$scratch/lock-sentinel"
printf 'lock sentinel\n' > "$lock_sentinel"
rm "$lock_file"
ln -s "$lock_sentinel" "$lock_file"
if install_snapshot > "$scratch/invalid-lock-file.log" 2>&1; then
    printf 'installer unexpectedly accepted a symlinked lock file\n' >&2
    exit 1
fi
if ! grep -q 'refusing invalid SRS toolchain snapshot lock file' "$scratch/invalid-lock-file.log"; then
    printf 'installer did not explain the symlinked lock-file refusal\n' >&2
    exit 1
fi
if [[ "$(cat "$lock_sentinel")" != "lock sentinel" ]]; then
    printf 'installer followed a symlinked lock file\n' >&2
    exit 1
fi
rm "$lock_file"

read_only_install_root="$scratch/read-only-install-root"
mkdir "$read_only_install_root"
chmod a-w "$read_only_install_root"
if [[ ! -w "$read_only_install_root" ]]; then
    if env \
        SRS_INSTALL_ROOT="$read_only_install_root" \
        SRS_RUSTUP_BIN="$rustup_bin" \
        SRS_SLD_BIN="$sld_bin" \
        SRS_TEST_RUSTUP_LINKS="$rustup_links" \
        "$root/install.sh" srs-read-only-root "$toolchain_dir" "$cargo_bin" > "$scratch/read-only-root.log" 2>&1
    then
        printf 'installer unexpectedly accepted an unwritable snapshot root\n' >&2
        exit 1
    fi
    if ! grep -q 'failed to open SRS toolchain snapshot lock file' "$scratch/read-only-root.log"; then
        printf 'installer did not explain the unwritable snapshot-root refusal\n' >&2
        exit 1
    fi
fi
chmod u+w "$read_only_install_root"

failing_mktemp_bin="$scratch/failing-mktemp-bin"
mkdir "$failing_mktemp_bin"
cat > "$failing_mktemp_bin/mktemp" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
chmod +x "$failing_mktemp_bin/mktemp"
if env \
    PATH="$failing_mktemp_bin:$PATH" \
    SRS_INSTALL_ROOT="$install_root" \
    SRS_RUSTUP_BIN="$rustup_bin" \
    SRS_SLD_BIN="$sld_bin" \
    SRS_TEST_RUSTUP_LINKS="$rustup_links" \
    "$root/install.sh" srs-mktemp-failure "$toolchain_dir" "$cargo_bin" > "$scratch/mktemp-failure.log" 2>&1
then
    printf 'installer unexpectedly accepted a failed transaction-directory creation\n' >&2
    exit 1
fi
if ! grep -q 'failed to create SRS toolchain snapshot transaction' "$scratch/mktemp-failure.log"; then
    printf 'installer did not explain the failed transaction-directory creation\n' >&2
    exit 1
fi
if [[ -e "$physical_install_root/.srs-mktemp-failure.transaction" || -L "$physical_install_root/.srs-mktemp-failure.transaction" ]]; then
    printf 'installer left transaction metadata after failed transaction-directory creation\n' >&2
    exit 1
fi

transaction_sentinel="$scratch/transaction-sentinel"
mkdir "$transaction_sentinel"
ln -s "$transaction_sentinel" "$transaction_dir"
if install_snapshot > "$scratch/invalid-transaction-path.log" 2>&1; then
    printf 'installer unexpectedly accepted a symlinked transaction path\n' >&2
    exit 1
fi
if ! grep -q 'refusing invalid SRS toolchain snapshot transaction path' "$scratch/invalid-transaction-path.log"; then
    printf 'installer did not explain the symlinked transaction-path refusal\n' >&2
    exit 1
fi
rm "$transaction_dir"

mkdir "$transaction_dir"
printf 'transaction marker\n' > "$transaction_dir/marker"
if install_snapshot > "$scratch/missing-transaction-phase.log" 2>&1; then
    printf 'installer unexpectedly accepted transaction metadata without a phase\n' >&2
    exit 1
fi
if ! grep -q 'refusing SRS toolchain snapshot transaction without readable phase' "$scratch/missing-transaction-phase.log"; then
    printf 'installer did not explain the missing transaction phase refusal\n' >&2
    exit 1
fi
if [[ "$(cat "$transaction_dir/marker")" != "transaction marker" ]]; then
    printf 'installer removed transaction metadata without a readable phase\n' >&2
    exit 1
fi
rm -rf "$transaction_dir"

unowned_staging_dir="$physical_install_root/.${name}.tmp.preserved"
mkdir "$unowned_staging_dir"
printf 'preserved marker\n' > "$unowned_staging_dir/marker"
if install_snapshot > "$scratch/unowned-staging.log" 2>&1; then
    printf 'installer unexpectedly replaced a snapshot while checking unowned staging paths\n' >&2
    exit 1
fi
if [[ "$(cat "$unowned_staging_dir/marker")" != "preserved marker" ]]; then
    printf 'installer removed an unowned private-prefix staging path\n' >&2
    exit 1
fi
rm -rf "$unowned_staging_dir"

mkdir -p "$transaction_dir/staging"
printf 'preparing\n' > "$transaction_dir/phase"
mv "$snapshot_dir" "$transaction_dir/replaced-snapshot"
if install_snapshot > "$scratch/stale-install.log" 2>&1; then
    printf 'installer unexpectedly replaced a recovered snapshot without opt-in\n' >&2
    exit 1
fi
if ! grep -q 'recovering stale SRS toolchain snapshot installation' "$scratch/stale-install.log"; then
    printf 'installer did not explain stale installation recovery\n' >&2
    exit 1
fi
if ! grep -q 'SRS toolchain snapshot already exists' "$scratch/stale-install.log"; then
    printf 'installer did not restore immutable-label handling after stale recovery\n' >&2
    exit 1
fi
if [[ -e "$transaction_dir" || -L "$transaction_dir" ]]; then
    printf 'installer left stale transaction paths after recovery\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installer did not restore snapshot after stale replacement recovery\n' >&2
    exit 1
fi

mkdir -p \
    "$toolchain_dir/bin" \
    "$toolchain_dir/lib/rustlib/rustc-src/rust" \
    "$toolchain_dir/lib/rustlib/src/rust"
printf 'preserved rustc source\n' > "$toolchain_dir/lib/rustlib/rustc-src/rust/README"
printf 'preserved library source\n' > "$toolchain_dir/lib/rustlib/src/rust/README"
cat > "$toolchain_dir/bin/rustc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "--print" && "${2:-}" == "host-tuple" ]]; then
    if [[ -n "${SRS_TEST_COLLIDE_BACKUP_BASE:-}" ]]; then
        mkdir -p "$SRS_TEST_COLLIDE_BACKUP_BASE.$PPID"
    fi
    if [[ -n "${SRS_TEST_RETARGET_INSTALL_ROOT:-}" ]]; then
        rm "$SRS_TEST_RETARGET_INSTALL_ROOT"
        ln -s "$SRS_TEST_RETARGET_INSTALL_ROOT_TO" "$SRS_TEST_RETARGET_INSTALL_ROOT"
    fi
    printf 'fake-host\n'
    exit 0
fi
printf 'fake rustc B\n'
EOF
chmod +x "$toolchain_dir/bin/rustc"
write_fake_tool_bins B

cat > "$cargo_bin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'fake cargo B\n'
EOF
chmod +x "$cargo_bin"

cat > "$sld_bin" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'fake sld B\n'
EOF
chmod +x "$sld_bin"

mkdir -p "$transaction_dir/staging"
printf 'published\n' > "$transaction_dir/phase"
mv "$snapshot_dir" "$transaction_dir/replaced-snapshot"
mkdir -p "$snapshot_dir/bin"
cat > "$snapshot_dir/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'stale published cargo\n'
EOF
chmod +x "$snapshot_dir/bin/cargo"
if install_snapshot > "$scratch/stale-published-replacement.log" 2>&1; then
    printf 'installer unexpectedly retained an interrupted replacement without opt-in\n' >&2
    exit 1
fi
if ! grep -q 'recovering stale SRS toolchain snapshot installation' "$scratch/stale-published-replacement.log"; then
    printf 'installer did not explain interrupted replacement recovery\n' >&2
    exit 1
fi
if ! grep -q 'SRS toolchain snapshot already exists' "$scratch/stale-published-replacement.log"; then
    printf 'installer did not restore immutable-label handling after interrupted replacement recovery\n' >&2
    exit 1
fi
if [[ -e "$transaction_dir" || -L "$transaction_dir" ]]; then
    printf 'installer left interrupted replacement transaction paths after recovery\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installer did not roll back an interrupted published replacement\n' >&2
    exit 1
fi

stale_initial_name="srs-stale-initial"
stale_initial_snapshot="$physical_install_root/$stale_initial_name"
stale_initial_transaction="$physical_install_root/.${stale_initial_name}.transaction"
mkdir -p "$stale_initial_snapshot/bin" "$stale_initial_transaction"
printf 'stale snapshot marker\n' > "$stale_initial_snapshot/bin/stale-marker"
printf 'published\n' > "$stale_initial_transaction/phase"
env \
    SRS_INSTALL_ROOT="$install_root" \
    SRS_RUSTUP_BIN="$rustup_bin" \
    SRS_SLD_BIN="$sld_bin" \
    SRS_TEST_RUSTUP_LINKS="$rustup_links" \
    "$root/install.sh" "$stale_initial_name" "$toolchain_dir" "$cargo_bin"
if [[ -e "$stale_initial_snapshot/bin/stale-marker" ]]; then
    printf 'installer retained an interrupted initial publication\n' >&2
    exit 1
fi
if [[ "$("$stale_initial_snapshot/bin/cargo")" != "fake cargo B" ]]; then
    printf 'installer did not recover an interrupted initial publication\n' >&2
    exit 1
fi
if [[ -e "$stale_initial_transaction" || -L "$stale_initial_transaction" ]]; then
    printf 'installer left stale initial-publication transaction paths after recovery\n' >&2
    exit 1
fi

stale_linking_name="srs-stale-linking"
stale_linking_snapshot="$physical_install_root/$stale_linking_name"
stale_linking_transaction="$physical_install_root/.${stale_linking_name}.transaction"
mkdir -p "$stale_linking_snapshot/bin" "$stale_linking_transaction"
printf 'linked snapshot marker\n' > "$stale_linking_snapshot/bin/linked-marker"
printf 'linking\n' > "$stale_linking_transaction/phase"
if env \
    SRS_INSTALL_ROOT="$install_root" \
    SRS_RUSTUP_BIN="$rustup_bin" \
    SRS_SLD_BIN="$sld_bin" \
    SRS_TEST_RUSTUP_LINKS="$rustup_links" \
    "$root/install.sh" "$stale_linking_name" "$toolchain_dir" "$cargo_bin" > "$scratch/stale-linking.log" 2>&1
then
    printf 'installer unexpectedly replaced a recovered linking snapshot without opt-in\n' >&2
    exit 1
fi
if [[ ! -e "$stale_linking_snapshot/bin/linked-marker" ]]; then
    printf 'installer did not retain a linking snapshot during stale cleanup\n' >&2
    exit 1
fi
if [[ "$(readlink "$rustup_links/$stale_linking_name")" != "$stale_linking_snapshot" ]]; then
    printf 'installer did not replay an interrupted rustup link\n' >&2
    exit 1
fi
if [[ -e "$stale_linking_transaction" || -L "$stale_linking_transaction" ]]; then
    printf 'installer left linking transaction paths after stale cleanup\n' >&2
    exit 1
fi

stale_complete_name="srs-stale-complete"
stale_complete_snapshot="$physical_install_root/$stale_complete_name"
stale_complete_transaction="$physical_install_root/.${stale_complete_name}.transaction"
mkdir -p "$stale_complete_snapshot/bin" "$stale_complete_transaction/replaced-snapshot/bin"
cat > "$stale_complete_snapshot/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'committed cargo\n'
EOF
chmod +x "$stale_complete_snapshot/bin/cargo"
printf 'old snapshot marker\n' > "$stale_complete_transaction/replaced-snapshot/bin/old-marker"
printf 'complete\n' > "$stale_complete_transaction/phase"
if env \
    SRS_INSTALL_ROOT="$install_root" \
    SRS_RUSTUP_BIN="$rustup_bin" \
    SRS_SLD_BIN="$sld_bin" \
    SRS_TEST_RUSTUP_LINKS="$rustup_links" \
    "$root/install.sh" "$stale_complete_name" "$toolchain_dir" "$cargo_bin" > "$scratch/stale-complete.log" 2>&1
then
    printf 'installer unexpectedly replaced a recovered committed snapshot without opt-in\n' >&2
    exit 1
fi
if [[ "$("$stale_complete_snapshot/bin/cargo")" != "committed cargo" ]]; then
    printf 'installer did not retain a committed snapshot during stale cleanup\n' >&2
    exit 1
fi
if [[ -e "$stale_complete_transaction" || -L "$stale_complete_transaction" ]]; then
    printf 'installer left committed transaction paths after stale cleanup\n' >&2
    exit 1
fi

if \
    SRS_INSTALL_REPLACE=1 \
    SRS_TEST_RUSTUP_FAIL=1 \
    SRS_TEST_COLLIDE_BACKUP_BASE="$snapshot_dir.replaced" \
    install_snapshot > "$scratch/replace-failure.log" 2>&1
then
    printf 'installer unexpectedly accepted requested fake rustup failure\n' >&2
    exit 1
fi
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installer did not roll back snapshot after rustup failure\n' >&2
    exit 1
fi
shopt -s nullglob
collision_backups=("$snapshot_dir".replaced.*)
shopt -u nullglob
if [[ "${#collision_backups[@]}" -ne 1 ]]; then
    printf 'expected exactly one injected predictable-backup collision, found %s\n' "${#collision_backups[@]}" >&2
    exit 1
fi
rm -rf "${collision_backups[@]}"

ln -s "$scratch/rust-source" "$toolchain_dir/lib/mutable-external"
if SRS_INSTALL_REPLACE=1 install_snapshot > "$scratch/external-symlink.log" 2>&1; then
    printf 'installer unexpectedly accepted a retained absolute symlink\n' >&2
    exit 1
fi
if ! grep -q 'refusing absolute symlink in SRS toolchain snapshot' "$scratch/external-symlink.log"; then
    printf 'installer did not explain the retained absolute symlink refusal\n' >&2
    exit 1
fi
rm "$toolchain_dir/lib/mutable-external"
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installer changed snapshot after rejecting an absolute symlink\n' >&2
    exit 1
fi

ln -s ../../rust-source "$toolchain_dir/lib/mutable-relative-external"
if SRS_INSTALL_REPLACE=1 install_snapshot > "$scratch/relative-external-symlink.log" 2>&1; then
    printf 'installer unexpectedly accepted a retained external relative symlink\n' >&2
    exit 1
fi
if ! grep -q 'refusing external relative symlink in SRS toolchain snapshot' "$scratch/relative-external-symlink.log"; then
    printf 'installer did not explain the retained external relative symlink refusal\n' >&2
    exit 1
fi
rm "$toolchain_dir/lib/mutable-relative-external"
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installer changed snapshot after rejecting an external relative symlink\n' >&2
    exit 1
fi

ln -s ../.. "$toolchain_dir/lib/x"
if SRS_INSTALL_REPLACE=1 install_snapshot > "$scratch/relative-parent-escape-symlink.log" 2>&1; then
    printf 'installer unexpectedly accepted a retained parent-escaping relative symlink\n' >&2
    exit 1
fi
if ! grep -q 'refusing external relative symlink in SRS toolchain snapshot' "$scratch/relative-parent-escape-symlink.log"; then
    printf 'installer did not explain the retained parent-escaping relative symlink refusal\n' >&2
    exit 1
fi
rm "$toolchain_dir/lib/x"
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo A" ]]; then
    printf 'installer changed snapshot after rejecting a parent-escaping relative symlink\n' >&2
    exit 1
fi

mkdir -p "$toolchain_dir/lib/internal-target" "$toolchain_dir/lib/-internal-target"
ln -s internal-target "$toolchain_dir/lib/internal-relative"
ln -s -- -internal-target "$toolchain_dir/lib/dash-prefixed-internal-relative"
SRS_INSTALL_REPLACE=1 install_snapshot
if [[ "$("$snapshot_dir/bin/cargo")" != "fake cargo B" ]]; then
    printf 'installer did not replace snapshot after explicit opt-in\n' >&2
    exit 1
fi
if [[ ! -L "$snapshot_dir/lib/internal-relative" || "$(readlink "$snapshot_dir/lib/internal-relative")" != "internal-target" ]]; then
    printf 'installer did not retain an internal relative symlink\n' >&2
    exit 1
fi
if [[ ! -L "$snapshot_dir/lib/dash-prefixed-internal-relative" || "$(readlink "$snapshot_dir/lib/dash-prefixed-internal-relative")" != "-internal-target" ]]; then
    printf 'installer did not retain a dash-prefixed internal relative symlink\n' >&2
    exit 1
fi
for preserved_path in \
    "$snapshot_dir/lib/rustlib/rustc-src/rust/README" \
    "$snapshot_dir/lib/rustlib/src/rust/README"
do
    if [[ ! -f "$preserved_path" ]]; then
        printf 'installer did not retain copied rust-src directory content: %s\n' "$preserved_path" >&2
        exit 1
    fi
done

retarget_physical_install_root="$scratch/retarget-physical-install-root"
retarget_other_install_root="$scratch/retarget-other-install-root"
retarget_install_root="$scratch/retarget-install-root"
retarget_name="srs-retarget-smoke"
mkdir -p "$retarget_physical_install_root" "$retarget_other_install_root"
ln -s "$retarget_physical_install_root" "$retarget_install_root"
env \
    SRS_INSTALL_ROOT="$retarget_install_root" \
    SRS_RUSTUP_BIN="$rustup_bin" \
    SRS_SLD_BIN="$sld_bin" \
    SRS_TEST_RETARGET_INSTALL_ROOT="$retarget_install_root" \
    SRS_TEST_RETARGET_INSTALL_ROOT_TO="$retarget_other_install_root" \
    SRS_TEST_RUSTUP_LINKS="$rustup_links" \
    "$root/install.sh" "$retarget_name" "$toolchain_dir" "$cargo_bin"
if [[ ! -x "$retarget_physical_install_root/$retarget_name/bin/rustc" ]]; then
    printf 'installer did not publish into the pinned physical install root\n' >&2
    exit 1
fi
if [[ -e "$retarget_other_install_root/$retarget_name" || -L "$retarget_other_install_root/$retarget_name" ]]; then
    printf 'installer followed a retargeted install-root symlink during publication\n' >&2
    exit 1
fi
if [[ "$(readlink "$rustup_links/$retarget_name")" != "$retarget_physical_install_root/$retarget_name" ]]; then
    printf 'installer did not link rustup to the pinned physical snapshot path\n' >&2
    exit 1
fi

printf 'immutable install snapshot smoke passed\n'
