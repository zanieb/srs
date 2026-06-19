# Cranelift-Generated Rust Runtime Performance

## Scope

This investigation measures the runtime of native Rust binaries produced by
Cranelift, not Cranelift compile time. LLVM and Cranelift use the same SRS
rustc, Cargo, sources, profile, target CPU, and linker. Only target codegen
changes.

The comparison was collected on aarch64 macOS at SRS `691d7d23a7a1`, with:

- rustc `1.97.0-dev (9e2ac2b97 2026-05-21)`;
- Cranelift `0.133.0-691d7d23a`;
- uv `6c963dd3cb0e`;
- Ruff/ty `e0eb28d6345b`;
- the repositories' `profiling` profile, with LTO disabled for both backends;
- `CARGO_INCREMENTAL=0`;
- the SRS artifact cache and incremental linker disabled; and
- `/usr/bin/clang` as the linker.

Ruff's production release profile uses fat LTO. Disabling it makes the
backend comparison possible and controlled, but probably makes the measured
LLVM lead conservative relative to the shipped Ruff binary.

## Representative Results

Each row is 20 timed `hyperfine` trials after 5 warmups. Lower is better.
"Patched" includes the Cranelift-specific MIR inlining defaults described
below.

| Workload | LLVM | Cranelift baseline | Cranelift patched | LLVM lead after patch | Patch change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 32.6 ms | 83.6 ms | 83.2 ms | 2.55x | -0.5% |
| `uv lock --check`, offline | 58.3 ms | 118.9 ms | 120.4 ms | 2.06x | +1.3% |
| `ruff check` over 1,592 fixtures | 80.8 ms | 316.0 ms | 272.0 ms | 3.37x | -13.9% |
| `ty check` over `scripts/ty_benchmark` | 47.0 ms | 214.5 ms | 206.5 ms | 4.40x | -3.7% |

The uv lock fixture is an intentional offline failure caused by the pinned
checkout's unavailable resolver data. The ty fixture reports the same four
unresolved imports in every successful compiler lane. These rows measure the
real resolver and type-checking failure paths, not a no-op command.

The conclusion is unambiguous: LLVM is currently about 2-4.4x faster for
these user-visible operations. The MIR inlining change recovers meaningful
ground for Ruff and some for ty, but it does not address uv's dominant costs.

### Binary size

The profiling binaries retain full debuginfo, so these are comparative rather
than distribution sizes.

| Binary | LLVM | Cranelift baseline | Cranelift patched |
| --- | ---: | ---: | ---: |
| Ruff | 39.4 MB | 135.8 MB | 134.4 MB |
| ty | 39.4 MB | 145.6 MB | 143.7 MB |
| uv | 87.2 MB | 255.0 MB | 266.5 MB |

Cranelift's much larger output is consistent with missed inlining, folding,
dead-code elimination, and code-layout opportunities. More inlining is not a
complete answer: the moderate policy is nearly size-neutral for Ruff and ty,
but grows uv by 4.5% without improving its measured operations.

## Changes Implemented

### Backend-aware MIR inlining

Cranelift has no later, function-level inliner comparable to LLVM's. The
shared MIR inliner is therefore its only opportunity to expose optimization
across many call boundaries.

The codegen backend can now supply MIR inlining defaults. Cranelift raises the
local thresholds from 30/100/50 to 60/200/100 for forwarders, hinted calls,
and ordinary calls. Other backends are unchanged, and explicit command-line
thresholds always win.

Two controls bounded the policy:

- Raising the cross-crate threshold from 100 to 200 added no measurable Ruff
  or ty benefit, so the patch leaves it alone.
- An aggressive always-inline experiment improved Ruff by 19.2%, but produced
  a 235 MB Ruff binary and a 559 MB ty binary, as well as unsupported-intrinsic
  and linker-pressure warnings. It is not a viable default.

The retained moderate policy is the useful part of that curve: 13.9% faster
Ruff and 3.7% faster ty in the final controlled run, without the aggressive
variant's size or correctness risk.

### Late aggregate scalarization and dereference-aware SSA

Destination propagation can make an aggregate local stop escaping after the
first SROA pass has already run. LLVM can often recover these values later,
but Cranelift otherwise receives the aggregate as memory. Cranelift now asks
the optimized MIR pipeline to run a second SROA pass after destination
propagation. The pass is backend-scoped: an LLVM compilation does not run it.

