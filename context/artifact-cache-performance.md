# Cargo Artifact Cache Performance

## Scope

This investigation used uv as a pinned representative workload, not uv CI as
the test harness. Measurements ran locally on an Apple silicon macOS runner
with:

- SRS `2026.06.11` at `4a8cc277e`;
- uv PR 19754's merge revision `f74311c15`;
- LLVM target codegen;
- `SLD_INCREMENTAL=0` for the full root-build comparison; and
- `CARGO_INCREMENTAL=0` for the full root-build comparison.

Queue time, cache-service network transfer, and benchmark runtime were not
included. The results diagnose the cache layers; they are not a reproduction
of uv's nine CI jobs or a claim that the aggregate acceptance target has been
met.

## Results

### Small controlled graph

`cargo build -p uv-normalize` exercised 30 compiler and build-script commands.

| State | Wall time | Artifact hits | Executed commands |
| --- | ---: | ---: | ---: |
| Empty target, cache disabled | 4.97s | 0 | 30 |
| Empty target, cold artifact cache | 4.48s | 0 | 30 |
| Empty target, warm artifact cache | 3.73s | 5 | 25 |
| Warm target, cache disabled | 0.16s | n/a | 0 |
| Warm target and warm artifact cache | 0.13s | n/a | 0 |

The warm artifact cache hit only 5 of 30 commands. The same warm cache hit 2
units during `cargo clippy -p uv-normalize --all-targets` and 5 during
`cargo test -p uv-normalize --no-run`. A target snapshot restored at a
timestamp-preserving path made the whole graph Cargo-fresh.

The graph's target directory contained about 90 MB of logical data. Its zstd
archive was 25 MB. Local archive extraction plus an all-fresh Cargo invocation
took 2.17s, compared with 3.73s for per-unit artifact replay. The fingerprint
directories themselves were only 20 KB and Cargo dep-info was 182 KB, which is
why a smaller portable freshness layer remains attractive.

After moving publication staging outside the global cache lock and making
short cache-lock contention wait with in-process coordination, the same graph
published all 9 eligible units instead of publishing 7 and losing 2 to lock
contention. A fresh target then hit 7 of those 9 units instead of 5. The two
remaining misses were `uv-small-str` and `uv-normalize`; both are downstream of
uncached generated or proc-macro inputs whose bytes still vary by target path.
The patched debug Cargo took 8.71s cold and 7.09s warm, so those wall times
should not be compared directly with the installed release-Cargo rows above.

### Full uv root build

The representative command was:

```bash
cargo build --profile no-debug --bin uv --bin uvx --locked
```

| State | Wall time | Artifact hits | Executed commands |
| --- | ---: | ---: | ---: |
| Empty target, cold artifact cache | 116.06s | 0 | 618 |
| Empty target, warm artifact cache | 114.21s | 29 | 589 |
| Same-path warm target snapshot | 1.10s | n/a | 0 |

The warm cache hit 29 of 618 units, or 4.7%, and improved wall time by only
1.85s. The cold writer published 103 entries, rejected 95 publications after
its compiler loader inputs changed, and skipped 175 publications because the
global cache lock was contended. The resulting artifact cache contained 181 MB
while the target directory contained 1.68 GB.

The complete target compressed to 393 MB. Creating the local zstd archive took
2.85s and extracting it at the same absolute target path took 2.92s. Extraction
plus the all-fresh Cargo invocation therefore took 4.02s before network
transfer. Restoring the same snapshot at a different absolute target path took
29.39s and rebuilt 17 units: `uv-python` records an `OUT_DIR` file in a
`rerun-if-changed` directive, and that path change propagated through its
dependents. Workload snapshots therefore need a stable target path or explicit
build-script path relocation.

### Publication and restore locking fix

A clean paired rerun with the patched debug Cargo used the same uv revision,
LLVM settings, command, and empty target directories:

