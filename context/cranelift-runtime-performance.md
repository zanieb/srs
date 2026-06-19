# Cranelift-Generated Rust Runtime Performance

## Scope

This investigation measures the runtime of native Rust binaries produced by
Cranelift, not Cranelift compile time. LLVM and Cranelift use the same SRS
rustc, Cargo, sources, profile, target CPU, and linker. Only target codegen
changes.

The latest comparison was collected on aarch64 macOS from the source tree now
recorded at SRS `5b3d2b69c`, with:

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

Each row is 50 timed trials after 10 warmups. Lower is better. LLVM, the
preceding Cranelift policy, and the current Cranelift policy were run in the
same randomized, block-balanced matrix from freshly built binaries.

| Workload | LLVM | Current Cranelift | LLVM lead |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | 30.9 ms | 65.3 ms | 2.12x |
| `uv lock --check`, offline | 57.8 ms | 98.0 ms | 1.70x |
| `ruff check` over 1,592 fixtures | 81.0 ms | 204.7 ms | 2.53x |
| `ty check` over `scripts/ty_benchmark` | 47.5 ms | 150.1 ms | 3.16x |

The uv lock fixture is an intentional offline failure caused by the pinned
checkout's unavailable resolver data. The ty fixture reports the same four
unresolved imports in every successful compiler lane. These rows measure the
real resolver and type-checking failure paths, not a no-op command.

The conclusion is unambiguous: LLVM is currently about 1.7-3.2x faster for
these user-visible operations. The implemented changes recover meaningful
ground for Ruff and ty and a smaller amount for uv lock, but they do not close
the remaining loop-optimization and code-quality gap.

### Binary size

The profiling binaries retain full debuginfo, so these are comparative rather
than distribution sizes.

| Binary | LLVM | Cranelift baseline | Current Cranelift |
| --- | ---: | ---: | ---: |
| Ruff | 45.0 MB | 135.8 MB | 128.0 MB |
| ty | 47.5 MB | 145.6 MB | 133.2 MB |
| uv | 100.0 MB | 255.0 MB | 256.8 MB |

Cranelift's much larger output is consistent with missed inlining, folding,
dead-code elimination, and code-layout opportunities. More inlining is not a
complete answer: the final policy reduces Ruff and ty size, but the remaining
output is still roughly three times LLVM's and uv remains nearly neutral on
environment creation.

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
check, pinned uv/Ruff/ty builds, and the 50-run correctness probes. Every
complete runtime gate preserved every correctness digest. A fresh full
uv/Ruff/ty suite pair is reserved for the finalization gate rather than
repeated after every retained optimization. The current cg_clif sysroot
harness cannot provide
additional coverage: its stdlib patch no longer applies to this Rust snapshot,
and its standalone JIT smoke aborts in rustc query TLS before entering the test
program.

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
`NonNull` iterator cursors in SSA completes one high-impact stack-slot case,
but the remaining 2-4x LLVM gap shows that local representation fixes
alone will not close the runtime difference.

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
