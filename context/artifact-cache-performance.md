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

The opt-in counters were then measured with an optimized build of the patched
Cargo. Every row used an empty target except the explicitly Cargo-fresh row;
Clippy's cold row reused the central build cache but had no matching
all-features variants.

| Command and state | Wall | Eligible | Hits | Hit rate | Ineligible | rustc executions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Build, cache disabled | 4.37s | 0 | 0 | n/a | 0 | 26 |
| Build, cold cache | 5.44s | 9 | 0 | 0% | 17 | 26 |
| Build, warm cache | 3.85s | 9 | 7 | 77.8% | 17 | 19 |
| Build, same warm target | 0.13s | 0 | 0 | n/a | 0 | 0 |
| Clippy all-targets/all-features, cold variants | 6.39s | 4 | 0 | 0% | 66 | 70 |
| Clippy all-targets/all-features, warm variants | 4.51s | 4 | 4 | 100% | 66 | 66 |
| Test/no-run, warm build cache | 2.55s | 8 | 7 | 87.5% | 18 | 19 |

The warm build restored 31.7 MB of outputs. Its seven hit lookups took 83ms of
cumulative worker time, of which hardlink materialization took 3ms. Action
input hashing took 27ms, and the one compiler identity computation hashed 56
files / 332.1 MB in 196ms wall and 180ms thread CPU. The two misses published
0.76 MB in 46ms. This is materially cheaper than the seven avoided rustc
actions; the remaining 3.85s is dominated by 17 ineligible actions and the two
target-path-contaminated misses.

Warm Clippy restored all four eligible variants, but 66 of 70 rustc actions
remained ineligible: 44 Check-mode actions, 11 proc macros, nine non-library
targets, and two packages with build scripts. The four hit lookups took 937ms
cumulative while physical materialization took 2ms. Warm test/no-run restored
seven of eight eligible units in 90ms cumulative and spent 171ms in its one
link-producing primary rustc action. These counters establish internal hit
rates without claiming that Cargo currently measures linker-only time.

An outer `-Zartifact-cache` did not survive the external `cargo-clippy` hop by
itself: the inner Cargo summary reported `configured:false`. Exporting the same
policy through Cargo's unstable configuration fixed the normal SRS wrapper
path; a real wrapped cold/warm Clippy pair then reported four eligible misses
followed by four hits, with the warm command taking 4.42s.

### Metadata-only Check actions

The next local iteration admitted non-test `--emit=dep-info,metadata` library
actions. Restored Check outputs are copied even when the cache is configured for
hardlinks because Cargo advances their mtimes after a successful check. This
keeps the shared entry immutable while allowing the restored target to become
Cargo-fresh on its next invocation.

All rows below used the same optimized patched Cargo and `uv-normalize` graph.
The Clippy control disabled the artifact cache on the exact current code. Each
warm row used an empty target and entries produced by the corresponding cold
row; the three Clippy warm samples were 7.43s, 5.46s, and 4.87s.

| Command and state | Wall | Eligible | Hits | Hit rate | Ineligible | rustc executions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Clippy, cache disabled | 6.81s | 0 | 0 | n/a | 0 | 70 |
| Clippy, cold Check variants | 8.37s | 25 | 0 | 0% | 45 | 70 |
| Clippy, warm Check variants, median | 5.46s | 25 | 25 | 100% | 45 | 45 |
| Plain Check, cold cache | 7.31s | 27 | 0 | 0% | 43 | 70 |
| Plain Check, warm cache | 4.81s | 27 | 25 | 92.6% | 43 | 45 |

The warm Clippy runs restored 21 Check rmeta files / 11.6 MB by copy and eight
Build outputs / 18.2 MB by hardlink. Physical materialization took only 5-9ms.
Restore-phase counters instead found that a redundant pre-materialization
compiler/action-input validation duplicated the correctness-authoritative
post-materialization validation. Removing the first pass retained the mutation
regressions and reduced the Clippy warm median below the disabled control by
1.35s. Splitting the remaining final validation phase showed 2.984s of one
3.004s total was compiler-loader validation; identity-witness validation used
6ms and action-input validation used 14ms. On macOS the loader root was the same
immutable SRS `lib` directory already hashed by the compiler identity, but
reached through rustup's toolchain symlink. Binding the canonical directory to
the identity witness removed the redundant content scan while retaining content
hashing for external loader roots and conservative recursive validation on
Linux.