| State | Wall time | Artifact hits | Stored entries | rustc execution spans |
| --- | ---: | ---: | ---: | ---: |
| Empty target, cold repaired cache | 85.71s | 0 | 370 | 478 |
| Empty target, warm repaired cache | 83.81s | 286 | 93 new variants | 192 |

The cold writer had no publication lock timeout and no input-race rejection.
This is the direct comparison with the original cold writer's 103 stores, 175
lock-contention skips, and 95 loader-input race rejections. The warm reader also
had no cache-lock timeout. Before restore locking was repaired, an intermediate
run published 369 entries but abandoned 220 restores when those publications
briefly held the global write lock; it produced only 75 hits. Bounded shared
lock waits raised that to 286 hits.

The approximate eligible-unit hit rate in the final warm run was
286 / (286 + 93), or 75.5%, pending the structured counters needed to classify
every miss exactly. Avoiding 286 rustc executions reduced user CPU time from
423.88s to 368.55s and system CPU time from 92.91s to 83.93s, but reduced wall
time by only 1.90s. This separates cache availability from cache usefulness:
the repaired artifact layer now reuses most portable eligible actions, while
per-unit input hashing, verification, replay, and the uncached build-script,
proc-macro, and final-link critical path consume nearly all of the saved
parallel rustc work.

### Compiler identity

The first compiler identity calculation hashed 56 files and 332,101,740 bytes.
Ten isolated warm-filesystem measurements put the content-hashing portion at
160-169ms wall time with nearly the same thread CPU time. The identity is held
in a shared `OnceLock` and reused by all eligible units in the Cargo process.

This cost is worth reporting, but it does not explain a material part of the
114-second warm build. Shipping a digest alone would also weaken mutation
detection for files changed after extraction but before Cargo starts. Safe
cross-process reuse requires an installer-verified digest bound to an
extraction-specific metadata witness, or a trusted immutable filesystem
measurement.

## Why The Current Cache Misses

### Volatile target directories contaminate compiler identity

Cargo adds the target `deps` directory to rustc's dynamic loader path. The
artifact cache hashed every dynamic library in that directory for every
otherwise eligible library action. Proc-macro dylibs appear while the graph is
running and differ across target paths, so unrelated action keys change and
publication frequently rejects the action as raced.

An ordinary cache-admitted `--crate-type lib` action cannot contain a dynamic
extern and does not invoke the linker. Removing Cargo's injected target `deps`
path from that action's actual loader environment is both narrower and cheaper
than hashing unrelated proc macros. The focused fix in this worktree does that
only after runtime admission has accepted the action.

### Publication and restore used opportunistic global locking

The original publication path performed a nonblocking global cache-lock attempt
before copying and hashing outputs. Parallel rustc completion discarded many
useful cold-run publications. Once staging moved outside that lock, publication
succeeded, but nonblocking restore attempts then lost 220 available hits to the
short commit sections.

The repaired path uses bounded striped action leases, stages and hashes outputs
outside the global lock, serializes commit sections within one Cargo process,
and reserves the global lock for final validation, atomic rename, size
accounting, cleanup, and eviction. Restores use coordinated shared locks and
bounded waits rather than falling back to rustc during those short commits.
Active staging directories no longer dirty completed-entry size state, and the
bounded stripe files live at the cache root so empty action directories remain
prunable.

### Uncached units poison downstream content keys

The cache rejects proc macros, build scripts, test binaries, Check actions,
Clippy wrapper actions, and every package that has a custom build script. A
consumer's action key hashes raw dependency artifacts. When an uncached
dependency embeds an absolute `OUT_DIR` or target path, its bytes differ on the
next runner and every downstream consumer misses even if its own source and
arguments are unchanged.

For example, serde's rmeta differed by only 50 bytes between two controlled
targets; those bytes included its generated `OUT_DIR/private.rs` path. That was
enough to make the otherwise identical `uv-small-str` action miss repeatedly.

