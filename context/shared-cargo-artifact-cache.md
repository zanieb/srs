# Shared Cargo Artifact Cache

## Purpose

SRS developers commonly build from multiple worktrees. Each worktree should
keep its own writable target directory, but independently compiling and storing
the same dependencies wastes both build time and disk space.

The installed SRS Cargo wrapper enables Cargo's verified ordinary-library
artifact cache at:

```text
${CARGO_HOME:-$HOME/.cargo}/srs-artifact-cache-v2
```

Dependencies compiled from a shared source location, such as registry
packages, can be restored across SRS worktrees. Separate local checkouts remain
distinct when their source location can affect output.

## Materialization

On macOS and Linux, cached artifacts are restored by hardlink by default.
Hardlink materialization avoids allocating a second copy of identical `.rlib`
and `.rmeta` files. Restoration automatically falls back to copying when the
cache and build directory are on different filesystems.

Cargo detaches restored hardlinks before rebuilding them, including when the
cache feature is later disabled. Tools outside Cargo must not overwrite
restored `.rlib` or `.rmeta` files in place: in hardlink mode, those files
share storage with the central cache. Use copy materialization for workflows
that mutate build artifacts after compilation. Changing materialization mode or
disabling the cache does not eagerly detach already-fresh outputs. Run one Cargo
build with the new setting or clean the target directory before allowing an
external tool to mutate artifacts previously restored by hardlink.

## Cache Admission

The cache is deliberately limited to verified ordinary-library artifacts.
Builds that use unmodeled inputs execute normally without artifact restoration.

Restoration is skipped for inputs including:

- wrapped `rustc` invocations
- explicitly configured or otherwise unmodeled compiler dispatch
- self-profile builds and rustc tracing runs
- `RUSTC_BOOTSTRAP` builds
- forced rustc version identity overrides
- profile-guided compiler inputs
- host CPU auto-detection such as `-Ctarget-cpu=native`
- arbitrary LLVM backend arguments
- runtime Cranelift or GCC backend controls
- external codegen backend libraries
- custom target specifications or search paths
- explicit sysroot overrides
- unmodeled dynamic loader overrides
- unmodeled dependency search paths
- inherited SLD work-product provenance controls
- rustc `-Z` options other than selecting a cache-safe named sysroot codegen
  backend, setting `-Zpreserve-duplicate-constants={yes,no}`, or Cargo's
  checksum-freshness hash algorithm
- Windows GNU-family targets, where raw-dylib compilation can invoke unmodeled
  `dlltool` programs

These exclusions cover configurations that can change output or request side
effects outside Cargo's cache key.

Source-input publication assumes ordinary filesystem writes update modification
times. Mutating a source while Cargo is compiling it and deliberately preserving
an older modification time is outside the cache model.

## Compiler Identity

Each cache key includes the BLAKE3 contents and relative paths of the sysroot
compiler and target library files,
named sysroot codegen backend contents including SRS's Cranelift backend, and
compiler-visible dynamic library search inputs. Compiler and Cargo executable
locations and extraction-assigned device and inode numbers are excluded from the
persistent identity so an otherwise identical toolchain snapshot can reuse
entries after restoration on another runner.

For an ordinary library action that passes cache admission, Cargo removes its
injected target `deps` directory from the action's actual dynamic loader search
path. Admitted actions have no target-local dynamic extern and do not invoke the
linker, so unrelated proc-macro dylibs cannot affect that rustc process. Other
configured and inherited loader roots remain modeled in the cache key.

Linux runs with nonempty `GLIBC_TUNABLES` or nested shared objects in compiler
loader roots execute normally without restoration. This includes glibc
hardware-capability candidates in configured or installed compiler loader
roots, because they can change the selected compiler library.

Sysroot content changes completed before Cargo starts are detected even when
file sizes and modification times are preserved. Device, inode, size,
modification-time, and Unix change-time metadata remain part of a process-local
witness and detect ordinary completed replacement of watched installed files or
directory trees after identity calculation. Concurrent mutation or publication
of a toolchain during an active Cargo invocation remains outside the cache model.

## Capacity And Concurrency

The cache is unbounded by default. When a size limit is configured, completed
entries are evicted oldest-first and entries larger than the configured limit
are not published. Aborted publications are cleaned during later cache
publication activity. The configured cap counts logical bytes reachable under
completed entries; it is not a bound on physical blocks or a guarantee that
eviction immediately reclaims storage while target-directory hardlinks remain.
`cargo clean` removes target-directory links but leaves the central cache
intact. To reclaim central-cache storage manually, remove the cache root only
while no Cargo process is using it.

Concurrent restores use shared cache locks. Publishers hold a bounded striped
action lease while staging and hashing outputs, then use a short exclusive
cache-lock section for final validation, atomic commit, cleanup, size
accounting, and eviction. Cargo coordinates these sections within one process
and waits up to five seconds for short cross-process cache-lock contention.
When the filesystem does not provide reliable locking, or contention outlives
that bound, Cargo executes normally without restoring or publishing the
affected shared artifact.

Cache descendants and modeled generated-input trees are traversed without
following symlinks. A symlinked or otherwise unsupported tree falls back to an
ordinary compilation rather than participating in shared restoration.

## Configuration

Set `SRS_CARGO_ARTIFACT_CACHE=0` to disable shared artifact restoration and
publication.

Set `SRS_CARGO_ARTIFACT_CACHE_DIR` to choose a different central cache root.
The wrapper exports its per-user default directory, so this alias or the
lower-level environment setting should be used to override that default for
`cargo +srs`; a TOML `artifact-cache-dir` setting has lower precedence.

Set `SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION=copy` to retain cache reuse
without hardlink materialization. When no alias or lower-level environment
setting is present, Cargo's normal `hardlink` default applies and a TOML
`artifact-cache-materialization` setting remains effective.

Set `SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE` to a human-readable cache limit such as
`100GiB`. The cache is unbounded when neither this alias, the lower-level
environment setting, nor a TOML `artifact-cache-max-size` setting is present.

The lower-level `CARGO_BUILD_ARTIFACT_CACHE_DIR`,
`CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION`, and
`CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE` settings take precedence over their
`SRS_` aliases.

Choose a cache root writable only by trusted build processes. Entry hashes
reject accidental corruption but do not authenticate artifacts supplied by
another writer with access to that directory.

Set `CARGO_LOG=cargo::core::compiler=debug` when diagnosing cache admission,
restoration, or publication. When switching an existing target directory from
hardlink to copy mode or disabling the cache, run one Cargo build with the new
setting or clean that target directory before allowing external tools to mutate
artifacts previously restored by hardlink.