The final same-binary, externally timed comparison used three empty targets per
state. Warm Clippy took 5.37s, 3.66s, and 3.85s, for a 3.85s median; disabled
controls took 4.93s, 6.09s, and 5.22s, for a 5.22s median. The warm cache was
therefore 1.37s (26%) faster despite the noisy first warm sample. Final loader
validation fell to 105-122ms cumulative and total final validation to
126-145ms, with the same 25 hits, 45 rustc executions, and one Cargo-fresh
Build unit. The cold
population tax remains visible, so a designated writer should populate shared
entries rather than every reader.

### Portable Cargo-fresh preflight

The next iteration restores cacheable Build-library dependency closures before
Cargo's ordinary fingerprint walk. It never copies a producer fingerprint.
Instead, it verifies the current action and source inputs, materializes the
artifact and dep-info, calculates a fingerprint from the consumer's current
unit graph and filesystem state, and writes the short fingerprint hash last as
the freshness commit marker. A dependency must already be Cargo-fresh or have
completed the same protocol earlier in the preflight. Any miss, incomplete
input closure, failed validation, or interrupted commit falls back to normal
dirty scheduling.

The first implementation deliberately covers only ordinary pure Build-library
units. It excludes packages with build scripts, proc macros, artifact
dependencies, SBOM outputs, SLD native incremental work, and fine-grained
target locking. Check artifacts remain cacheable per unit but do not yet enter
the graph-wide freshness protocol.

The following controlled reruns used the reviewed optimized Cargo, exact
current-code cache-disabled controls, LLVM codegen, empty target directories,
and the same pinned `uv-normalize` graph.

| Command and state | Wall | Hits / eligible | Preflight finalized | Cargo-fresh | rustc executions |
| --- | ---: | ---: | ---: | ---: | ---: |
| Build, cache disabled, median of 3 | 4.81s | 0 / 0 | 0 | 0 | 26 |
| Build, cold cache | 5.08s | 0 / 9 | 0 | 0 | 26 |
| Build, warm cache, median of 3 | 3.07s | 7 / 9 | 4 | 4 | 19 |
| Test/no-run, cache disabled, midpoint of 2 | 4.92s | 0 / 0 | 0 | 0 | 26 |
| Test/no-run, warm cache, midpoint of 2 | 3.15s | 7 / 8 | 4 | 4 | 19 |
| Clippy, cache disabled, median of 3 | 5.22s | 0 / 0 | 0 | 0 | 70 |
| Clippy, cold cache after loader fix | 7.04s | 0 / 25 | 0 | 0 | 70 |
| Clippy, warm cache after loader fix, median of 3 | 3.85s | 25 / 25 | 1 | 1 | 45 |

The warm Build preflight finalized four dependency libraries and removed them
from Cargo's dirty queue, improving the exact disabled median by 1.74s and the
earlier per-unit replay result by 0.78s. Test/no-run improved its disabled
midpoint by 1.77s. After removing the redundant loader scan, Clippy improved
its disabled median by 1.37s, but only one pure Build library became
Cargo-fresh; 24 metadata-only Check hits still ran through dirty replay. Their
total final validation was reduced to 126-145ms cumulative. Build preflight
took 227-250ms and optimized Clippy preflight took 175-205ms. This makes
extending safe graph-wide freshness to Check actions a smaller, separately
measurable scheduling optimization rather than a presumed replay fix.

The reviewed implementation was also rerun on the full root command. Every row
used an empty target; the cold row had no matching entries and the warm row
reused the entries it published.

| State | Wall samples | Hits / eligible | Preflight finalized | Cargo-fresh | rustc executions |
| --- | ---: | ---: | ---: | ---: | ---: |
| Cache disabled | 56.04s | 0 / 0 | 0 | 0 | 562 |
| Cold artifact cache | 59.67s | 0 / 382 | 0 | 0 | 562 |
| Warm artifact cache | 54.40s | 297 / 382 | 179 | 179 | 265 |

The warm run was 1.64s faster than the exact disabled control. It restored 594
files / 341.7 MB, made 179 units Cargo-fresh, and avoided 297 rustc executions.
Preflight took 2.07s wall after limiting fingerprint work to reachable
candidates, while the two remaining primary link actions consumed 6.56s
cumulatively. A single local pair is directional evidence, not a stable
aggregate-speed claim. The remaining 85 eligible misses and 180 ineligible
actions also show why a build-script/nonportable-state layer or thin workload
snapshot is still needed.

