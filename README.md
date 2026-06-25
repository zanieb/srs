# SRS

SRS is the slop Rust stack: a downstream Rust toolchain repository that carries
the patched Cranelift backend needed to compile Astral workloads on macOS arm64.

## Layout

- `rust/`: the downstream Rust source tree that builds the toolchain and owns
  the `compiler/rustc_codegen_cranelift` integration.
- `rust/src/tools/cargo/`: Cargo, tracked as ordinary SRS content because
  bootstrap requires it at this path.
- `rust/src/tools/clippy/`: Clippy, also tracked as ordinary SRS content.
- `cranelift/`: the patched Wasmtime/Cranelift source tree consumed by the
  backend in `rust/`.
- `sld/`: the downstream `sld` linker source tree.
- `bootstrap.toml`: Rust bootstrap configuration for the SRS toolchain build.
- `build.sh`: builds `sld`, then a stage 2 Rust toolchain with LLVM and
  Cranelift backends, plus Cargo from the Rust tree.
- `build-sld.sh`: builds the `sld` binary, using the linked SRS toolchain by
  default when called directly.
- `scripts/pull-upstream.sh`: refreshes the imported downstream source trees
  from their upstream repositories for review.
- `scripts/build-apple-containers.sh`: builds the Linux x86_64 SRS lane from
  macOS through Apple containers.
- `install.sh`: clones or copies the built stage 2 toolchain into an immutable
  per-name snapshot, attaches copied SRS Cargo plus `sld`, and links that
  snapshot into rustup under a custom name.
- `cargo-srs.sh`: the installed Cargo wrapper that keeps build scripts, proc
  macros, their host-side dependencies, and ordinary target artifacts on LLVM
  by default, and starts the macOS arm64 incremental-link lane.
- `with-sld.sh`: runs a command with the macOS Rust flags needed to link
  through SRS's built `sld` binary.

The bootstrap config keeps LLVM first in `rust.codegen-backends` and forces the
bootstrap build back through LLVM. The Rust source tree makes the installed
macOS arm64 and Linux x86_64 compilers prefer Cranelift when invoked directly.
The installed Cargo wrapper keeps build scripts, proc macros, and their
host-side dependencies on LLVM because helpers can exercise host intrinsics
that Cranelift does not support yet. It also selects LLVM for ordinary target
artifacts by default because workload runtime remains substantially faster
with LLVM. Set
`SRS_TARGET_CODEGEN_BACKEND=cranelift` to exercise the composed backend path
explicitly. For macOS arm64, SRS bakes `sld` in as rustc's default linker.

## Quick Start

Initialize the remaining external dependency submodules after cloning SRS:

```bash
git submodule update --init --recursive
```

The top-level `rust/`, `cranelift/`, and `sld/` trees are already present in the
SRS checkout. Cargo and Clippy are already present under `rust/src/tools/`.
Submodules are reserved for nested external dependencies such as LLVM,
documentation repositories, test suites, and linker fixtures.

Build a stage 2 toolchain and link it into rustup as `srs`:

```bash
./build.sh
./install.sh
```

Check that rustup can see it:

```bash
rustc +srs -Vv
cargo +srs -Vv
```

## Install A Release

Tagged releases provide self-contained toolchain snapshots for Linux x86_64
and Apple silicon macOS. Download the archive matching the host tuple reported
by Rust, verify its checksum, and link the extracted snapshot into rustup:

```bash
tag=2026.06.08
target="$(rustc -vV | sed -n 's/^host: //p')"
archive="srs-${tag}-${target}.tar.gz"
install_root="$HOME/code/tmp/srs-toolchains"

mkdir -p "$install_root"
gh release download "$tag" \
    --repo zanieb/srs \
    --pattern "$archive*" \
    --dir "$install_root"
(cd "$install_root" && shasum -a 256 -c "$archive.sha256")
tar -C "$install_root" -xzf "$install_root/$archive"
rustup toolchain link "srs-${tag}" \
    "$install_root/srs-${tag}-${target}"
```

Dispatch the Release workflow from `main` to publish a release. It uses the
dispatch's UTC date as a zero-padded CalVer tag such as `2026.06.08`, builds
both supported hosts, and creates the tag only after both archives are ready.
A second release on the same UTC date is refused instead of replacing an
existing tag or release.

