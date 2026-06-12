# Artifact Cache Benchmarking

This protocol measures the portable SRS Cargo artifact cache without using uv
CI as the diagnostic harness. It can run on a local Linux host or a bare
Namespace instance. Keep the uv revision, SRS toolchain, machine, backend,
linker, filesystem, and Cargo job count fixed for a complete matrix.

The primary question is coverage: how many compiler actions can safely reuse a
portable entry across target paths? Replay overhead is a secondary question
because expanding coverage is not useful when validating and materializing a
hit costs as much as the avoided compiler action.

## Conditions

For every workload, the runner first populates an immutable warm seed from an
empty producer target. The measured consumer always uses a different, fixed
absolute target path. Each invocation starts with an empty consumer target.

- **Disabled** disables the artifact cache and uses an empty active cache.
- **Cold** enables an empty active cache.
- **Warm** resets the active cache from the immutable producer seed.

Copying the seed, clearing the target, and collecting directory sizes happen
outside the timed interval. The seed is copied again before every warm trial,
so an earlier consumer cannot teach a later consumer target-specific variants.
The runner hashes the seed before and after the matrix and fails if it changed.
The warm condition deliberately models a locally materialized cache whose seed
was copied immediately before the timed command. It does not model a remote
download or cold filesystem page cache, and it does not sample publication
variance across multiple producers.

Seven trials use randomized blocks containing disabled, cold, and warm once
each. Across the first six blocks, every condition appears exactly twice in
each ordinal position. The recorded seed makes the order reproducible.

## Workloads

[`artifact-cache-benchmark-workloads.json`](../scripts/artifact-cache-benchmark-workloads.json)
contains command arrays rather than shell fragments. The retained commands are:

```text
cargo build --profile no-debug --bin uv --bin uvx --locked
cargo clippy --workspace --all-targets --all-features --locked
cargo nextest run --cargo-profile fast-build --features test-python-patch,native-auth,secret-service --workspace --profile ci-linux
```

The nextest environment retains the Linux filesystem and snapshot variables.
Provision `/btrfs`, `/tmpfs`, and `/minix`, install the required Python
versions, `cargo-nextest`, and the secret-service dependencies before running
that workload. Preparation and dependency hydration are not benchmark trials.

## Running

Use an installed final-HEAD SRS Cargo wrapper, not the bootstrap Cargo used to
build SRS. Put output, target, and cache roots outside both repositories and on
the same filesystem. The runner refuses dirty or unexpected revisions unless
`--allow-dirty` is explicitly supplied.

```bash
SRS_REVISION="$(git -C "$HOME/src/srs" rev-parse HEAD)"
python3 "$HOME/src/srs/scripts/benchmark-artifact-cache.py" run \
  --workspace "$HOME/src/uv" \
  --expect-workspace-rev f74311c15 \
  --expect-srs-rev "$SRS_REVISION" \
  --cargo "$HOME/.rustup/toolchains/srs-cache/bin/cargo" \
  --toolchain-provenance "$HOME/src/srs-toolchain-provenance.json" \
  --workloads build,clippy,test \
  --backend llvm \
  --linker clang \
  --jobs 16 \
  --materialization hardlink \
  --trials 7 \
  --seed 19754 \
  --target-root "$HOME/code/tmp/srs-cache-benchmark/target" \
  --cache-root "$HOME/code/tmp/srs-cache-benchmark/cache" \
  --output "$HOME/code/tmp/srs-cache-benchmark/results"
```

Run LLVM and Cranelift as separate matrices with separate output, target, and
cache roots. The pinned nextest workload requires LLVM, so the harness rejects
`test` in a Cranelift matrix; compare Cranelift only for `build` and `clippy`.
Hold the linker and job count constant within each comparison.

The provenance JSON must contain the exact full `srs_revision`, the SHA-256 of
the installed toolchain archive as `artifact_sha256`, a nonempty `source`
identifying its build record, and an `executables` object containing the
expected SHA-256 values for `cargo`, `cargo-srs-real`, and `rustc`. The runner
rejects executable mismatches and records resolved linker, cargo-nextest, and
Python identities. It sets `CARGO_INCREMENTAL=0`, disables SLD incrementality
and exact-path snapshots, removes inherited rustc and wrapper overrides, clears
inherited cache size limits, configures materialization explicitly, requests
structured cache statistics, and records the target backend in target-only
rustflags. Every cold or warm record must confirm that the cache was configured;
every disabled record must confirm the opposite.

