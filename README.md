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
- `install.sh`: links the built stage 2 toolchain into rustup under a custom
  name and attaches SRS Cargo plus `sld` to that linked sysroot.
- `cargo-srs.sh`: the installed Cargo wrapper that keeps build scripts, proc
  macros, and their host-side dependencies on LLVM.
- `with-sld.sh`: runs a command with the macOS Rust flags needed to link
  through SRS's built `sld` binary.

The bootstrap config keeps LLVM first in `rust.codegen-backends` and forces the
bootstrap build back through LLVM. The Rust source tree makes the installed
macOS arm64 and Linux x86_64 compilers prefer Cranelift for normal SRS target
artifacts. The installed Cargo wrapper keeps build scripts, proc macros, and
their host-side dependencies on LLVM because those helpers run during the
build and can exercise host intrinsics that Cranelift does not support yet.
LLVM stays available in the toolchain for explicit overrides. For macOS arm64,
SRS bakes `sld` in as rustc's default linker.

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

`./build.sh` is the slow step. Re-run it after changing Rust, Cargo, Clippy,
cg_clif, Cranelift, or the linker default. It builds `sld` with `stable` first so the
installer can attach the built binary. Rust bootstrap stays on the system
compiler driver while the installed SRS compiler defaults to `sld`; set
`SRS_SLD_BOOTSTRAP_TOOLCHAIN` to choose another existing rustup toolchain for
that step. `./install.sh` relinks the resulting stage 2 sysroot; it does not
copy the toolchain.

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

The SRS toolchain uses Cranelift by default for target artifacts and still has
LLVM available. Its Cargo wrapper uses LLVM for host build helpers. On macOS
arm64 it links through the `sld` binary attached to the installed toolchain:

```bash
cargo +srs build
```

The installed Cargo wrapper sets `SLD_INCREMENTAL=1` by default so `sld` can
reuse link state across development builds. It also enables Cargo's verified
ordinary-library artifact cache at
`${CARGO_HOME:-$HOME/.cargo}/srs-artifact-cache-v2`. Dependencies compiled from
a shared source location, such as registry packages, can be restored by
hardlink across SRS worktrees on macOS and Linux; separate local checkouts
remain distinct when their source location can affect output. Restoration
copies automatically when the cache and build directory are on different
filesystems.

Wrapped `rustc` invocations, explicitly configured or otherwise unmodeled
compiler dispatch, self-profile builds, rustc tracing runs,
`RUSTC_BOOTSTRAP` builds, forced rustc version identity overrides, builds
using profile-guided compiler inputs, arbitrary LLVM backend arguments,
external codegen backend libraries, custom target specifications or search
paths, explicit sysroot overrides, unmodeled dynamic loader overrides, and
unmodeled dependency search paths execute normally without artifact
restoration. Rustc `-Z` options do as well unless they select a named sysroot
codegen backend, because those flows can change output or request side effects
outside Cargo's cache key. Windows GNU-family targets also run without
restoration because raw-dylib compilation can invoke unmodeled `dlltool`
programs.

Sysroot compiler and target library file identity, named sysroot codegen
backend contents including SRS's Cranelift backend, and compiler-visible
dynamic library search inputs participate in each cache key. Linux runs with
nonempty `GLIBC_TUNABLES` or nested shared objects in compiler loader roots,
including glibc hardware-capability candidates in configured or installed
compiler loader roots, execute without restoration because they can change
the selected compiler library.

Sysroot library identity assumes ordinary toolchain publication updates file
identity metadata. Overwriting one in place while preserving identity, size,
and modification time, or mutating installed toolchain files in place during
an active Cargo invocation, is outside the cache model. For ordinary
publication that replaces watched installed files or directory trees, file
and directory identity metadata are included in the cache identity and
rechecked before artifact restoration and publication.

Cargo detaches restored hardlinks before rebuilding them, but tools outside
Cargo must not overwrite restored `.rlib` or `.rmeta` files in place: in
hardlink mode, those files share storage with the central cache. Use copy
materialization when a workflow mutates build artifacts after compilation.
The cache is capped at 10 GB by default; completed entries are evicted
oldest-first, entries larger than the configured limit are not published, and
aborted publications are cleaned during later cache publication activity.
Concurrent restores use shared cache locks; publication and eviction use an
exclusive cache lock.

Set `SLD_INCREMENTAL=0` for a full-link comparison or when diagnosing
incremental-link behavior.

Set `SRS_CARGO_ARTIFACT_CACHE=0` to disable shared artifact restoration.
Set `SRS_CARGO_ARTIFACT_CACHE_DIR` to choose a different central cache root or
`SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION=copy` to retain cache reuse without
hardlink materialization. Set `SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE` to a
human-readable cache limit such as `2GiB`. Choose a cache root writable only
by trusted build processes; integrity checks are not authentication for
artifacts supplied by another writer.

`with-sld.sh` is useful when the `sld` choice needs to be explicit, such as
testing a non-installed linker binary or composing the linker with another
toolchain:

```bash
./with-sld.sh cargo +srs build
```

The wrapper sets the `RUSTFLAGS` form used by sld's own macOS Rust workflows:
`-C linker=<sld> -C link-arg=-flavor -C link-arg=darwin`. By default it uses
the `sld` binary from `./build-sld.sh` at `target/sld/opt/sld`; set
`SRS_SLD_BIN` to test another binary. Normal `cargo +srs` commands do not need
the wrapper after `./install.sh`.

Use a separate rustup toolchain name when keeping multiple SRS builds linked:

```bash
./install.sh srs-dev
cargo +srs-dev build
```

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