### Remaining Clippy critical path

The generic action-rejection counter originally hid two different classes.
The structured summary now reports direct dynamic-library externs and compiler
wrappers separately. A warm scoped Clippy run reported 13 `dynamic_extern`
units, two `compiler_wrapper` units, 11 proc macros, nine build-script
executables, nine package libraries with build scripts, and one test-mode unit.
No units remained in the generic `unmodeled_rustc_action` bucket.

The same run measured 14.40s of cumulative rustc worker time and nine build
script processes using another 6.39s cumulatively. An earlier Cargo timing trace
split the rustc work more precisely:

| Remaining class | Units | Cumulative job time |
| --- | ---: | ---: |
| Proc-macro producers | 11 | 4.73s |
| Metadata consumers with proc-macro dylib externs | 13 | 4.17s |
| Libraries belonging to packages with build scripts | 9 | 1.97s |
| Build-script executables | 9 | 1.12s |
| Actual Clippy wrapper actions | 2 | 0.16s |
| Test-mode Check | 1 | 0.12s |

The nine build-script process executions added 4.54s in that trace. These are
overlapping worker totals rather than additive wall time, but they account for
the remaining work and establish the next order: model proc-macro producers and
their direct consumers, then build-script state and generated trees. A
composite Clippy identity covers only two units in this graph and is not the
primary fix.

### Check preflight and thin-snapshot experiment

An experimental extension admitted metadata-only Check units to the graph-wide
freshness preflight. Focused tests covered copy immutability, two-unit dependency
closure, partial caches, forced execution, cached warning denial, and source
mutation after materialization. It finalized 14 units instead of one, but kept
45 rustc executions. Alternating same-binary controls were neutral: the old
preflight median was 4.43s and the Check-enabled median was 4.39s. The additional
fingerprint work therefore was not retained as a standalone product change.

The experiment was also composed with a same-path thin snapshot. The prototype
omitted exactly the 29 target outputs / 29.86 MB that matched verified portable
entries, rather than filtering arbitrary files by suffix. The full scoped
target compressed to 24 MB and the thin target to 15 MB. Results were:

| Restored layer | Extract | Cargo | End to end | Cargo-fresh | rustc executions |
| --- | ---: | ---: | ---: | ---: | ---: |
| Thin snapshot plus portable cache | 0.21s | 4.28s | 4.49s | 29 | 30 |
| Full same-path target snapshot | 0.26s | 0.52s | 0.78s | 70 | 0 |

The portable cache restored all 25 eligible actions from the thin snapshot,
but its new completion mtimes made retained proc macros and downstream
fingerprints stale. Simply adding Check preflight did not make that archive
Cargo-fresh.

The retained implementation instead emits a Cargo-produced ownership manifest
after a successful population build. It records the exact target-relative file,
verified cache entry and digest, mode, size, and nanosecond mtime for every
cache-owned output. After extracting a thin archive at the same absolute target
path, Cargo verifies the completed cache entries, copies the omitted files back,
and restores their original metadata before parsing build-script state or
calculating any fingerprint. This reconstructs the writer target's dependency
ordering without copying producer fingerprints or mutating cache hardlinks.

The same scoped Clippy graph was rerun three times per layer with an optimized
Cargo. Both archives used PAX metadata and zstd level 3. The manifest certified
29 outputs / 29.86 MB; emitting it used 35.8ms during the population run.

| Layer | Compressed | Extract median | Cargo median | End-to-end median | Cargo-fresh | rustc / build scripts |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Thin plus reconstructed cache outputs | 16.15 MB | 0.17s | 0.22s | 0.39s | 70 | 0 / 0 |
| Full target snapshot | 25.24 MB | 0.20s | 0.16s | 0.35s | 70 | 0 / 0 |

Thin end-to-end samples were 0.44s, 0.38s, and 0.39s. Full samples were
0.35s, 0.40s, and 0.35s. Reconstruction copied the 29 files in 71-76ms. The
thin layer was only 40ms slower at the local median while reducing compressed
bytes by 36.0%; unlike the earlier 4.49s prototype, it made every unit fresh.
The full archive remains a useful control and fallback, while the thin layer is
now viable when transfer size matters.