Regenerate a summary from retained raw data with:

```bash
python3 scripts/benchmark-artifact-cache.py summarize \
  "$HOME/code/tmp/srs-cache-benchmark/results" --format markdown
```

## Namespace

Use one fixed bare instance for the complete matrix. Creating a new instance
for each condition would replace build noise with machine variance. Pin the
machine type and duration according to the expected workload, then upload or
download only outside timed commands. For example:

```bash
nsc create --bare --ephemeral --duration 8h \
  --machine_type "$NAMESPACE_MACHINE_TYPE" \
  --purpose "SRS artifact-cache final-HEAD benchmark" \
  --cidfile "$HOME/code/tmp/srs-cache-benchmark.cid"

INSTANCE="$(cat "$HOME/code/tmp/srs-cache-benchmark.cid")"
nsc instance upload "$INSTANCE" srs-toolchain.tar.gz /work/input/srs-toolchain.tar.gz --mkdir
nsc instance upload "$INSTANCE" srs-toolchain-provenance.json /work/input/srs-toolchain-provenance.json --mkdir
nsc ssh -T "$INSTANCE" '<provision pinned SRS and uv; run the command above under /work>'
nsc instance download "$INSTANCE" /work/results \
  "$HOME/code/tmp/srs-cache-benchmark/results" --mkdir
```

Build or download the final-HEAD toolchain before the matrix and construct its
provenance record from the CI run head SHA and artifact digest. Record the
Namespace instance metadata alongside `metadata.json`; the target-side runner
also records kernel, CPU model and affinity, workspace/target/cache/output
filesystems, cgroup, mount, load, pressure, and memory information without
depending on Namespace-specific APIs.

## Retained Evidence

Each population and measured invocation retains raw stdout, raw stderr, the
exact structured Cargo statistics record, command result and resource usage,
and before/after system samples. `metadata.json` records revisions, the Cargo
version, configuration digest, schedule, backend, linker, jobs, paths, and host
information. `runs.jsonl` is the append-only result index.

The runner refuses to recursively clean a nonempty root unless it already has
the harness ownership marker. It validates every root and the target/cache
filesystem relationship before deletion. It also verifies that each workload
leaves the pinned repository status unchanged, including newly ignored paths;
writable snapshot state is redirected into the retained run directory.

`summary.json` and `summary.md` reject incomplete or duplicate matrices, report
all wall samples, their median and median absolute deviation, paired cold-minus-
disabled and warm-minus-disabled deltas within each randomized block, and three
deliberately separate aggregate rates:

- **Eligibility:** eligible actions divided by eligible plus ineligible actions.
- **Effectiveness:** hits divided by eligible actions.
- **Total coverage:** hits divided by eligible plus ineligible actions.

Thus 25 hits among 25 eligible actions in a 70-action graph are 100% effective
but cover only 35.7% of the graph. Disabled rows normally have no admission
population and therefore show these rates as unavailable.

The summary also retains samples, availability, medians, and cross-trial totals
for the supporting counters. Its rendered tables include:

- restored files and logical bytes;
- hardlinked, configured-copy, and cross-device-copy files and logical bytes;
- published files and logical bytes;
- compiler-identity files, bytes, and wall time;
- action-input hash calls, files, bytes, and wall time;
- lookup, preflight, materialization, publication, rustc, and build-script
  cumulative worker time;
- build-script executions; and
- median per-run and aggregate counts for every reported ineligibility reason.

Missing optional fields are `null` in `summary.json` and `n/a` in Markdown;
they are never silently converted to zero. Early version-1 Cargo statistics
records may omit action-input file and byte volumes; final-HEAD SRS reports
them and the harness includes them directly.

Cargo phase counters are cumulative worker time and overlap under parallel
execution. Do not add hashing, lookup, materialization, rustc, build-script,
or primary-link counters to manufacture a wall-time budget. Preserve them for
attribution alongside the externally measured wall time. Use a separate,
untimed instrumentation pass when Cargo timing spans or per-linker SLD traces
are needed; do not silently add their overhead to the primary matrix.
