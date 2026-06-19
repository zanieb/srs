# Cranelift-Generated Rust Runtime Performance

## Scope

This investigation measures the runtime of native Rust binaries produced by
Cranelift, not Cranelift compile time. LLVM and Cranelift use the same SRS
rustc, Cargo, sources, profile, target CPU, and linker. Only target codegen
changes.

The latest comparison was collected on aarch64 macOS from the source tree now
recorded at SRS `51a7c38ad`, with:

- rustc `1.97.0-dev (9e2ac2b97 2026-05-21)`;
- Cranelift `0.133.0`;
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

Each row is 50 timed trials after five warmups. Lower is better. LLVM, the
preceding Cranelift policy, and the current Cranelift policy were run in the
same randomized, block-balanced matrix from freshly built binaries.

| Workload | LLVM | Current Cranelift | LLVM lead |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | 32.7 ms | 55.6 ms | 1.70x |
| `uv lock --check`, offline | 58.7 ms | 84.5 ms | 1.44x |
| `ruff check` over 1,592 fixtures | 79.2 ms | 145.3 ms | 1.83x |
| `ty check` over `scripts/ty_benchmark` | 47.1 ms | 115.7 ms | 2.46x |

The uv lock fixture is an intentional offline failure caused by the pinned
checkout's unavailable resolver data. The ty fixture reports the same four
unresolved imports in every successful compiler lane. These rows measure the
real resolver and type-checking failure paths, not a no-op command.

The conclusion is unambiguous: LLVM is currently about 1.4-2.5x faster for
these user-visible operations. The implemented changes recover meaningful
ground for Ruff and ty and a smaller amount for uv lock, but they do not close
the remaining loop-optimization and code-quality gap.

### Final acceptance gate