Archive metadata is part of the correctness protocol. A default tar archive
that rounded retained files to whole-second mtimes reproduced partial
invalidation: only 37 units stayed fresh and 25 rustc actions reran. PAX (or an
equivalent format preserving nanosecond mtimes and modes) is required. Restore
also requires the same artifact-cache policy used by the writer, because the
completion-stamp output contract participates in Cargo freshness.

The full root target confirms the composition at scale. A cold population run
stored all 382 eligible entries and emitted a manifest for 764 outputs / 597.1
MB logical in 1.01s. It took 89.24s and executed 562 rustc actions plus 56
build-script processes; the manifest does not make the writer cheaper. PAX tar
plus zstd level 3 compressed the complete target to 394 MiB and the thin target
to 227 MiB, a 42.4% reduction.

The first retained reconstruction streamed every cache output and redundantly
hashed the already verified source a second time. Its Cargo phase took 3.57s.
The measured follow-up uses a same-filesystem copy-on-write clone when
available, falls back to a byte copy, and retains the post-reconstruction
digest check. Removing only the redundant source digest and adding the clone
path reduced the final Cargo median to 1.89s. All three thin trials cloned 764
files / 597.1 MB, byte-copied none, kept all 562 units Cargo-fresh, and executed
zero rustc actions and zero build scripts.

| Layer | Compressed | Extract samples | Cargo samples | End-to-end median |
| --- | ---: | --- | --- | ---: |
| Thin plus reconstructed cache outputs | 227 MiB | 2.92s, 2.90s, 2.82s | 2.70s, 1.85s, 1.89s | 4.75s |
| Full target snapshot | 394 MiB | 4.95s, 3.70s, 3.71s | 0.94s, 0.52s, 0.48s | 4.22s |

The thin layer is 0.53s slower at the local median but transfers 167 MiB less.
Ignoring request latency, it wins once effective archive transfer bandwidth is
below about 315 MiB/s. Network transfer was not measured, so both layers
remain useful integration controls: full is the fastest local fallback, while
thin avoids paying remote-cache bandwidth for bytes already present in the
portable artifact cache. The retained target still contains uncached library
pairs, final executable copies, `build/` state, and proc-macro dylibs. Cargo
must emit the omission manifest; external matching by extension, basename,
inode, size, or mtime is not a safe product protocol.

A bounded four-worker prototype then staged and verified reconstructed files in
parallel using available Cargo jobserver tokens, with a barrier before the
first target installation. It improved failure atomicity but not speed: the
three restore samples were 1.52s, 1.17s, and 1.68s (1.52s median), versus the
retained serial samples of 1.50s, 1.29s, and 1.33s (1.33s median). Whole Cargo
time regressed from a 1.89s serial median to 3.04s. APFS metadata and read
contention outweighed file-level hash parallelism, so the prototype was not
retained. Grouping the two files per cache entry can still remove duplicate
small control-file reads, but cannot avoid either 597 MB data digest and is not
a priority without contrary measurements on the Linux cache filesystem.

### Generated-file and profiling controls

Two additional commands were measured locally with empty target directories.
The generated-file command was the unchanged
`cargo dev generate-all --mode dry-run`; a scratch `npx` shim invoked an
already installed Prettier package because the machine-global npm cache was not
writable. The profiling control used
`cargo build --profile profiling -p uv-bench --benches --locked`. It exercises
the benchmark Rust graph and profile but does not claim to reproduce
`cargo codspeed build` or benchmark runtime.

| Workload and state | Wall | Hits / eligible | Cargo-fresh | rustc | Build scripts |
| --- | ---: | ---: | ---: | ---: | ---: |
| Generate, cache disabled | 94.14s | 0 / 0 | 0 | 562 | 55 |
| Generate, cold cache | 101.04s | 0 / 385 | 0 | 562 | 55 |
| Generate, warm cross-target cache | 83.40s | 304 / 385 | 186 | 258 | 55 |
| Profiling, cache disabled | 136.21s | 0 / 0 | 0 | 531 | 53 |
| Profiling, cold cache | 142.04s | 0 / 357 | 0 | 531 | 53 |
| Profiling, warm cross-target cache | 122.63s | 279 / 357 | 168 | 252 | 53 |