`./build.sh` is the slow step. Re-run it after changing Rust, Cargo, Clippy,
cg_clif, Cranelift, or the linker default. It builds `sld` with `stable` first so the
installer can attach the built binary. Rust bootstrap stays on the system
compiler driver while the installed SRS compiler defaults to `sld`; set
`SRS_SLD_BOOTSTRAP_TOOLCHAIN` to choose another existing rustup toolchain for
that step. `./install.sh` snapshots the resulting stage 2 sysroot under
`${SRS_INSTALL_ROOT:-$HOME/code/tmp/srs-toolchains}/srs`. The installer uses
copy-on-write filesystem clones where available and a portable copy fallback
otherwise. It also copies the Cargo wrapper, Cargo binary, and `sld`, so an
installed name continues to work after later rebuilds or source-worktree
cleanup. Mutable bootstrap `rust-src` symlinks back into the source checkout
are intentionally omitted from the installed snapshot. Explicit sysroots with
copied `rust-src` directories keep those sources. A normal bootstrap snapshot
remains suitable for normal `rustc`, Cargo, and `sld` development builds after
source-worktree cleanup, but is not a `rust-src`-bearing toolchain. Workflows
that require compiler sources inside the sysroot, such as Cargo `-Z build-std`,
must use the task worktree's stage 2 sysroot instead. If that worktree has
already been removed, recreate and rebuild one; replacing an installed
snapshot intentionally does not restore the mutable `rust-src` symlinks.

On Apple silicon macOS, use Apple containers to exercise the Linux x86_64 build
lane locally:

```bash
container system start
./scripts/build-apple-containers.sh
```

The script mounts SRS at `/work`, runs an amd64 Linux Rust image with Rosetta,
installs the Linux build dependencies in that disposable container, and keeps
its build output under `target/apple-containers/`. It passes additional
arguments through to `build.sh`, for example
`./scripts/build-apple-containers.sh --dry-run -v`. Set
`SRS_APPLE_CONTAINER_IMAGE`, `SRS_APPLE_CONTAINER_MEMORY`,
`SRS_APPLE_CONTAINER_CPUS`, or `SRS_APPLE_CONTAINER_DNS` to tune the container
run.

## Usage

The SRS compiler still has Cranelift available. Its Cargo wrapper uses LLVM for
host build helpers and ordinary target artifacts by default on Linux x86_64
and macOS arm64. On macOS arm64 it links through the `sld` binary attached to
the installed toolchain:

```bash
cargo +srs build
```

The installed Cargo wrapper sets `SLD_INCREMENTAL=1` by default so `sld` can
reuse link state across development builds. It passes `-Z checksum-freshness`
so touched-but-unchanged source files stay fresh and source content changes are
detected even when their mtimes are preserved. Regular files declared through
build-script `rerun-if-changed` directives use the same content-based freshness.
Directories, symlinks, and unavailable watched paths retain Cargo's
conservative mtime behavior. Cargo also preserves its invocation-time guard
for watched files modified while a build script is running.

On Apple silicon macOS, the wrapper also
requests signed private root-executable outputs and transient-input
stabilization from the patched Cargo binary. Rustc also supplies SLD with
persisted work-product digests so unchanged root objects can retain isolated
stable aliases without rereading their contents. Public artifacts remain detached
copies, while dependencies and build scripts do not inherit the root-link
environment. For target artifacts the macOS arm64 wrapper also enables
`-Zpreserve-duplicate-constants=yes`, preventing LLVM builds from coalescing
distinct string constants into a layout change that cannot be incrementally
patched. This intentionally trades some target object size for stable link
layout. Linux incremental-link behavior is unchanged. Set `SLD_INCREMENTAL=0`
for a full-link comparison or when diagnosing incremental-link behavior, set
`SRS_PRESERVE_DUPLICATE_LLVM_CONSTANTS=0` to compare LLVM without
compiler-side preservation, or set
`SRS_TARGET_CODEGEN_BACKEND=cranelift` for an explicit Cranelift composition
run.

The private-output mode assumes root executable links resolve to `sld`; the
installed toolchain default satisfies that contract. Cargo does not verify
linker identity. When deliberately selecting another linker through Cargo
configuration, environment rustflags, or `cargo rustc` arguments, set
`SLD_INCREMENTAL=0` so the wrapper does not enable private-output preservation.

The wrapper also enables Cargo's verified ordinary-library artifact cache at
`${CARGO_HOME:-$HOME/.cargo}/srs-artifact-cache-v2`. Dependencies compiled from
a shared source location, such as registry packages, can be restored by
hardlink across SRS worktrees on macOS and Linux; separate local checkouts
remain distinct when their source location can affect output. Restoration
copies automatically when the cache and build directory are on different
filesystems.