Cranelift's own SSA analysis also no longer stack-spills the base pointer for
`&*pointer`. That expression takes the address of the pointee, not the address
of the pointer local, so the pointer remains eligible for an SSA value.

A reduced Ruff-style byte scan showed why both pieces matter. Late SROA splits
the slice iterator into its pointer and end-pointer fields; the dereference-aware
analysis then keeps those fields out of memory. The AArch64 stack frame shrank
from 96 to 64 bytes and the scalar Cranelift loop improved by roughly 20%.
LLVM still runs the reduction about two orders of magnitude faster because it
processes 64 bytes per loop with NEON, so automatic loop vectorization remains
the dominant long-term gap.

The application gate used 20 balanced, randomized trials after five warmups.
The paired change compares the candidate to the preceding Cranelift build from
the same trial, and the sign-test p-value excludes ties.

| Workload | LLVM | Previous Cranelift | Late SSA | LLVM lead | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 31.9 ms | 80.7 ms | 76.1 ms | 2.39x | -5.7% | 19/20 | 0.00004 |
| `uv lock --check`, offline | 59.8 ms | 118.0 ms | 113.7 ms | 1.90x | -3.1% | 18/20 | 0.00040 |
| `ruff check` over 1,592 fixtures | 77.6 ms | 281.8 ms | 277.0 ms | 3.57x | -2.0% | 17/20 | 0.00258 |
| `ty check` over `scripts/ty_benchmark` | 46.1 ms | 197.7 ms | 188.4 ms | 4.09x | -4.3% | 20/20 | 0.000002 |

The candidate's uv, Ruff, and ty binaries are respectively 1.02%, 1.14%, and
1.04% smaller than the previous Cranelift binaries. Correctness-probe exit
codes and stdout/stderr digests match across LLVM, the previous Cranelift
build, and the candidate.

### AArch64 CRC intrinsics

The initial Cranelift ty binary aborted while reading its vendored typeshed
archive because `llvm.aarch64.crc32x` was unsupported. All eight AArch64 CRC32
and CRC32C LLVM intrinsics are now lowered through fixed-register inline
assembly:

- `crc32b`, `crc32h`, `crc32w`, and `crc32x`;
- `crc32cb`, `crc32ch`, `crc32cw`, and `crc32cx`.

The Cranelift standard example checks every intrinsic against a known result.
This is primarily a coverage and correctness change, but it is required for
ty to become a usable performance benchmark.

## Performance Roadmap

The benchmark suite should remain the decision gate. A candidate only moves
up the roadmap when it improves at least one representative workload without
materially regressing the others or causing disproportionate binary growth.

### 1. Keep the matched uv/Ruff/ty matrix as a regression gate

Record runtime, binary size, and compile time separately. Runtime comparisons
must pin the compiler, backend, source revision, profile, target CPU, linker,
and cache settings. A successful build is not a runtime result, and a compiler
crash must not silently disappear from the sample set.

Use workloads that exercise real work:

- uv environment creation and offline resolution;
- Ruff parsing, indexing, semantic analysis, and diagnostics over a fixed
  source corpus; and
- ty project discovery, typeshed loading, indexing, and type checking.

### 2. Build loop and range optimization before chasing isolated peepholes