These are single sequential disabled/cold/warm samples, so the wall-time deltas
are directional rather than stable medians. The warm rows were 10.74s (11.4%)
and 13.58s (10.0%) below their disabled controls; the cold rows were 6.90s and
5.83s above them. Filesystem-cache state, run order, and machine noise are not
isolated. Both warm runs nevertheless restored about 78-79% of eligible
actions, while every build script and the primary link-producing action still
ran. The generated-file warm run recorded 258 rustc executions exactly
matching its 81 eligible misses plus 177 ineligible actions. The profiling warm
run similarly recorded 252 executions, matching 78 misses plus 174 ineligible
actions. This accounts for the ordinary compiler queue rather than leaving an
unexplained cache gap.

Warm generation spent 18.53s of cumulative worker time in lookup and 11.03s
hashing action inputs while its remaining rustc and build-script processes used
237.50s and 182.82s cumulatively. Warm profiling spent 10.32s in lookup and
12.91s hashing action inputs; remaining rustc and build scripts used 519.79s
and 140.20s cumulatively, and the primary compile/link action used 28.16s.
These totals overlap concurrently and structurally: action-input hashing is
inside lookup/publication work, materialization is inside lookup, and the
primary link counter is a subset of rustc. They must not be summed. Compiler
identity remained only 0.23-0.30s wall per Cargo process. The measurements
support a workload snapshot for these graphs and do not support weakening
linked-action identity merely to raise the per-unit hit count.

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

The cache rejects proc macros, build scripts, test binaries, test-mode Check
actions, Clippy wrapper actions, and every package that has a custom build
script. A consumer's action key hashes raw dependency artifacts. When an
uncached dependency embeds an absolute `OUT_DIR` or target path, its bytes
differ on the next runner and every downstream consumer misses even if its own
source and arguments are unchanged.

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

### Selected composition and safety protocol

The best current design is not a larger monolithic archive. It is a stack in
which each layer owns a different portability boundary:

| Layer | Authority | Intended contents | Fallback |
| --- | --- | --- | --- |
| Portable artifact cache | Content-verified rustc action | Ordinary library outputs, dep-info, messages | Execute rustc |
| Portable freshness capsule | Current Cargo process | Restored outputs plus a newly calculated current fingerprint | Normal dirty scheduling |
| Thin workload snapshot | Workload/path domain | Build scripts, proc macros, final binaries, `OUT_DIR`, messages and target-local state not covered above | Full workload build |
| Full target snapshot | Exact target-path domain | Complete profile directory; control and emergency fast path | Full workload build |
| SLD state | Exact linked action and root output | Private linker state paired with the root binary | Full relink |
| Release compiler identity | Verified SRS installer | Preverified relative content digest plus extraction witness | Hash the installed toolchain |

Portable freshness now runs as a graph-wide preflight before the first normal
fingerprint calculation. Restoring a unit after Cargo has recursively memoized
dependent fingerprints is too late. The implementation factors the shared
cache action description out of `rustc()`, walks the unit graph in dependency
order, and admits a unit only when every fingerprint dependency is already
Cargo-fresh or has been successfully restored in the same preflight closure.

For each admitted unit the preflight must:

1. Verify the current compiler, action, source, generated input, cache-control,
   and stored-output digests.
2. Stage the outputs, translated rustc dep-info, compiler messages, and
   completion stamp under the target lock.
3. Give the closure a dependency-safe current preflight timestamp rather than
   trusting producer mtimes.
4. Calculate the fingerprint from the current `Unit`, dependency
   fingerprints, flags, environment, and verified filesystem state.
5. Commit the current fingerprint last as the freshness marker.

The portable record is a versioned restoration capsule: it references the
verified artifact action, output manifest and digests, rustc dep-info and its
origin roots, compiler messages, and completion-stamp kind. A producer's Cargo
fingerprint is never cache authority and is not copied into place. A crash,
lock timeout, malformed record, partial closure, or mutation before the final
fingerprint commit can only leave the unit dirty. Restored hardlinks remain
immutable and are detached before any compiler, linker, or external tool can
mutate them.

The implementation first performs a cheap candidate scan so unrelated roots do
not pay a duplicate fingerprint walk. Executor-forced units bypass both
preflight restoration and the ordinary runtime cache path. A cold preflight miss
also records a canonical command/environment digest; dirty execution reuses the
prepared action only if the runtime command still matches, otherwise it compiles
without cache publication. Focused regressions cover forced rebuilds, command
drift, and failure after detailed fingerprint JSON is written but before the
short-hash commit marker.