The wrapper's macOS duplicate-constant preservation flag and SLD
provenance-bearing dependency rlibs participate in cache keys. Root
executables remain outside the shared cache.

Cargo detaches restored hardlinks before rebuilding them, but tools outside
Cargo must not overwrite restored `.rlib` or `.rmeta` files in place: in
hardlink mode, those files share storage with the central cache. Use copy
materialization when a workflow mutates build artifacts after compilation.
Changing materialization mode or disabling the cache does not eagerly detach
already-fresh outputs. Run one Cargo build with the new setting or clean the
target directory before allowing an external tool to mutate artifacts
previously restored by hardlink.

See [Shared Cargo artifact cache](context/shared-cargo-artifact-cache.md) for
the cache admission rules, compiler identity model, concurrency behavior, and
operational details.

Set `SRS_CARGO_CHECKSUM_FRESHNESS=0` to restore Cargo's default mtime-based
source freshness while investigating a build.

Set `SRS_CARGO_ARTIFACT_CACHE=0` to disable shared artifact restoration and
publication.
Set `SRS_CARGO_ARTIFACT_CACHE_DIR` to choose a different central cache root or
`SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION=copy` to retain cache reuse without
hardlink materialization. The cache is unbounded by default; set
`SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE`, for example to `100GiB`, when a logical
byte cap is appropriate. `cargo clean` leaves the central cache intact. The
dedicated cache documentation describes the remaining controls, precedence
rules, manual reclamation, and trust assumptions.

SRS also builds and installs Clippy. By default, its `--fix` mode checks the
same dependencies as a plain Clippy run, so workspace-member artifacts remain
reusable when applying suggestions that Clippy just reported:

```bash
cargo +srs clippy --fix
```

When a plain Clippy run uses `--no-deps` to lint only the selected crate, repeat
`--no-deps` explicitly when applying its suggestions:

```bash
cargo +srs clippy --no-deps
cargo +srs clippy --fix --no-deps
```

`with-sld.sh` is useful when the `sld` choice needs to be explicit, such as
testing a non-installed linker binary or composing the linker with another
toolchain:

```bash
./with-sld.sh cargo +srs build
```

The wrapper sets the `RUSTFLAGS` form used by sld's own macOS Rust workflows:
`-C linker=<sld> -C link-arg=-flavor -C link-arg=darwin`. By default it uses
the `sld` binary from `./build-sld.sh` at `target/sld/opt/sld`; set
`SRS_SLD_PROFILE` to select another Cargo profile or `SRS_SLD_BIN` to test
another binary. Normal `cargo +srs` commands do not need the wrapper after
`./install.sh`.

Use a separate rustup toolchain name when keeping multiple SRS builds linked:

```bash
./install.sh srs-dev
cargo +srs-dev build
```

Installed names are immutable snapshots. After rebuilding, refresh an existing
name by explicitly replacing its snapshot:

```bash
SRS_INSTALL_REPLACE=1 ./install.sh srs-dev
```

Set `SRS_INSTALL_ROOT=/path/to/snapshots` to store the snapshots elsewhere.

The installer also accepts an explicit stage 2 sysroot and Cargo binary:

```bash
./install.sh srs-dev /path/to/stage2 /path/to/cargo
```

Set `SRS_SLD_BIN=/path/to/sld` when installing an alternate `sld` binary.

## Development

All downstream source changes are ordinary SRS changes. Create one SRS
worktree per task, edit `rust/`, `cranelift/`, `sld/`, or
`rust/src/tools/{cargo,clippy}/` directly, and commit the resulting files in
SRS. The SRS branch is the only branch needed for downstream development.

Read [`context/how-to-use-worktrees.md`](context/how-to-use-worktrees.md) before
starting concurrent work.

Refresh imported source trees through the reviewable upstream-update script:

```bash
before="$(git rev-parse HEAD)"
./scripts/pull-upstream.sh rust
git log --oneline "$before"..HEAD
git diff --stat "$before"..HEAD
git diff "$before"..HEAD
```

Pass one of `rust`, `cargo`, `cranelift`, or `sld`, plus an optional upstream
ref. The script creates reviewable import and metadata commits. Review those
commits and run appropriate validation before sharing them.
After pulling an SRS change that advances a nested external dependency pin,
initialize or refresh those external dependencies before rebuilding:

```bash
git submodule update --init --recursive
```

Never push branches or open pull requests unless the user explicitly asks for
that public action.