The final gate is the complete repository test suite under matched LLVM and
Cranelift toolchains, not just the four timing operations. The matched macOS
commands and their established 3,866-test uv, 7,962-test Ruff/ty, and doctest
results are recorded under [Full-Suite Backend Validation](#full-suite-backend-validation).
This expensive matrix is reserved for a policy that is otherwise ready to
finalize; the randomized application gate remains the inner loop. It will be
rerun on the eventual final policy once the runtime gap is close enough to call
the work complete.

### Binary size

The profiling binaries retain full debuginfo, so these are comparative rather
than distribution sizes.

| Binary | LLVM | Cranelift baseline | Current Cranelift |
| --- | ---: | ---: | ---: |
| Ruff | 45.0 MB | 135.8 MB | 117.4 MB |
| ty | 47.5 MB | 145.6 MB | 122.2 MB |
| uv | 100.0 MB | 255.0 MB | 238.7 MB |

Cranelift's much larger output is consistent with missed inlining, folding,
dead-code elimination, and code-layout opportunities. More inlining is not a
complete answer: the final policy reduces Ruff and ty size, but the remaining
output is still roughly 2.4-2.6 times LLVM's and uv environment creation is
still about 70% slower.

## Changes Implemented

### Backend-aware MIR inlining

Cranelift does not have LLVM's mature, whole-IR inlining pipeline. The shared
MIR inliner remains the primary opportunity to expose optimization across most
call boundaries; a bounded post-monomorphization CLIF inliner now handles a
small scalar subset that MIR cannot resolve early enough.

The codegen backend can now supply MIR inlining defaults. Cranelift initially
raised the local thresholds from 30/100/50 to 60/200/100 for forwarders,
hinted calls, and ordinary calls. After the scalarization work exposed more
small iterator calls, the hinted-call threshold moved to 500. The final policy
uses local thresholds of 60/800/100 and raises the cross-crate eligibility
threshold from 100 to 500. It also expands the top-down multi-call inlining
limit from 5 to 12. LLVM keeps its existing cross-crate and top-down defaults
and does not receive Cranelift's local defaults. Explicit command-line
thresholds always win.

Two controls bounded the policy:

- Raising only the cross-crate threshold from 100 to 200 on top of the
  60/500/100 policy added no measurable Ruff or ty benefit and slightly grew
  both binaries, so that candidate was rejected.
- An aggressive always-inline experiment improved Ruff by 19.2%, but produced
  a 235 MB Ruff binary and a 559 MB ty binary, as well as unsupported-intrinsic
  and linker-pressure warnings. It is not a viable default.

The retained moderate policy is the useful part of that curve: each increase
is justified by a named hot call and a balanced application gate, without the
aggressive variant's size or correctness risk.

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

### Thin `NonNull` values in SSA

Late SROA exposed iterator fields, but thin `NonNull<T>` values still lacked a
Cranelift scalar type and therefore remained in stack slots. This was especially
expensive for slice iterators: every loop iteration loaded, advanced, and
stored the current pointer through memory.

Thin `NonNull<T>` is now represented as a pointer-sized Cranelift SSA value;
fat `NonNull<T>` remains memory-backed. Pattern-typed pointer fields use their
base type's machine representation, and transparent scalar field projections
and scalar constants preserve the wrapper's layout while operating on the
underlying value.

In the reduced byte-scan kernel, this shrank the AArch64 frame from 64 to 32
bytes, removed the loop-carried pointer spills, and improved the scalar loop by
about 6.2x. The application gate used the same 20 balanced, randomized trials
and five warmups as the preceding comparison:

| Workload | LLVM | Previous Cranelift | Scalar `NonNull` | LLVM lead | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 32.2 ms | 79.3 ms | 78.7 ms | 2.44x | -0.4% | 11/20 | 0.82380 |
| `uv lock --check`, offline | 59.2 ms | 116.3 ms | 115.4 ms | 1.95x | -0.6% | 12/20 | 0.50344 |
| `ruff check` over 1,592 fixtures | 77.2 ms | 282.7 ms | 248.4 ms | 3.22x | -13.0% | 20/20 | 0.000002 |
| `ty check` over `scripts/ty_benchmark` | 47.5 ms | 195.4 ms | 188.3 ms | 3.96x | -4.9% | 17/20 | 0.00258 |

The uv rows are statistically neutral, while Ruff and ty improve decisively.
The candidate's uv, Ruff, and ty binaries are respectively 0.58%, 0.80%, and
0.72% smaller. Correctness-probe exit codes and output digests match across all
three lanes.

### Scalar `NonZero` values in SSA

A later ty profile put `boxcar::Index::location` among the hottest remaining
pure-Rust leaves. LLVM emits a short register-only sequence for its `ilog2`,
power-of-two bucket length, and index arithmetic. Cranelift used a 48-byte
frame with repeated stack traffic for `NonZeroUsize` and
`Option<NonZeroUsize>`, even though rustc describes both layouts as one scalar.

An experiment that treated every scalar-layout ADT as an SSA value exposed
invalid assumptions in aggregate field projection during the stage-2 standard
library build. The retained change is deliberately narrower: only the
diagnostic `NonZero` item and `Option<NonZero<_>>` use the scalar path. The
exact kernel's frame fell from 48 to 16 bytes before the later stack-forwarding
change, and its wrapper and niche values no longer round-trip through memory.

The 50-run application gate measured small but consistently favorable effects:

| Workload | Paired change | Wins | Sign p | Binary change |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -1.03% | 33/50 | 0.03284 | -0.034% |
| `uv lock --check`, offline | -0.51% | 28/50 | 0.47989 | -0.034% |
| `ruff check` over 1,592 fixtures | -1.43% | 32/50 | 0.06491 | -0.035% |
| `ty check` over `scripts/ty_benchmark` | -0.48% | 32/50 | 0.06491 | -0.108% |

Only uv environment creation reaches the sign-test threshold on its own, but
all four medians improve and every binary shrinks. The focused regression,
cg_clif check, complete stage-2 backend and standard-library build, and direct
kernel execution pass, so the narrow scalar representation is retained.

### Higher hinted-call budget after scalarization

A fresh Ruff profile after the `NonNull` change still showed
`core::str::validations::next_code_point` calling the small, hinted
`slice::Iter<u8>::next` helper four times. LLVM folded the iterator operations
into a 160-byte leaf; Cranelift emitted a 648-byte function with repeated
calls, niche checks, and spills. This is exactly the kind of call boundary the
hinted MIR threshold is intended to remove.

Raising only that threshold from 200 to 500 produced the following paired
result in the complete four-workload gate:

| Workload | Paired change | Wins | Sign p | Binary change |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.6% | 12/20 | 0.50344 | -1.20% |
| `uv lock --check`, offline | -3.0% | 17/20 | 0.00258 | -1.20% |
| `ruff check` over 1,592 fixtures | -11.6% | 19/20 | 0.00004 | -2.53% |
| `ty check` over `scripts/ty_benchmark` | -7.1% | 16/20 | 0.01182 | -5.15% |

The ty row overlapped an unrelated host workload, so a focused Ruff/ty rerun
was used as a variance check. It measured Ruff at -11.1% and ty at -7.1%, with
20/20 wins and a two-sided sign-test p-value of 0.000002 for each. Correctness
digests match the previous Cranelift and LLVM lanes. Unlike indiscriminate
inlining, the higher hinted-call budget also makes every application binary
smaller, so it is retained as the default.

### Broader cross-crate hinted bodies

A profile of the 60/500/100 build showed `anstyle::Style::eq` as a remaining
Ruff leaf under diagnostic rendering. Cranelift emitted several calls from the
hot `StyledBuffer::render` path and used a 1,040-byte frame. Optimized MIR
assigned `Style::eq` a cost of 585, so neither the default cross-crate
eligibility limit of 100 nor a trial limit of 200 made the body available to
the MIR inliner.

The cross-crate-200 experiment was neutral: Ruff changed by -0.04% and ty by
-0.35%, with 11/20 and 12/20 wins respectively, while their binaries grew by
0.23% and 0.28%. It was rejected. Raising cross-crate eligibility to 500 and
the hinted-call budget to 600 admits the profiled body and removes the
out-of-line style comparisons from the hot function. The complete balanced
gate measured:

| Workload | Paired change | Wins | Sign p | Binary change |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -3.2% | 15/20 | 0.04139 | +0.028% |
| `uv lock --check`, offline | -0.9% | 13/20 | 0.26318 | +0.028% |
| `ruff check` over 1,592 fixtures | -1.2% | 12/20 | 0.50344 | +0.003% |
| `ty check` over `scripts/ty_benchmark` | -1.9% | 19/20 | 0.00004 | -0.428% |

A separate 50-trial Ruff/ty variance check measured Ruff at -1.4% with 30/50
wins and ty at -2.5% with 47/50 wins. Ruff remains directionally positive but
statistically neutral; ty is decisive in both runs. All four correctness
probes have identical exit codes and output digests between the 60/500/100 and
500-cross-crate/60/600/100 lanes. The size effect is effectively flat, so the
broader policy is retained.

### Deeper top-down iterator inlining

After the broader cross-crate policy removed `Style::eq`, a fresh Ruff profile
showed `[char]::contains` and its iterator adapters among the hottest remaining
pure-Rust leaves. The standard library deliberately expresses this operation
as `chunks_exact`, `fold`, and `any`, expecting the optimizer to flatten the
chain. Cranelift inlined the outer bodies but reached the MIR inliner's default
top-down multi-call limit before the final equality closure, leaving a function
call inside each scalar comparison of short character sets.

Cranelift first moved to a top-down limit of 8 instead of 5. A reduced
`[char]::contains` check falls from three functions and 412 bytes to one
276-byte leaf, removes the call in the comparison loop, and shrinks its frame
from 96 to 80 bytes. Setting `-Zinline-mir-top-down-depth=5` restores the old
assembly, confirming that an explicit override still wins. The complete
balanced gate measured:

| Workload | Paired change | Wins | Sign p | Binary change |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -1.8% | 18/20 | 0.00040 | -0.159% |
| `uv lock --check`, offline | +0.3% | 7/20 | 0.26318 | -0.159% |
| `ruff check` over 1,592 fixtures | -2.8% | 19/20 | 0.00004 | -0.295% |
| `ty check` over `scripts/ty_benchmark` | -0.2% | 12/20 | 0.50344 | -0.184% |

A separate 50-trial Ruff/ty check measured Ruff at -1.7% with 37/50 wins
(two-sided sign-test p = 0.00094) and ty at +0.08% with 24/50 wins. Ruff and uv
environment creation improve decisively; ty and uv resolution are neutral.
All correctness digests match, and all three binaries become smaller, so the
policy is retained.

### Globally enabled target features

An amplified ty profile made small AArch64 NEON wrappers the hottest active
Cranelift leaves. Calls such as `vceq_u8`, `vcltz_s8`, `vdup_n_u8`, and
`vld1_u8` remained out of line even though NEON is globally enabled for every
AArch64 function. The MIR inliner required the caller and callee's explicit
`#[target_feature]` lists to be identical and did not account for session-wide
features. LLVM repaired the missed opportunity in its later IR inliner;
at that point cg_clif had no enabled second inlining stage. The bounded CLIF
inliner added later is too narrow to move target-feature-sensitive bodies.

The compatibility check now removes globally enabled features before comparing
the function-local sets. It retains exact matching for genuinely local
features, so moving a call across a feature or ABI boundary remains forbidden.
The AArch64 regression test proves both sides: redundant NEON can inline into a
normal caller, while local SVE cannot. In the pinned ty binary, the 284 copies
of the four hot NEON wrapper families fall to zero.

The matched three-lane gate used freshly rebuilt LLVM and Cranelift binaries
from the same compiler and pinned application sources:

| Workload | Candidate vs previous Cranelift | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -1.6% | 15/20 | 0.04139 | +120.3% | -0.162% |
| `uv lock --check`, offline | +0.7% | 8/20 | 0.50344 | +74.3% | -0.162% |
| `ruff check` over 1,592 fixtures | +1.7% | 5/20 | 0.04139 | +163.7% | -0.211% |
| `ty check` over `scripts/ty_benchmark` | -4.5% | 18/20 | 0.00040 | +236.1% | +0.462% |

A separate 50-trial Ruff/ty gate measured Ruff at +0.6% with 19/50 wins
(two-sided sign-test p = 0.11892) and ty at -4.0% with 45/50 wins
(p = 0.0000000042). The broader gate confirms a real tradeoff: the change
improves the largest remaining gap and uv environment creation, leaves uv
resolution neutral, and regresses Ruff. It is retained as a net improvement
across the representative set, with the Ruff regression explicitly carried
into the next optimization gate rather than hidden by the aggregate result.

After removing the hot AArch64 wrappers, a fresh Ruff profile still found the
same iterator chain active below the eighth inlining level. The current policy
therefore raises the limit from 8 to 12. The named slice/iterator symbols in
Ruff fall from 153 to 136, while Ruff, ty, and uv shrink by 9,584, 16,240, and
40,016 bytes respectively. The complete three-lane gate measured:

| Workload | Candidate vs depth 8 | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.7% | 14/20 | 0.11532 | +116.9% | -0.015% |
| `uv lock --check`, offline | -1.4% | 12/20 | 0.50344 | +75.8% | -0.015% |
| `ruff check` over 1,592 fixtures | -1.3% | 13/20 | 0.26318 | +160.4% | -0.007% |
| `ty check` over `scripts/ty_benchmark` | +0.9% | 7/20 | 0.26318 | +236.6% | -0.012% |

A separate 50-trial Ruff/ty gate measured Ruff at -0.9% with 34/50 wins
(two-sided sign-test p = 0.01535) and ty at +0.02% with 25/50 wins. This
recovers the target-feature change's Ruff regression without sacrificing its
ty improvement. The broader gate is directionally favorable for both uv
operations and Ruff, ty is neutral across the higher-powered focused gate,
and every binary becomes smaller, so the depth-12 policy is retained.

### Small constant non-overlapping copies

The next amplified ty profile put `_platform_memmove` at the top of the active
stack at roughly 11 times LLVM's sampled rate: 5,748 samples over 10 seconds
versus 257 over 5 seconds. A local interposition trace showed that two
monomorphizations of `read_unaligned<uint8x8_t>` alone issued roughly 1.2
million eight-byte libc copies in the unamplified check. Fixed-size hash-table
group loads were another large family. LLVM expands these calls in its later
IR pipeline; Cranelift was discarding the constant count and always calling
libc.

cg_clif now preserves a constant `CopyNonOverlapping` byte count and passes it,
the known alignment, and the non-overlap guarantee to Cranelift's existing
small-copy emitter. Copies requiring at most four load/store pairs stay in the
function; larger or dynamic copies retain the libc path. The regression probe
covers both a three-element `u32` copy and an unaligned `u64` read. The emitted
AArch64 code for the former is three loads followed by three stores, with no
libc call.

The 50-run matched three-lane gate measured:

| Workload | Candidate vs depth 12 | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.9% | 32/50 | 0.06491 | +114.6% | -0.028% |
| `uv lock --check`, offline | -1.0% | 30/50 | 0.20264 | +73.1% | -0.028% |
| `ruff check` over 1,592 fixtures | -0.2% | 26/50 | 0.88772 | +155.3% | -0.085% |
| `ty check` over `scripts/ty_benchmark` | -1.0% | 32/50 | 0.06491 | +235.3% | -0.075% |

A separate 50-run two-lane replication measured uv environment creation at
-2.0% (37/50 wins, p = 0.00094), uv resolution at -0.9% (33/50,
p = 0.03284), ty at -1.1% (37/50, p = 0.00094), and Ruff at +0.2%
(23/50, p = 0.67181). An overlapping-copy experiment removed additional hot
libc calls but regressed uv, so it was rejected. The retained change improves
the three affected workloads, is neutral for Ruff, and shrinks every binary.

### Final bounded hinted-call budget

The small-copy profile left hashbrown's hinted group-probe loop as the hottest
pure-Rust leaf. A threshold sweep provided a useful boundary. Raising the
hinted-call budget from 600 to 1,000 reduced the remaining probe copies from 31
to 20 and improved ty by 0.75%, but regressed Ruff by 1.85% while growing its
binary by 1.74%; that candidate was rejected. A budget of 800 keeps the large
probe out of line while admitting the smaller hinted bodies below it.

The exact retained combination, measured in a 50-run three-lane gate, produced:

| Workload | Candidate vs small-copy policy | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -1.4% | 32/50 | 0.06491 | +116.1% | +0.204% |
| `uv lock --check`, offline | -0.5% | 30/50 | 0.20264 | +74.6% | +0.204% |
| `ruff check` over 1,592 fixtures | -2.1% | 35/50 | 0.00660 | +160.6% | +0.846% |
| `ty check` over `scripts/ty_benchmark` | -0.7% | 31/50 | 0.11892 | +227.5% | +0.450% |

All four workloads move in the intended direction and Ruff improves
decisively. The size cost is bounded and well below the rejected 1,000-budget
candidate, so 800 is retained as the backend default. Explicit command-line
thresholds continue to override it.

### Bounded post-monomorphization CLIF inlining

The next ty profile showed tiny `rustc_hash::FxHasher` methods consuming about
950 active samples. A targeted MIR census found that every direct
`write_usize`, `write_u64`, `write_u32`, and `finish` call considered by the
MIR inliner was already accepted. Machine-code inspection found the remaining
calls inside generic upstream `Hash::hash` implementations: MIR optimizes those
bodies while the `Hasher` type is still abstract, and the calls become direct
only after monomorphization. LLVM inlines them in its later IR pipeline, while
cg_clif previously emitted the newly direct calls unchanged.

Cranelift 0.133 provides function-inlining mechanics but leaves policy and
call-graph ownership to the embedding compiler. cg_clif now uses that API
within each codegen unit for a deliberately narrow set of `#[inline]` callees:
at most 32 CLIF instructions and three blocks, with no nested calls, stack
slots, dynamic stack slots, global values, or stack limit. It legalizes the
callee before insertion, skips recursion, and visits only the original caller
body, so one decision cannot recursively expand another.

The safety boundary is empirical. Allowing general three-block hinted bodies
miscompiled stage-2 build scripts; call-free bodies still reproduced the
failure until stack and global state were excluded. The retained scalar subset
passed a complete two-stage SRS build. In the pinned ty binary it removes all
847 direct calls to the four hot `FxHasher::write_*` families.

The 50-run matched three-lane gate measured:

| Workload | Candidate vs previous Cranelift | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -3.40% | 46/50 | 0.00000000045 | +112.4% | -0.736% |
| `uv lock --check`, offline | -3.20% | 45/50 | 0.0000000042 | +70.4% | -0.736% |
| `ruff check` over 1,592 fixtures | -0.40% | 28/50 | 0.47989 | +154.5% | -0.607% |
| `ty check` over `scripts/ty_benchmark` | -4.90% | 48/50 | 0.0000000000023 | +218.6% | -0.751% |

Both uv operations and ty improve decisively, Ruff is neutral, every binary
shrinks, and all correctness-probe exit codes and output digests match. This is
retained. Extending the CLIF inliner beyond the scalar subset now requires a
reduced correctness reproducer and explicit validation of stack slots, global
values, unwind edges, and debug metadata before expanding the policy.

A follow-up profile exposed 359 remaining direct calls to
`FxHasher::write_isize` in ty. The function is already a tiny call-free scalar
leaf, so a second bounded inlining pass could not help: it removed only one
additional `rustc_hash` call from the entire binary. The real boundary is
codegen-unit ownership. The hot call sites and the locally emitted copies of
the default trait method are in different cg_clif modules, while the current
inliner deliberately owns only one module's function bodies.

Admitting ordinary unhinted scalar leaves up to 16 CLIF instructions tested
the nearest broader policy. It removed 86 additional `rustc_hash` calls from
ty and shrank uv, Ruff, and ty by 1.60%, 1.22%, and 2.14%, respectively, but a
20-run application screen rejected the tradeoff. Ruff improved by 1.65% and
ty by 1.05% directionally; uv environment creation regressed by 0.87%, and uv
resolution regressed by 0.94% with only 5/20 wins (two-sided sign-test
`p = 0.04139`). The policy also left all 359 cross-CGU `write_isize` calls in
place. Closing this part of the LLVM gap therefore needs cross-CGU body import
or a genuinely post-link optimizer, not a looser per-CGU size threshold.

The bounded body-import experiment proved that the call boundary can be
crossed, but also showed that doing so without a call-site model is not a
runtime win. Each codegen unit recorded its referenced monomorphizations,
translated only hinted MIR bodies with at most six blocks and 64 operations
into an isolated catalogue, imported one dependency level, and admitted only
the existing call-free, stack-free, global-free 32-instruction scalar subset.
The composed candidate was capped at six CLIF blocks because one inlining step
adds unconditional jump scaffolding around the otherwise straight-line body.

This removed every direct `FxHasher::write_isize` call: 356 in uv, 149 in
Ruff, and 359 in ty. The binaries shrank by 0.21%, 0.12%, and 0.24%,
respectively, and the complete stage-2 backend and standard-library build
passed. The 20-run application screen was nevertheless mixed:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +0.46% | 9/20 | 0.82380 |
| `uv lock --check`, offline | -1.09% | 12/20 | 0.50344 |
| `ruff check` over 1,592 fixtures | +1.61% | 8/20 | 0.50344 |
| `ty check` over `scripts/ty_benchmark` | -0.92% | 12/20 | 0.50344 |

All correctness digests matched, but none of the movements was decisive and
Ruff moved in the wrong direction. The experiment was rejected. Cross-CGU
availability is therefore not sufficient by itself; another attempt needs a
hot-call-site or callee-specific profitability signal that can avoid broad
layout perturbation.

A second threshold probe targeted callable `dyn Fn` shims. Raising the hinted
budget from 800 to 850 retained all 31 hot hashbrown probe copies; 900 and
1,000 reduced the count to 20 but did not improve the application gate and
worsened ty. A one-codegen-unit upper-bound build reduced ty's size by 9.5%
but left the profiled `boxcar` helper count unchanged and only moved the probe
count from 31 to 30. This reinforces the same roadmap conclusion: further
progress needs cross-crate body import and stronger post-monomorphization
optimization, not another local threshold increase.

### Profiled small constant copies

A size histogram of the remaining Darwin libc copy traffic corrected an
important attribution detail: the hot `_platform_memmove` implementation is
also reached through `_memcpy`, and symbolicated callers load the non-overlap
symbol. In one representative ty check, exact 40-, 20-, and 10-byte copies
accounted for 2,526,499, 1,746,150, and 389,754 calls. Each requires exactly
five register load/store pairs, one beyond Cranelift's original threshold of
four.

Raising the shared small-copy threshold to five removed 11,232 static ty
`memcpy` call sites. The dynamic 40-byte count fell to 1,464, the 20-byte count
to 155,826, and the 10-byte count to 2,443: 4,502,670 calls removed across
those three sizes. The frontend regression test now fixes the boundary at a
40-byte, five-register copy and verifies that all loads precede all stores.

The 50-run matched three-lane gate measured:

| Workload | Candidate vs previous Cranelift | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -2.22% | 41/50 | 0.0000056 | +100.0% | +0.037% |
| `uv lock --check`, offline | -1.77% | 32/50 | 0.06491 | +67.8% | +0.037% |
| `ruff check` over 1,592 fixtures | -10.39% | 49/50 | 0.000000000000091 | +125.4% | +0.081% |
| `ty check` over `scripts/ty_benchmark` | -1.73% | 40/50 | 0.0000239 | +207.4% | +0.021% |

All four workloads improve directionally, three decisively, while binary
growth stays below 0.1%. Correctness-probe exit codes and output digests match.
The focused frontend tests and the complete two-stage SRS build, including
rustc, cg_clif, std, Cargo, and Clippy, pass. The five-register step was
retained.

A fresh profile after that change still put libc copies at the top. The next
four exact sizes were 28, 48, 56, and 64 bytes, requiring seven, six, seven,
and eight register pairs. Raising the ceiling from five to eight removed a
further 25,602 static ty `memcpy` call sites. Their combined dynamic count fell
from 5,836,618 to 11,494, eliminating 5,825,124 calls in the representative
check.

The subsequent 50-run gate measured the eight-register policy against the
five-register policy:

| Workload | Candidate vs five registers | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +0.76% | 19/50 | 0.11892 | +100.1% | +0.407% |
| `uv lock --check`, offline | +0.62% | 21/50 | 0.32224 | +66.1% | +0.407% |
| `ruff check` over 1,592 fixtures | -0.74% | 27/50 | 0.67181 | +124.8% | +0.439% |
| `ty check` over `scripts/ty_benchmark` | -2.69% | 44/50 | 0.000000032 | +200.3% | +0.431% |

The uv movements are both below 1% and statistically neutral, Ruff is neutral,
and ty improves decisively for less than 0.5% binary growth. The regression
test now fixes the ceiling at a 64-byte, eight-register copy. The complete SRS
build and the focused frontend tests pass, so eight registers is the retained
ceiling.

Two larger ceilings were screened and rejected. Nine registers grew uv,
Ruff, and ty by 0.168%, 0.095%, and 0.098%, respectively, but its 20-run paired
changes against eight registers were statistically neutral: -0.06% for uv
environment creation (10/20 wins, `p = 1.0`), +0.59% for uv resolution (9/20,
`p = 0.82380`), -1.83% for Ruff (12/20, `p = 0.50344`), and +0.34% for ty
(9/20, `p = 0.82380`). Twelve registers cost substantially more code size:
1.73% for uv, 3.17% for Ruff, and 0.54% for ty. Its 20-run screen was likewise
neutral on all four workloads, with paired median changes of +0.60%, +0.17%,
-1.57%, and -0.12%, respectively. The profile-supported ceiling therefore
ends at eight registers; extending it speculatively does not pay for itself.

### Profiled small constant byte fills

The same libc census found that cg_clif sent constant-size byte repeats and
`write_bytes` operations directly to `memset`, even though the Cranelift
frontend already expands small fills. One representative ty check made
1,432,618 eight-byte `memset` calls and 53,779 24-byte calls. Preserving the
constant size through cg_clif and adding a frontend entry point for a runtime
byte value reduced those counts to 16 and 3,289, respectively. The dynamic
byte is widened and replicated only for an inline fill; larger or nonconstant
operations retain the existing libc call.

The 50-run matched gate measured the change against the retained eight-register
copy policy:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +1.17% | 20/50 | 0.20264 | +102.1% | -0.027% |
| `uv lock --check`, offline | -0.94% | 32/50 | 0.06491 | +64.6% | -0.027% |
| `ruff check` over 1,592 fixtures | +0.12% | 24/50 | 0.88772 | +131.8% | -0.018% |
| `ty check` over `scripts/ty_benchmark` | -0.43% | 33/50 | 0.03284 | +199.9% | -0.031% |

Ty improves decisively, uv resolution improves directionally, and the other
two rows are neutral. All three binaries shrink and every correctness digest
matches, so the constant-size fill expansion is retained. Its focused
frontend tests, cg_clif check, and complete SRS build pass.

A nearby constant-comparison experiment was rejected. The remaining libc
`memcmp` traffic comes from dynamic `compare_bytes` paths rather than
constant-size `raw_eq`, so expanding `raw_eq` changed neither the relevant
static call sites nor the dynamic census.

### Pre-legalization stack-load forwarding

Cranelift legalizes explicit `stack_load` and `stack_store` operations to
ordinary memory operations before alias analysis. Its coarse memory version
then treats an intervening store to a different offset as a clobber, hiding
simple store-to-load forwarding opportunities that are still obvious while
the stack-slot identity and offset are explicit.

A new pre-legalization pass tracks exact slot, offset, and type within each
block. It forwards only from non-address-taken slots, invalidates overlapping
writes, and leaves escaped slots alone. User stack maps are also treated as
external observations even when no `stack_addr` exists. Once every read from
an unobserved slot has been forwarded, its dead stores are removed and an
unkeyed empty slot is made size zero so it reserves no frame space. Focused
filetests cover interleaved offsets, overlapping writes, a clobbering call,
and GC stack-map preservation.

The `boxcar::Index::location` reduction exposed the immediate benefit. Its
interleaved `Option<usize>` discriminant and payload stores no longer block
payload forwarding. The temporary stack traffic, redundant nonzero niche
branch, jump table, and extra 16-byte frame allocation disappear. A companion
fold recognizes that shifting the single set bit in integer `1` can never
produce zero because CLIF masks shift counts to the value width.

The final 50-run matched gate measured the committed policy against the scalar
`NonZero` build:

| Workload | LLVM | Previous Cranelift | Stack forwarding | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 32.7 ms | 64.8 ms | 59.5 ms | -7.87% | 47/50 | 0.000000000037 |
| `uv lock --check`, offline | 59.4 ms | 96.6 ms | 91.2 ms | -5.63% | 45/50 | 0.0000000042 |
| `ruff check` over 1,592 fixtures | 79.0 ms | 180.6 ms | 160.1 ms | -10.80% | 45/50 | 0.0000000042 |
| `ty check` over `scripts/ty_benchmark` | 47.5 ms | 141.1 ms | 128.7 ms | -9.04% | 50/50 | 0.0000000000000018 |

The uv, Ruff, and ty binaries shrink by 2.16%, 2.84%, and 2.87%, respectively.
Correctness-probe exit codes and output digests match across LLVM, the previous
Cranelift build, and the candidate. The focused filetest, all 184
`cranelift-codegen` unit tests, and the complete stage-2 cg_clif and standard
library build pass. This is the largest broadly positive retained runtime step
in the investigation so far.

### Cross-block and width-changing stack forwarding

A later profile made `hashbrown::find_or_find_insert_index_inner` the hottest
active pure-Rust leaf. In its scalarized control-group comparison, eight byte
results were stored to adjacent stack offsets in one block and reloaded as one
`i64` in the next. The exact per-block forwarding rule could see neither the
control-flow relationship nor the width-changing equivalence.

The pass now carries known values into a block when it has exactly one distinct
predecessor and that predecessor has already been processed. Joins and
backedges without available state remain conservative. It can also assemble an
integer load of at most 64 bits from smaller integer values that completely and
uniquely cover the requested byte range. The shifts follow the target's native
endianness; gaps, overlaps, partial coverage, multiple predecessors, and
non-integer pieces leave the load intact. Focused x86-64 and s390x filetests
cover little- and big-endian assembly, and a diamond test confirms that state is
not propagated across a multi-predecessor join.

For the profiled hashbrown body, the eight byte stores and `i64` reload
disappear and the AArch64 frame drops from 112 to 80 bytes. The final 50-run
matched gate measured this extension against the 16-register stack-copy
policy:

| Workload | LLVM | Previous Cranelift | Cross-block forwarding | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 32.9 ms | 56.3 ms | 55.2 ms | -1.46% | 35/50 | 0.00660 |
| `uv lock --check`, offline | 58.6 ms | 85.9 ms | 85.5 ms | -0.90% | 28/50 | 0.47989 |
| `ruff check` over 1,592 fixtures | 79.7 ms | 152.6 ms | 148.2 ms | -2.54% | 39/50 | 0.000090 |
| `ty check` over `scripts/ty_benchmark` | 47.8 ms | 123.9 ms | 122.2 ms | -0.65% | 36/50 | 0.00260 |

The uv, Ruff, and ty binaries shrink by 0.83%, 0.72%, and 0.83%, respectively,
and every correctness digest matches. All 184 `cranelift-codegen` unit tests,
the focused filetests, cg_clif check, and the complete stage-2 cg_clif and
standard-library build pass. This is a smaller step than the original
forwarding pass, but it is unusually clean: every workload moves forward and
three of the four paired results are decisive.

The inverse width-changing rule was tested and rejected. It replaced a narrow
integer load from a covering wider stack value with a native-endian shift and
reduction. In the same hot hashbrown copy, the body shrank by 84 bytes and its
frame fell from 160 to 128 bytes. The binaries also shrank slightly: 0.07% for
uv and 0.09% for Ruff and ty. That local cleanup did not survive the 20-run
application screen. Paired changes were +0.30% for uv environment creation,
-0.55% for uv resolution, effectively zero for Ruff, and +0.20% for ty, with
9/20, 11/20, 10/20, and 10/20 wins respectively. All sign-test p-values were
at least 0.82. Forwarding wider loads from adjacent narrow values removes an
aggregate round trip; decomposing a wide value into scalar byte operations is
only a different spelling of work the application already performs.

### Boolean-result range folding

The next hot hashbrown reduction exposed an instruction-local range fact that
the egraph did not use. A scalar integer comparison produces only zero or one,
including after zero extension, yet later unsigned comparisons still checked
that value against constants outside that range. Cranelift now folds `ugt`,
`uge`, `ult`, and `ule` checks whose result is fixed for both possible values.
The rules are explicitly scalar: vector comparisons produce all-ones lane
masks, and a regression test confirms that those comparisons remain intact.

The final 50-run matched gate measured the scalar-safe rules against the
cross-block stack-forwarding policy:

| Workload | LLVM | Previous Cranelift | Boolean range folding | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 32.3 ms | 55.6 ms | 55.8 ms | +0.43% | 22/50 | 0.47989 |
| `uv lock --check`, offline | 59.1 ms | 84.9 ms | 85.4 ms | +0.38% | 23/50 | 0.67181 |
| `ruff check` over 1,592 fixtures | 80.2 ms | 148.0 ms | 148.2 ms | -0.10% | 26/50 | 0.88772 |
| `ty check` over `scripts/ty_benchmark` | 48.3 ms | 122.3 ms | 119.9 ms | -1.61% | 43/50 | 0.000000210 |

The uv, Ruff, and ty binaries shrink by 0.96%, 1.08%, and 1.04%, respectively.
Every correctness digest matches. The focused scalar and vector filetests, all
184 `cranelift-codegen` unit tests, cg_clif check, and complete stage-2 cg_clif
and standard-library build pass. The runtime result is concentrated in ty;
Ruff and both uv operations are statistically neutral rather than inheriting
the apparent gains from an earlier vector-unsafe experiment.

### Boolean-indexed branch-table lowering

The optimized CLIF for the same hot hashbrown body showed where three of the
remaining range checks came from. Rust had lowered boolean control flow to a
`br_table` indexed by a zero-extended scalar comparison. The comparison can
only produce zero or one, but AArch64 lowering still emitted a `cset`, a
comparison against two, a branch to the out-of-range trap, and an indirect
table branch.

The skeleton optimizer now resolves jump-table entries zero and one and
replaces this shape with a `brif` on the original comparison. The default
destination is unreachable by construction. The skeleton cost model prices a
branch table above an otherwise equivalent conditional branch so the rewrite
is selected. The match remains explicitly scalar, and the resolved
`BlockCall`s preserve destination arguments. In the profiled function, all
three comparison-derived table checks disappear, one separate two-entry Rust
discriminant table remains, and the body shrinks from 1,772 to 1,624 bytes
(-8.35%).

The 50-run matched gate measured the rewrite against scalar boolean-result
range folding:

| Workload | LLVM | Previous Cranelift | Boolean table lowering | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 32.7 ms | 55.7 ms | 55.6 ms | -0.34% | 28/50 | 0.47989 |
| `uv lock --check`, offline | 58.7 ms | 84.6 ms | 84.5 ms | -0.001% | 25/50 | 1.00000 |
| `ruff check` over 1,592 fixtures | 79.2 ms | 148.2 ms | 145.3 ms | -1.97% | 35/50 | 0.00660 |
| `ty check` over `scripts/ty_benchmark` | 47.1 ms | 119.1 ms | 115.7 ms | -3.19% | 41/50 | 0.0000056 |

The uv, Ruff, and ty binaries shrink by 2.07%, 2.46%, and 2.15%, respectively.
Every correctness digest matches. The focused branch-argument filetest, all
185 `cranelift-codegen` unit tests, cg_clif check, and complete stage-2 cg_clif
and standard-library build pass. The broad size reduction shows that this is a
common Rust lowering shape, while the runtime result is strongest in the two
hash-table-heavy workloads.

A follow-up AArch64 experiment lowered all one- and two-entry tables to direct
branches. It replaced the remaining hot indirect sequence with
`cmp; b.hs; cbz` and shrank the uv, Ruff, and ty binaries by another 0.68%,
0.67%, and 0.55%, respectively. The 20-run application screen nevertheless
rejected it: `uv venv --clear` regressed by 1.92% with 5/20 wins
(sign-test p=0.041), while `uv lock --check` improved by 0.53% with 12/20 wins
(p=0.503), Ruff changed by +0.02% with 10/20 wins, and ty changed by +0.31%
with 10/20 wins. Every correctness digest matched. Small-table encoding is
therefore not a current runtime arc despite its local code-size win.

A separate range-preservation experiment targeted the generic switch lowering
seen in ty's large `Type::hash` body. Rust had already zero-extended an
eight-bit discriminant before a later block used it, but the frontend passed
the widened value to Cranelift's `Switch`. Recovering the original narrow
source removed the generic greater-than-`u32::MAX` check and filtered explicit
cases outside the source type's proven unsigned range. It shrank uv, Ruff, and
ty by 0.17%, 0.23%, and 0.16%, respectively, and every correctness digest
matched.

The 50-run application gate found no runtime benefit, however. Paired changes
were +0.09% for `uv venv --clear` (24/50 wins, `p = 0.888`), -0.13% for
`uv lock --check` (26/50, `p = 0.888`), -0.09% for Ruff (25/50, `p = 1`),
and +0.54% for ty (20/50, `p = 0.203`). The frontend-specific rewrite was
therefore rejected. Carrying a narrow range only into generic switch lowering
is a code-size opportunity; the runtime arc needs to eliminate checks inside
hot loops or enable a larger control-flow transformation.

### Bounded stack-backed aggregate copies

The first profile after stack forwarding still showed Darwin's
`_platform_memmove` as the second-hottest active leaf. A dynamic size census
made the scale visible: the representative ty corpus executed 3,565,932,020
copy calls and moved about 450 GB, versus 119,334,944 calls and 17 GB in the
LLVM binary. The dominant Cranelift-only sizes were 120, 128, and 80 bytes.
Symbolicated callers showed repeated aggregate shuttling through temporary
stack slots; one 328-byte `Default::default` body made nine separate 120-byte
libc calls.

cg_clif now retains a constant non-overlapping copy as explicit loads and
stores when either endpoint is a known stack slot and the copy needs at most
16 registers. Loads still precede every store, preserving overlap safety for
the shared implementation. Arbitrary-pointer copies retain the separately
profiled eight-register frontend ceiling. Keeping stack identity explicit lets
the pre-legalization forwarding pass collapse chains that were previously
hidden behind `stack_addr` and a libcall.

On the same ty corpus, dynamic copy calls fell to 1,056,739,242 (-70.37%) and
bytes moved fell to about 197 GB (-56.24%). The 120- and 128-byte libc buckets
disappeared entirely. The 50-run matched gate measured the retained policy
against stack forwarding alone:

| Workload | LLVM | Stack forwarding | Stack-backed copies | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 30.9 ms | 59.6 ms | 55.6 ms | -5.98% | 45/50 | 0.0000000042 |
| `uv lock --check`, offline | 58.4 ms | 90.7 ms | 85.4 ms | -5.63% | 47/50 | 0.000000000037 |
| `ruff check` over 1,592 fixtures | 79.2 ms | 158.8 ms | 153.9 ms | -3.23% | 42/50 | 0.0000012 |
| `ty check` over `scripts/ty_benchmark` | 47.7 ms | 129.2 ms | 122.9 ms | -4.76% | 50/50 | 0.0000000000000018 |

The uv, Ruff, and ty binaries shrink by 1.64%, 1.96%, and 1.94%, respectively,
and all correctness digests match. The standard cg_clif example exercises
80-, 120-, and 128-byte stack copies and runs successfully with the stage-2
backend.

The next profile-supported boundary was tested and rejected. Raising the
ceiling to 20 registers grew uv, Ruff, and ty by 0.25%, 0.19%, and 0.14%.
Against 16 registers it regressed uv environment creation by 1.19% (5/20 wins,
`p = 0.04139`) and uv resolution by 1.40% (4/20, `p = 0.01182`); Ruff and ty
were statistically neutral. Sixteen registers is therefore the retained
stack-specific ceiling.

A narrower follow-up isolated the hottest newly admitted bucket instead of
raising the ceiling. Residual 152-byte stack-backed copies execute about 54.6
million times in the amplified ty corpus and require 19 registers, so the
candidate admitted exactly 19-register copies while still rejecting 17, 18,
and 20. It grew uv, Ruff, and ty by only 0.06%, 0.08%, and 0.06%, but the
20-run screen still regressed uv resolution by 1.83% with 5/20 wins
(`p = 0.04139`). The paired changes for uv environment creation, Ruff, and ty
were statistically neutral at -0.30%, -0.12%, and -0.47%. The isolated bucket
was rejected. The 16-register boundary is not merely hiding one profitable
larger size; the residual hot copies need a loop, vector, or forwarding
strategy that does not materialize nineteen scalar load/store pairs at every
call site.

An exact-size vector variant tested that alternative directly. It represented
152 bytes as nine 16-byte vector values and one 8-byte tail, cutting the
expanded value count from 19 to 10 while leaving every smaller retained copy
unchanged. Binary growth fell to 0.04% for uv, 0.06% for Ruff, and 0.03% for
ty, but the 20-run paired changes were +0.17%, +1.58%, +0.84%, and -0.10% for
uv environment creation, uv resolution, Ruff, and ty. No movement was
statistically decisive and three workloads moved backward, so the vector
variant was also rejected. The residual 152-byte traffic needs elimination or
loop-level reuse rather than a differently shaped call-site expansion.

### Rejected native SIMD comparison expansion

An amplified ty profile showed scalarized AArch64 hash-table control-group
comparisons. A broad cg_clif experiment lowered integer comparisons over
8-byte and 16-byte vectors directly to Cranelift vector `icmp`, and used native
or SWAR high-bit packing for the corresponding `simd_bitmask` operations. The
50-run gate exposed a real but unacceptable tradeoff:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +2.43% | 15/50 | 0.00660 |
| `uv lock --check`, offline | +1.45% | 15/50 | 0.00660 |
| `ruff check` over 1,592 fixtures | -0.75% | 30/50 | 0.20264 |
| `ty check` over `scripts/ty_benchmark` | -1.81% | 39/50 | 0.00009 |

Correctness probes matched and all three binaries became smaller, but both uv
regressions are decisive, so the broad lowering is rejected.

A compile-time census then proved that the applications instantiate only the
shapes covered by three narrower controls: 8-lane hash-group equality and
signed-less-than, 8-lane signed-greater-than-or-equal full-bucket scans, and
16-lane byte equality plus bitmask extraction for core substring search. The
first control was neutral in a 50-run gate (-0.25% to +0.05%, all sign-test
`p >= 0.67`). The signed-greater-than-or-equal control was also neutral at 50
runs, with changes from +0.15% to +1.40% and no sign-test below 0.0649. The
16-lane substring control was neutral in its 20-run screen (-0.34% to +0.92%,
all `p >= 0.26`). This rules out a missing fourth comparison shape: the broad
ty improvement and uv regressions arise from the combined code-layout and
lowering change, not an independently profitable operation.

### Rejected backend-specific UB-check inlining

Rust's unsafe-precondition helpers are deliberately `#[rustc_no_mir_inline]`
because LLVM can fold the guard and inline their bodies later. Cranelift has no
mature general-purpose later inliner, and these helpers appeared prominently in
the amplified ty profile. A backend-specific experiment allowed only `#[inline]`,
`#[track_caller]`, non-unwinding functions named `precondition_check` to bypass
the attribute. It reduced retained helper copies from 627 to 584 in Ruff, 542
to 502 in ty, and 1,161 to 1,103 in uv, but grew the binaries by 0.10%, 0.37%,
and 0.12% respectively.

The 20-run gate moved every workload in the wrong direction: uv environment
creation +1.29%, uv resolution +0.85%, Ruff +0.49%, and ty +0.67%, with no
sign-test below 0.26. Forcing every otherwise-legal helper past the MIR cost
threshold did not remove any additional retained copies and produced another
neutral-to-negative screen (+0.13% to +1.54%). Both variants are rejected.
The result is useful attribution: these sampled helpers are symptoms of the
larger call, range, and code-layout gap, not a profitable isolated exception to
`rustc_no_mir_inline`. The later bounded CLIF inliner deliberately excludes
these call-bearing helpers.

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

## Full-Suite Backend Validation

The 60/500/100 candidate also ran the repositories' complete macOS test
commands from clean target directories under both LLVM and Cranelift. These are
single-run operational timings, not block-balanced runtime benchmarks: they
mix compilation, thousands of short subprocesses, filesystem work, and test
execution. They are useful as a broad correctness gate and as a directional
measure of the entire developer operation.

| Suite and boundary | LLVM | Cranelift | Cranelift change | Outcome in both lanes |
| --- | ---: | ---: | ---: | --- |
| uv cold build + Nextest | 579.10 s | 608.28 s | +5.0% | 3,866 tests run: 3,773 passed, 90 failed, 3 timed out, 3 skipped |
| uv Nextest phase | 527.251 s | 499.409 s | -5.3% | Same exact failing-test set |
| Ruff/ty cold build + Nextest | 197.77 s | 201.29 s | +1.8% | 7,962 tests run: 7,960 passed, 2 failed, 44 skipped |
| Ruff/ty Nextest phase | 95.414 s | 97.383 s | +2.1% | Same two failing tests |
| Ruff/ty cold build + doctests | 75.23 s | 98.55 s | +31.0% | 194 passed, 12 ignored, 0 failed across 48 crates |

The pinned uv snapshots have drifted under the current SRS compiler and host
environment. Both lanes therefore used scratch worktrees with Insta updates
forced to pass so all tests could execute. The 90 failure names and three
timeouts are identical, and the tracked snapshot diff is byte-identical;
parallel warning order and the local registry URL still vary in untracked
pending-snapshot metadata. Ruff/ty used the same scratch policy but left both
worktrees clean. Its shared failures are the format error-diagnostics test and
the deliberately panicking markdown rule, which aborts while initiating its
panic on this host. No Cranelift-only failure appeared in either repository.

The uv execution-phase result is I/O-heavy and comes from one full run, so its
apparent Cranelift lead should not be treated as an application-runtime win.
Conversely, the doctest operation launches many compiler processes and shows a
substantial cold rustdoc/build gap. The balanced application matrix above
remains the decision gate for generated-code runtime.

The subsequent 500-cross-crate/60/600/100 policy passed a complete two-stage
SRS build, all 18 `rustc_interface` unit tests, a matching standalone cg_clif
backend build, and cg_clif's no-sysroot AOT smoke. The depth-8 policy also
passed the complete SRS build and all 18 interface tests. The subsequent
target-feature policy passed another complete SRS build and its dedicated
AArch64 MIR test. The current depth-12 policy passed a complete SRS build and
all 18 interface tests. The small-copy policy passed a complete SRS build and
its dedicated cg_clif AOT regression probe. The current 800-budget policy
passed a complete SRS build and all 18 interface tests. The subsequent bounded
CLIF inliner passed a complete two-stage SRS build, a standalone stage-1 backend
check, pinned uv/Ruff/ty builds, and the 50-run correctness probes. The
subsequent eight-register small-copy policy passed its focused frontend tests
and another complete SRS build. The constant-size byte-fill policy passed its
focused frontend tests, cg_clif check, and a complete SRS build. Every complete
runtime gate preserved every correctness digest. The scalar `NonZero` change
then passed its focused regression and complete stage-2 backend and standard
library build. The stack-forwarding policy passed focused escaped-slot,
overlap, and stack-map tests, all 184 `cranelift-codegen` unit tests, another
complete stage-2 build, and the matched correctness probes. A fresh full
uv/Ruff/ty suite pair is reserved for the finalization gate rather than
repeated after every retained optimization. The bounded stack-copy policy
passed its stage-2 backend and standard-library build, direct standard-example
execution, application correctness probes, and 50-run runtime gate. The
scalar boolean-range and boolean-indexed branch-table policies passed focused
scalar, vector, and branch-argument filetests, all 185 `cranelift-codegen` unit
tests, cg_clif check, complete stage-2 backend and standard-library builds, and
matched 50-run correctness probes. The current cg_clif sysroot harness cannot
provide additional coverage: its stdlib
patch no longer applies to this Rust snapshot, and its standalone JIT smoke
aborts in rustc query TLS before entering the test program.

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
Boolean-result folding is the first retained range-reasoning step. It proves a
small instruction-local fact through zero extension and improves ty by 1.61%,
but it does not carry facts through block parameters or joins. Boolean-indexed
branch-table lowering consumes the same fact at a control-flow boundary,
removes three bounds checks from the profiled hashbrown body, improves Ruff by
1.97% and ty by 3.19%, and shrinks all three application binaries by more than
2%. One separate two-entry discriminant table remains in that body, but direct
small-table lowering lost the application gate despite shrinking code.
Propagating scalar ranges through CFG edges and joins remains the next concrete
step before affine loop-index analysis, but the rejected switch experiment
narrows the target: preserving a range solely to remove a generic switch
overflow check did not move runtime. The next candidate should consume the
fact in repeated loop bounds or enable a larger control-flow simplification.
The reduced scan confirms that vectorization and loop-wide range reasoning
remain the larger opportunities.

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
- import small post-monomorphization bodies across codegen units instead of
  repeatedly raising the local hinted-call threshold;
- reduce and fix the broader cg_clif inliner miscompile before admitting
  callees with stack slots, global values, nested calls, or larger control-flow
  graphs;
- investigate the open work to let
  [regalloc manage callee-saved registers](https://github.com/bytecodealliance/wasmtime/issues/7727);
- remove [unused stack slots](https://github.com/bytecodealliance/wasmtime/issues/6661)
  and [dead stores](https://github.com/bytecodealliance/wasmtime/issues/4167); and
- pass or retain stack arguments without unnecessary entry-block loads where
  [possible](https://github.com/bytecodealliance/wasmtime/issues/6301).

The application gate is important here: the target-feature policy helped ty
but regressed Ruff, while the subsequent depth increase recovered Ruff without
giving back the ty win. Their hot paths will not all improve merely by buying
more code size. Keeping thin
`NonNull` and `NonZero` values in SSA, forwarding exact explicit stack-slot
traffic, and exposing bounded stack-backed aggregate copies complete several
high-impact local cases. Carrying nonescaped stack state through
single-predecessor blocks and assembling adjacent narrow values completes the
profiled hashbrown control-group case without the uv regressions from broad
SIMD lowering. Folding impossible unsigned checks on scalar comparison results
adds the first bounded value-range rule and produces a decisive ty improvement
without a material uv or Ruff regression. Replacing branch tables indexed by
those comparison results with direct conditional branches then removes the
corresponding bounds checks, decisively improves Ruff and ty, and cuts another
2.1-2.5% from binary size without moving uv. The forwarding pass also
implements the safe nonescaped subset of the older unused-slot and dead-store
roadmap items. A fresh matched profile still shows hashbrown's
`find_or_find_insert_index_inner` and cross-CGU `FxHasher::write_isize` as
prominent Cranelift-only leaves while LLVM has absorbed them into callers.
An untargeted, one-level post-monomorphization body-import experiment removed
all of the profiled `FxHasher::write_isize` calls but produced a mixed runtime
screen and was rejected. The immediate call arc is now a profile- or
frequency-aware import policy, not broader body availability by itself.
The remaining 1.4-2.5x LLVM gap shows that local representation fixes alone
will not close the runtime difference.

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
