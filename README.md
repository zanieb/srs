# SRS

SRS is the slop Rust stack: a forked Rust toolchain that carries the patched
Cranelift backend needed to compile Astral workloads on macOS arm64.

## Layout

- `repos/rust/`: the Rust fork that builds the toolchain and owns the
  `compiler/rustc_codegen_cranelift` integration.
- `repos/cranelift/`: the patched Wasmtime/Cranelift checkout consumed by the
  backend in the Rust fork.
- `repos/sld/`: the pinned `zanieb/sld` linker checkout.
- `bootstrap.toml`: Rust bootstrap configuration for the SRS toolchain build.
- `build.sh`: builds `sld`, then a stage 2 Rust toolchain with LLVM and
  Cranelift backends, plus Cargo from the Rust tree's Cargo submodule.
- `build-sld.sh`: builds the `sld` binary, using the linked SRS toolchain by
  default when called directly.
- `scripts/build-apple-containers.sh`: builds the Linux x86_64 SRS lane from
  macOS through Apple containers.
- `install.sh`: links the built stage 2 toolchain into rustup under a custom
  name and attaches SRS Cargo plus `sld` to that linked sysroot.
- `cargo-srs.sh`: the installed Cargo wrapper that keeps build scripts, proc
  macros, and their host-side dependencies on LLVM.
- `with-sld.sh`: runs a command with the macOS Rust flags needed to link
  through SRS's built `sld` binary.

The bootstrap config keeps LLVM first in `rust.codegen-backends` and forces the
bootstrap build back through LLVM. The Rust fork makes the installed macOS
arm64 and Linux x86_64 compilers prefer Cranelift for normal SRS target
artifacts. The installed Cargo wrapper keeps build scripts, proc macros, and
their host-side dependencies on LLVM because those helpers run during the
build and can exercise host intrinsics that Cranelift does not support yet.
LLVM stays available in the toolchain for explicit overrides. For macOS arm64,
SRS bakes `sld` in as rustc's default linker.

## Quick Start

Initialize the pinned Rust, Cranelift, and sld source trees after cloning SRS:

```bash
git submodule update --init repos/rust repos/cranelift repos/sld
```

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

`./build.sh` is the slow step. Re-run it after changing Rust, cg_clif,
Cranelift, or the linker default. It builds `sld` with `stable` first so the
installer can attach the pinned binary. Rust bootstrap stays on the system
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

Development happens inside each submodule. Commit Rust, Cranelift, and sld
changes in their owning repositories, then stage the updated submodule paths in
SRS to pin the integrated stack revision.

```bash
git -C repos/cranelift commit
git -C repos/rust commit
git -C repos/sld commit
git add repos/cranelift repos/rust repos/sld
git commit
```

After pulling an SRS change that advances either source pin, update the
submodules before rebuilding:

```bash
git submodule update --init repos/rust repos/cranelift repos/sld
```

Fresh clones need the configured submodule remotes to contain the pinned SRS
Rust, Cranelift, and sld commits.