### A hit still reconstructs Cargo state one unit at a time

For each hit Cargo currently:

- hashes direct extern and generated action inputs repeatedly;
- verifies source inputs from dep-info;
- verifies cache control and artifact files;
- materializes outputs;
- translates dep-info;
- replays compiler messages; and
- writes target-local fingerprint state.

Those checks preserve correctness, but the work remains scheduled as a dirty
unit. A portable freshness layer can remove scheduling and replay overhead only
if it verifies the artifact first, recreates current dep-info and stamps, then
computes and commits the current Cargo fingerprint before normal scheduling.
Producer fingerprint files must not be trusted as an independent cache
authority.

## Recommended Layering

### Immediate workload layer

Use selective target snapshots for workloads whose expensive outputs are not
covered by the artifact cache:

- the Linux and macOS no-debug root builds;
- Clippy's target profile;
- Linux and macOS nextest fast-build profiles;
- generated-file tools;
- profiling and benchmark builds.

Keep keys separated by OS, architecture, toolchain, job, profile, command
shape, lock/config inputs, and source revision. Restore to the same absolute
target path, retain workload isolation, and designate one writer per key.
Snapshots are the fastest current control, not a substitute for repairing the
portable product layer.

SLD link state belongs with a workload target snapshot for incremental edits.
It does not help a fresh target until the root binary and Cargo fingerprints
have also been restored.

### Portable SRS product layer

Implement in measured order:

1. Keep compiler loader inputs stable for admitted ordinary libraries.
2. Keep the repaired cold publication and restore-lock behavior covered by
   concurrency, killed-publisher, size-accounting, and mutation regressions.
3. Add opt-in structured counters and cumulative phase timing so hit rates,
   skipped publications, hashed bytes, and replay cost are visible without
   parsing debug logs.
4. Cache metadata-only rustc Check actions. Use copy materialization or avoid
   changing the mtime of a restored hardlinked rmeta.
5. Add a composite identity for Clippy that covers the ordered wrapper chain,
   `clippy-driver`, rustc, and the sysroot.
6. Admit ordinary library actions from packages with build scripts only after
   generated trees and embedded build paths have a stable, verified portable
   representation.
7. Prewarm verified pure-library chains before Cargo scheduling and recompute
   current fingerprints rather than copying producer fingerprints.
8. Generalize linked-action identity and output manifests before caching proc
   macros, build-script executables, test binaries, debug sidecars, and final
   links.

Build-script execution state is a separate capsule design. It includes parsed
stdout, watched environment and paths, the complete `OUT_DIR` tree, and mtime
ordering. It should remain in workload snapshots until Cargo has a declared or
hermetic output contract.

### Instrumentation contract

An opt-in summary should report at least:

- Cargo-fresh units;
- eligible dirty units, hits, misses, and key failures;
- ineligible units by reason;
- publication success, rejection, failure, and lock contention;
- restored and published files/logical bytes;
- hardlink, copy, and cross-device fallback counts;
- compiler identity files/bytes, one-time wall/CPU time, and reuse count;
- action-input hash calls/files/bytes/time;
- cache verification, materialization, and publication cumulative worker time;
- rustc executions and final link executions.

Phase totals are cumulative worker time and may exceed wall time when jobs run
in parallel. The summary must remain off by default.

## Integration Gate

Do not use another broad uv CI experiment as the next diagnostic step. First
prove locally that:

- a cold parallel writer publishes nearly every eligible unit;
- stable pure-library graphs have near-complete cross-target hits;
- Clippy and test dependency hit rates are reported rather than inferred;
- target-path and generated-input portability tests stay green; and
- replay time is materially below the avoided rustc time.

After those gates, uv PR 19754 can combine the repaired artifact layer with
job-specific target snapshots and compare equivalent runner classes. The
aggregate CI acceptance target still requires that final integration rerun.