The current protocol covers pure Build-library units. Non-test metadata-only
Check actions are cacheable but still use per-unit dirty replay until their
mtime-sensitive target state can safely be finalized in the graph-wide
preflight. Clippy-wrapped actions require a separate composite action identity
covering wrapper order and arguments,
`clippy-driver`, rustc/sysroot, dynamic loader inputs, lint levels, relevant
environment, and `.clippy.toml`/configuration inputs, with diagnostic replay.
Packages with build scripts remain outside the portable closure until a capsule
can bind the build-script executable, observed environment, parsed stdout
directives, watched inputs, native search inputs, and the complete portable
`OUT_DIR` tree.

A thin snapshot should then be compared with the full profile snapshot. It
should omit `.rlib`/`.rmeta` bytes supplied by the portable layers and contain
only nonportable outputs and their target-local state: build-script output and
`OUT_DIR`, proc macros and build-script executables, final test/root binaries,
their dep-info/messages/fingerprints, and optional SLD state. Snapshot keys
must isolate OS, architecture, SRS release and schema, backend, linker, command
shape, profile, Cargo configuration, source revision, runner/path domain, and
SLD schema. Only one trusted writer publishes each domain.

SLD remains orthogonal to fresh-target acceleration. Its capsule must
atomically bind the private root executable and linker state to the linker
identity, full arguments, ordered object/archive/dylib/rlib provenance, and SLD
environment. Any mismatch forces a full relink. Measure it on an incremental
source edit after restoring the target layer; do not credit it for a fresh
target unless the root binary and Cargo freshness state were restored too.

A precomputed compiler digest is safe only when an SRS release manifest binds
relative paths and contents under the already verified archive checksum or a
signature, and installation creates an extraction-specific metadata/directory
witness. Cargo must validate that witness and fall back to content hashing on
any mismatch. At roughly 0.2s in optimized Cargo this is a later optimization,
not the primary performance fix.

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

For uv PR 19754, integrate the layers without changing any workload command:

1. Give each workload a stable absolute target path and a key covering the uv
   revision, lock/config inputs, SRS/cache schema, OS/architecture, backend,
   linker, profile, features, and command shape.
2. Let one designated writer restore the portable cache, run the original
   command with `SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_MANIFEST`, and publish the
   portable cache plus a PAX thin target archive that omits exactly the
   manifest-owned paths.
3. Let readers restore both layers, extract the thin archive at the same target
   path, and set `SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_RESTORE_MANIFEST` for the
   unchanged Cargo command. A failed explicit reconstruction discards that
   target and falls back to the full snapshot or ordinary build.
4. Retain separate keys and single-writer lanes for root build, Clippy,
   nextest, generated files, and profiling. Pair the root/profiling snapshot
   with SLD state only when measuring incremental edits.
5. Upload the structured JSON line with job timing. Gate rollout on zero
   restore failures, zero unexpected rustc/build-script executions after a
   snapshot hit, and an end-to-end median win including archive transfer.

The local full-root result supplies a useful policy boundary: prefer thin when
effective archive transfer is below roughly 315 MiB/s; otherwise a full target
snapshot remains the faster fallback. Re-measure that threshold on the actual
cache service rather than encoding the local number permanently.

### Portable SRS product layer

Implement in measured order:

1. Keep compiler loader inputs stable for admitted ordinary libraries.
2. Keep the repaired cold publication and restore-lock behavior covered by
   concurrency, killed-publisher, size-accounting, and mutation regressions.
3. Add opt-in structured counters and cumulative phase timing so hit rates,
   skipped publications, hashed bytes, and replay cost are visible without
   parsing debug logs.
4. Keep non-test metadata-only rustc Check actions copy-materialized and
   covered by cache-immutability, source-mutation, and next-run freshness tests.
5. Add a composite identity for Clippy that covers the ordered wrapper chain,
   `clippy-driver`, rustc, and the sysroot.
6. Admit ordinary library actions from packages with build scripts only after
   generated trees and embedded build paths have a stable, verified portable
   representation.
7. Keep verified pure-Build-library prewarming before Cargo scheduling,
   recompute current fingerprints rather than copying producer fingerprints,
   and add fine-grained target-lock support before broadening its scope.
8. Generalize linked-action identity and output manifests before caching proc
   macros, build-script executables, test binaries, debug sidecars, and final
   links.

Build-script execution state is a separate capsule design. It includes parsed
stdout, watched environment and paths, the complete `OUT_DIR` tree, and mtime
ordering. It should remain in workload snapshots until Cargo has a declared or
hermetic output contract.