Ruff and ty spend much of their time walking bytes, tokens, AST nodes, and
hash-table groups. LLVM can combine loop transformations, range reasoning,
bounds-check elimination, and auto-vectorization; Cranelift currently cannot
match that pipeline. The open Cranelift
[bounds-check elimination issue](https://github.com/bytecodealliance/wasmtime/issues/4132)
explicitly calls for value-range analysis as the later stage of that work.

This is the highest-upside arc for the parser-heavy Ruff and ty rows. Start
with redundant checks over the same SSA index, then extend the analysis to
affine index relationships. Use reduced CLIF extracted from the hot byte and
token loops, but accept a change only after the application benchmarks move.

The late scalarization work above is the first completed part of this arc: it
removes memory traffic around iterator state, but does not transform the loop.
The reduced scan confirms that vectorization and loop-wide range reasoning are
now the next larger opportunities.

Automatic loop vectorization is a longer-horizon companion. It is likely
necessary to close the largest LLVM gap, but its analysis, legality checks,
cost model, and target lowering make it a program of work rather than a first
patch.

### 3. Improve egraph extraction and machine scheduling

Cranelift's egraph already discovers equivalent expressions, but choosing the
best expression and placing it well on the target are separate problems. The
current roadmap includes:

- [sharing-aware egraph costs](https://github.com/bytecodealliance/wasmtime/issues/12156);
- [target-specific egraph costs](https://github.com/bytecodealliance/wasmtime/issues/12005);
- [instruction scheduling heuristics](https://github.com/bytecodealliance/wasmtime/issues/6260);
  and
- a [VCode peephole pass](https://github.com/bytecodealliance/wasmtime/issues/8520).

These are medium-to-high priority after the first loop/range work. Hashing,
parsing, and small semantic-analysis kernels have dense data dependencies and
many short target-specific sequences, making them better validation workloads
than microbenchmarks alone.

### 4. Reduce call, register, and stack overhead

The inlining result proves that call boundaries matter, but more indiscriminate
inlining quickly becomes counterproductive. Continue with targeted policies:

- incorporate call-site frequency and callee size into MIR defaults;
- investigate the open work to let
  [regalloc manage callee-saved registers](https://github.com/bytecodealliance/wasmtime/issues/7727);
- remove [unused stack slots](https://github.com/bytecodealliance/wasmtime/issues/6661)
  and [dead stores](https://github.com/bytecodealliance/wasmtime/issues/4167); and
- pass or retain stack arguments without unnecessary entry-block loads where
  [possible](https://github.com/bytecodealliance/wasmtime/issues/6301).

The application gate is important here: uv's neutral inlining result says its
hot paths will not improve merely by buying more code size.

### 5. Expand native intrinsic coverage continuously

Missing intrinsics turn performance comparisons into compatibility tests and
can force slower fallback code. Add a benchmark startup smoke before every
timing matrix, and treat any unsupported intrinsic as a prerequisite fix.
Prefer native Cranelift instructions when available; use small, reviewed inline
assembly lowerings only where the backend has no equivalent yet.

## Reproduction Shape

On Darwin, `cargo +srs` currently defaults target codegen to LLVM. Select
Cranelift explicitly; otherwise a purported Cranelift comparison is actually
LLVM.

```bash
SRS_TARGET_CODEGEN_BACKEND=cranelift \
SRS_ARTIFACT_CACHE=0 \
SRS_INCREMENTAL_LINKER=0 \
CARGO_INCREMENTAL=0 \
CARGO_PROFILE_PROFILING_LTO=false \
CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang \
CARGO_TARGET_DIR="$HOME/code/tmp/cranelift-runtime/ruff-cranelift" \
    cargo +srs build --profile profiling --bin ruff --bin ty
```

Build the LLVM lane into a separate empty target with the same environment and
`SRS_TARGET_CODEGEN_BACKEND=llvm`. Run each command repeatedly from the same
workspace and keep output outside the timed path.

The checked-in runner records repository revisions, executable hashes and
sizes, correctness-probe output, randomized lane order, every trial, and both
per-lane and paired summaries. Paired summaries include the median relative
change, median absolute deviation, wins/losses, and an exact two-sided sign
test. It accepts a third baseline lane when comparing a new candidate with
both LLVM and the previous Cranelift result:

```bash
python3 scripts/benchmark-cranelift-runtime.py \
  --lane llvm=<uv-llvm>,<ruff-llvm>,<ty-llvm> \
  --lane baseline=<uv-clif-base>,<ruff-clif-base>,<ty-clif-base> \
  --lane candidate=<uv-clif-new>,<ruff-clif-new>,<ty-clif-new> \
  --uv-workspace <uv-checkout> \
  --ruff-workspace <ruff-checkout> \
  --expect-uv-rev 6c963dd3cb0e \
  --expect-ruff-rev e0eb28d6345b \
  --scratch "$HOME/code/tmp/cranelift-runtime-benchmark" \
  --output "$HOME/code/tmp/cranelift-runtime-benchmark/results.json"
```

For a quick single-workload investigation, select it with `--workloads` and
reduce `--runs` explicitly. For example:

```bash
python3 scripts/benchmark-cranelift-runtime.py <lane and workspace arguments> \
  --workloads ruff-check --warmups 5 --runs 20 \
  --scratch "$HOME/code/tmp/cranelift-runtime-ruff" \
  --output "$HOME/code/tmp/cranelift-runtime-ruff/results.json"
```

Do not compare the two fresh build durations from this runtime matrix: source
cache state, dependency reuse, and machine thermals were not block-balanced
for compile-time measurement.