### Instrumentation contract

`SRS_CARGO_ARTIFACT_CACHE_STATS=1` now emits one versioned JSON line after the
build queue. It remains disabled by default and reports:

- Cargo-fresh units;
- preflight attempts, already-fresh and dependency-blocked units, successful
  finalizations, bypasses, and elapsed time;
- eligible dirty units, hits, misses, and key failures;
- ineligible units by reason;
- direct dynamic-extern and compiler-wrapper rejection counts;
- publication success, rejection, and failure;
- restored and published files/logical bytes;
- accepted hardlink, configured-copy, and cross-device fallback counts;
- compiler identity files/bytes, one-time wall/CPU time, and reuse count;
- action-input hash calls, failures, and time;
- total hit and miss lookup time, materialization time, and publication time;
- restore lock, control validation, source validation, entry validation, final
  validation split into compiler-identity witness, loader inputs, and action
  inputs, and target-state write time;
- rustc executions;
- build-script process executions, failures, and time;
- exact-path snapshot manifest/reconstruction copy-on-write clones, byte
  copies, already-present files, logical bytes, failures, and time; and
- link-producing primary-package rustc executions and full action time.

Phase totals are cumulative worker time and may exceed wall time when jobs run
in parallel. Cargo-fresh scheduling counters and dirty artifact-admission
counters deliberately remain separate populations. Materialization file
counters include only accepted restores, while materialization time also
captures work discarded by a later validation failure. Cache-lock wait time
and action-input files/bytes remain useful future schema additions if the v1
measurements show that those aggregates cannot explain replay cost.
`primary_link_rustc` includes frontend and code generation, so linker-only time
still requires rustc or SLD timing data.

## Integration Gate

Do not use another broad uv CI experiment as the next diagnostic step. First
prove locally that:

- a cold parallel writer publishes nearly every eligible unit;
- stable pure-library graphs have near-complete cross-target hits;
- Clippy and test dependency hit rates are reported rather than inferred;
- target-path and generated-input portability tests stay green; and
- replay time is materially below the avoided rustc time.

The pure Build-library closure now demonstrates the local scheduling gate, but
Check replay, fine-grained locking, and the nonportable build-script closure
remain open. After those gates, uv PR 19754 can combine the repaired artifact
layer with job-specific target snapshots and compare equivalent runner classes.
The aggregate CI acceptance target still requires that final integration rerun.

### uv PR 19754 integration path

The pinned PR revision currently installs SRS `2026.06.09`, combines registry,
git, and artifact-cache data in one `actions/cache` object, and restores target
state only for selected macOS no-debug and test paths. Linux nextest, Linux
Clippy, generated-file tools, and the profiling benchmark do not receive
workload target snapshots. The integration should change only after the local
layer gates above pass:

1. Update `setup-srs/install.sh` to the immutable SRS release containing the
   loader, publication/restore-lock, statistics, and subcommand-propagation
   fixes, with new archive checksums.
2. Give `setup-srs` explicit `cache-domain`, `command-schema`, `target-path`,
   backend, linker, runner/path-domain, and writer inputs. Include those values
   plus the SRS/cache schema in keys; do not use the broad target restore prefix
   across incompatible command or toolchain domains.
3. Separate the portable artifact archive from registry/git download caches so
   its hit, byte, and writer semantics are observable. Keep one trusted writer
   for each cache domain rather than letting parallel jobs race publication.
4. Set `SRS_CARGO_ARTIFACT_CACHE_STATS=1` for the measured jobs and retain the
   single JSON line as a small job artifact. Report internal admission/hit
   rates alongside target-cache hit state and command wall time.
5. Add isolated target domains for the Linux/macOS no-debug root builds,
   Linux/macOS nextest profiles, Linux Clippy, generated-file tools, and
   profiling benchmarks. Restore to the same absolute workspace-relative path
   and preserve the current commands and validation. Compare full target and
   thin nonportable-state snapshots before choosing the smaller transfer.
6. Force LLVM target codegen for CodSpeed simulation while keeping linker and
   profile selection constant inside each comparison. Attach SLD state only to
   the matching root-output snapshot and evaluate it on incremental edits.
7. Rerun equivalent runner classes and aggregate active runner time only after
   the local build, Clippy, test, snapshot, and correctness gates pass. That is
   the acceptance experiment, not the next diagnostic loop.
