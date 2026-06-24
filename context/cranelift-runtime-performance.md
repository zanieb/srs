# Cranelift-Generated Rust Runtime Performance

## Scope

This investigation measures the runtime of native Rust binaries produced by
Cranelift, not Cranelift compile time. LLVM and Cranelift use the same SRS
rustc, Cargo, sources, profile, target CPU, and linker. Only target codegen
changes.

The final comparison was collected on dedicated, unsandboxed remote x86_64
Linux and Apple-silicon macOS hosts from SRS backend source `7497012ee`, with:

- rustc `1.97.0-dev (9e2ac2b97 2026-05-21)`;
- Cranelift `0.133.0`;
- uv `6c963dd3cb0e`;
- Ruff/ty `e0eb28d6345b`;
- the repositories' `profiling` profile, with LTO disabled for both backends;
- `CARGO_INCREMENTAL=0`;
- the SRS artifact cache and incremental linker disabled;
- cg_clif's opt-in unwinding support enabled on both targets;
- the same linker within each host's LLVM and Cranelift pair; and
- fresh, backend-specific target directories.

Ruff's production release profile uses fat LTO. Disabling it makes the
backend comparison possible and controlled, but probably makes the measured
LLVM lead conservative relative to the shipped Ruff binary.

## Representative Results

Each row is 50 timed paired trials after ten warmups. Lower is better. The
pinned LLVM and Cranelift lanes ran in the same seeded, block-balanced schedule
on an otherwise idle remote host and were built from fresh target directories.

| Host | Workload | LLVM median | Cranelift median | Median ratio | Paired Cranelift change |
| --- | --- | ---: | ---: | ---: | ---: |
| Linux x86_64 | `uv venv --clear` | 31.76 ms | 63.82 ms | 2.01x | +101.03% |
| Linux x86_64 | `uv lock --check`, offline | 65.96 ms | 113.97 ms | 1.73x | +76.14% |
| Linux x86_64 | `ruff check` over 1,592 fixtures | 165.83 ms | 419.48 ms | 2.53x | +152.39% |
| Linux x86_64 | `ty check` over `scripts/ty_benchmark` | 119.57 ms | 364.44 ms | 3.05x | +204.84% |
| macOS arm64 | `uv venv --clear` | 52.48 ms | 58.27 ms | 1.11x | +25.38% |
| macOS arm64 | `uv lock --check`, offline | 100.60 ms | 101.87 ms | 1.01x | +3.93% |
| macOS arm64 | `ruff check` over 1,592 fixtures | 106.50 ms | 175.84 ms | 1.65x | +64.73% |
| macOS arm64 | `ty check` over `scripts/ty_benchmark` | 51.61 ms | 157.84 ms | 3.06x | +203.07% |

The paired column is the primary statistic because the macOS host showed
substantial frequency and thermal drift across the absolute samples. Linux is
stable and severe: Cranelift loses all 50 pairs in every row (`p = 1.78e-15`).
On macOS it loses all 50 Ruff and ty pairs, 45/50 uv environment pairs, and
29/50 uv lock pairs. The lock row is neutral (`p = 0.32224`); the other three
are decisive.

The uv lock fixture succeeds in both lanes after the stateful probe is primed.
The ty fixture reports the same four unresolved imports and exit status in
both lanes. Normalized exit codes and stdout/stderr digests match for every
probe on both hosts.

The conclusion is unambiguous: the user's understanding is correct. LLVM is
still substantially faster for the parser- and analysis-heavy Ruff and ty
operations, with ty just over 3x faster on both architectures. Linux also
shows 1.7-2.0x LLVM leads for the uv operations. macOS uv lock is the one
neutral row, while environment creation remains directionally slower under
Cranelift. The retained changes recover meaningful ground in profiled hot
paths but do not close the loop, range, call-boundary, and code-layout gap.

The final raw matrices are saved at
`/Users/zanie/code/tmp/cranelift-remote-final/linux-final-targeted/runtime.json`
and
`/Users/zanie/code/tmp/cranelift-remote-final/macos-final-no-weak/runtime.json`.

### Final acceptance gate

The final gate is the complete repository test suite under matched LLVM and
Cranelift toolchains, not just the four timing operations. The final policy ran
the complete uv, Ruff/ty, and doctest commands on remote x86_64 Linux and
Apple-silicon macOS hosts. Both macOS lanes are clean. Linux Ruff/ty and
doctests are clean, and the complete Linux uv lanes have the exact same 34
host-service and reflink failure names. The commands and results are recorded
under [Full-Suite Backend Validation](#full-suite-backend-validation).

The balanced application matrix remains the runtime decision boundary because
the complete suites also include compilation, network and filesystem work,
fixed timeouts, and thousands of short subprocesses. All retained performance
and finalization policies are included in this matched final gate.

### Binary size

The profiling binaries retain full debuginfo, so total file size is not a useful
cross-platform comparison. These are final machine-text section sizes.

| Host | Binary | LLVM text | Cranelift text | Cranelift / LLVM |
| --- | --- | ---: | ---: | ---: |
| Linux x86_64 | uv | 77.76 MB | 147.58 MB | 1.90x |
| Linux x86_64 | Ruff | 31.97 MB | 75.18 MB | 2.35x |
| Linux x86_64 | ty | 29.37 MB | 76.16 MB | 2.59x |
| macOS arm64 | uv | 39.48 MB | 84.00 MB | 2.13x |
| macOS arm64 | Ruff | 19.30 MB | 43.88 MB | 2.27x |
| macOS arm64 | ty | 18.56 MB | 39.44 MB | 2.13x |

Cranelift's much larger output is consistent with missed inlining, folding,
dead-code elimination, and code-layout opportunities. Correctly enabling
cg_clif unwinding on Apple silicon initially added 20.9% to Ruff, 20.0% to ty,
and 23.5% to uv relative to the same policy without exception tables.
Restricting personality and LSDA metadata to catching functions recovers
5.7-6.1% of the complete ELF binaries without changing machine text. Weakly
coalescing CGU-local copies then recovers another 13.6-33.8% on ELF without
exporting those symbols. Mach-O cannot retain that coalescing until its FDE and
LSDA atoms follow the winning weak text atom. More inlining is not a complete
answer: final machine text remains 1.9-2.6 times LLVM's.

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

Three controls bounded the policy:

- Raising only the cross-crate threshold from 100 to 200 on top of the
  60/500/100 policy added no measurable Ruff or ty benefit and slightly grew
  both binaries, so that candidate was rejected.
- Raising the forwarder and post-depth fallback from 60 to 100 targeted the 25
  remaining `Hasher::write_isize` calls in ty's active `Type::hash` body. The
  calls remained, although other newly admitted forwarders shrank Ruff and ty
  by 0.13% and 0.11%. A focused 20-run screen measured Ruff at +0.02% with
  10/20 wins (`p = 1`) and ty at +0.65% with 7/20 wins (`p = 0.26318`), so the
  broader fallback was rejected without building the uv lane.
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

The retained follow-up supplies a bounded static signal: an imported body is
eligible only when the same caller contains at least four direct calls to that
callee. Ordinary local scalar inlining is unchanged. This targets repeated
enum hashing without importing bodies across every one-off call boundary.
In ty, direct `FxHasher::write_isize` calls fall from 385 to 179, and every hot
`Type::hash` copy falls from 25 calls to zero. The broader rejected experiment
removed all 385 calls; the repeated-call policy deliberately leaves the
one-off sites alone. Binary size changes are correspondingly tiny: uv, Ruff,
and ty shrink by 0.0031%, 0.0011%, and 0.0018%.

The 50-run matched gate measured:

| Workload | Candidate vs previous Cranelift | Wins | Sign p | Candidate vs LLVM |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.49% | 30/50 | 0.20264 | +69.9% |
| `uv lock --check`, offline | -0.09% | 26/50 | 0.88772 | +43.7% |
| `ruff check` over 1,592 fixtures | -0.63% | 28/50 | 0.47989 | +80.9% |
| `ty check` over `scripts/ty_benchmark` | -0.99% | 30/50 | 0.20264 | +144.5% |

No row is individually decisive, but all four paired medians improve, every
binary shrinks, and all correctness digests match. The focused backend check
and complete stage-2 backend and standard-library build pass. The repeated-call
policy is retained for nontrivial bodies; the constant-like one-off exception
below is deliberately smaller than a general call-site signal.

Allowing one-off hinted imports only when the composed body had at most eight
CLIF instructions tested the smallest plausible exception. It still grew uv,
Ruff, and ty by 16,928, 7,104, and 4,496 bytes. In a 50-run matched gate, uv
environment creation regressed by 2.32% with 13/50 wins (`p = 0.00094`), while
uv resolution (+0.05%), Ruff (-0.87%), and ty (-0.04%) were neutral. All
correctness digests matched. The exception is rejected: even tiny one-off
imports perturb enough layout to hurt uv, so repeated calls remain the minimum
static hotness signal.

A later profile-derived exception allowed otherwise eligible one-off imports
only when the exact call instruction was inside a natural loop. The policy
shrunk ty by 43,456 bytes, but it did not inline the target
`RawTableInner::find_insert_index_in_group` call: that callee still has a live
16-byte aggregate-return stack slot and therefore fails the existing
stack-free safety boundary. The 20-run ty screen was neutral at -0.02% paired,
11/20 wins, and `p = 0.82380`. The exception is rejected without broadening the
application gate. The target call boundary first needs a stack-free callee;
loop membership alone is not enough reason to perturb every eligible call.

A direct-tag integer `Option<T>` return experiment then kept the pair ABI
result in SSA when MIR proved the return place's address was unobserved. It
removed `find_insert_index_in_group`'s 16-byte return slot and stores, shrinking
that helper from 140 to 120 bytes. The caller nevertheless retained both the
call and its 80-byte frame. The broader representation change shrank uv by
40,960 bytes, Ruff by 875,728 bytes, and ty by 1,986,704 bytes, but the 20-run
application gate moved backward: uv environment creation +1.22% with 6/20
wins (`p = 0.11532`), uv resolution +1.09% with 7/20 wins (`p = 0.26318`),
Ruff +0.17% with 10/20 wins (`p = 1.0`), and ty +0.72% with 7/20 wins
(`p = 0.26318`). All correctness digests matched and the complete stage-2
backend and standard-library build passed. The experiment is rejected despite
the code-size win: scalarizing the callee return does not by itself remove the
profiled boundary or improve application runtime. Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/bench-direct-option-pair-return-all20/results.json`.

Combining the two rejected experiments finally removed the profiled call. A
narrow `Option<usize>` return path supplied the stack-free pair, indirect
scalar-pair constants were decomposed to remove the `None` global, and a
loop-only tier pre-optimized bounded local hinted bodies before applying the
existing 32-live-instruction safety checks. The helper's runtime MIR has ten
blocks and 19 operations; optimized CLIF has nine blocks and 29 live
instructions. The helper symbol and call disappeared from ty, but the caller
retained its 80-byte frame. uv, Ruff, and ty shrank by 429,488, 1,184,992, and
2,389,344 bytes. A promising 20-run ty screen (-0.73%, 15/20 wins,
`p = 0.04139`) did not survive the 50-run four-application gate:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -0.38% | 28/50 | 0.47989 |
| `uv lock --check`, offline | -0.37% | 26/50 | 0.88772 |
| `ruff check` over 1,592 fixtures | +0.01% | 25/50 | 1.0 |
| `ty check` over `scripts/ty_benchmark` | -0.05% | 27/50 | 0.67181 |

All correctness digests matched and the complete stage-2 backend and
standard-library build passed. The combined policy is rejected: even removing
the exact hot loop call is runtime-neutral while its caller frame and stack
traffic remain. Further work on this body should eliminate that traffic rather
than broaden inlining. Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/bench-loop-option-usize-all50/results.json`.

Promoting every non-address-observed direct `Option<usize>` local to a pair of
SSA variables tested the caller traffic directly, without changing inlining
policy. The hot caller stopped storing and reloading the result at stack
offsets 48 and 56, and its local allocation fell from 80 to 64 bytes. Register
allocation used an additional x27/x28 callee-saved pair, however, leaving the
total frame footprint unchanged. The candidate shrank uv, Ruff, and ty by
584,336, 1,293,328, and 2,537,680 bytes. A strong ty-only screen (-1.58%,
16/20 wins, `p = 0.01182`) again washed out in the 50-run application gate:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -0.25% | 26/50 | 0.88772 |
| `uv lock --check`, offline | +0.64% | 21/50 | 0.32224 |
| `ruff check` over 1,592 fixtures | -1.84% | 30/50 | 0.20264 |
| `ty check` over `scripts/ty_benchmark` | -0.02% | 25/50 | 1.0 |

All correctness digests matched and the complete stage-2 build passed. This
representation is rejected for runtime: it trades stack loads and stores for
callee-saved register pressure rather than reducing the complete frame. A
future aggregate-promotion attempt needs to reduce the vector and probe-state
traffic together so the promoted values fit without another saved register
pair. Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/bench-option-usize-local-ssa-all50/results.json`.

An even narrower one-off exception is retained for bodies that become trivial
forwarders or constructors only after monomorphization. Unhinted source MIR is
considered only with at most two blocks and four operations. After importing
one dependency level and composing it, the body must satisfy the existing
call-free, global-free, dynamic-stack-free bounds and contain at most one
instruction other than `nop`, unconditional jump, or return. These trivial
imports bypass the four-calls-per-caller requirement; ordinary hinted imports
still use the repeated-call signal.

The missing legality fact was an already dead return slot. Cranelift forwarded
the temporary return traffic but left a zero-byte stack-slot record, while
cg_clif inspected the unoptimized catalogue and rejected any slot metadata.
cg_clif now pre-optimizes only catalogue bodies that have stack slots and keeps
the optimized form only when every slot is proven empty. Functions with live
stack state remain ineligible, and all previously eligible stack-free bodies
retain their old CLIF. Pre-optimizing the entire catalogue was an important
negative control: it removed the target calls and shrank ty by 1.31%, but
regressed the ty workload by 1.37% paired in a 20-run screen.

The bounded combination removes all 54 direct ty calls to the profiled
`BuildHasherDefault<FxHasher>::build_hasher` forwarder. The complete 50-run
three-lane gate measured:

| Workload | Candidate vs previous Cranelift | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.49% | 31/50 | 0.11892 | +63.8% | -1.107% |
| `uv lock --check`, offline | -0.49% | 27/50 | 0.67181 | +41.2% | -1.107% |
| `ruff check` over 1,592 fixtures | -0.84% | 26/50 | 0.88772 | +78.3% | -2.368% |
| `ty check` over `scripts/ty_benchmark` | -1.41% | 34/50 | 0.01535 | +124.1% | -3.773% |

ty improves decisively, the other three rows are neutral with favorable
paired medians, every binary shrinks substantially, and all application exit
codes and output digests match. The complete stage-2 backend, standard-library,
and proc-macro build and cg_clif check pass. The full matched repository-suite
rerun remains the finalization gate rather than a prerequisite for retaining
this independently bounded policy.

A second threshold probe targeted callable `dyn Fn` shims. Raising the hinted
budget from 800 to 850 retained all 31 hot hashbrown probe copies; 900 and
1,000 reduced the count to 20 but did not improve the application gate and
worsened ty. A one-codegen-unit upper-bound build reduced ty's size by 9.5%
but left the profiled `boxcar` helper count unchanged and only moved the probe
count from 31 to 30. This reinforces the same roadmap conclusion: further
progress needs cross-crate body import and stronger post-monomorphization
optimization, not another local threshold increase.

A later post-fix profile made the `boxcar` boundary more concrete. The retained
ty binary had 253 direct calls to `Index<58>::location` spread across ten
CGU-local copies; the hottest copy had 184 callers, even though the generated
wrappers generally called it only once each. `Index<58>::new_unchecked` had
another 137 direct calls across nine copies. This is exactly the shape that the
four-calls-in-one-caller import rule cannot detect.

A module-wide experiment therefore admitted an unhinted import only after at
least eight direct calls across the CGU. The broad form left all 253 `location`
calls in place and grew ty by 1,879,280 bytes (1.69%). Restricting the exception
to indirect aggregate returns with one replaceable cold failure edge still left
all 253 calls and grew ty by 1,946,160 bytes (1.75%); only eight incidental
`new_unchecked` calls disappeared. The original `location` CLIF explains the
miss: before normal optimization it has nine blocks, four sized stack slots,
and two cold `unwrap_failed` calls. Catalogue preparation did not make it
eligible under the existing stack-free and single-failure-edge boundary.

Both module-frequency variants are rejected before timing or multi-application
builds because they failed the structural target and caused large code growth.
The complete stage-2 backend, standard-library, and proc-macro build passed.
The final narrowed backend is saved as
`/Users/zanie/code/tmp/cranelift-runtime-performance/backends/frequent-sret-guarded-import/candidate.dylib`
with SHA-256
`5ba7c6bad11259b8a33e138e20ae1f9b46e220ac2f1d03502c3cd5ebc3520e2d`.
Revisiting this cluster requires a representation change that makes the helper
stack-free before import; weakening the catalogue safety boundary or importing
all frequent one-off wrappers is not promising.

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

Constant overlapping copies do not justify applying that retained ceiling to
`memmove`, either. A fresh fast census measured 2,384,084 copy calls in ty
versus 267,845 under LLVM. Preserving a constant `copy` intrinsic count and
expanding it with the existing overlap-safe load-before-store sequence removed
687,429 calls, mostly reducing the eight-byte bucket from 736,833 to 72,206.
It nevertheless grew uv, Ruff, and ty by 19,104, 15,744, and 12,272 bytes, and
the complete 20-run screen moved every workload backward: uv environment
creation +2.43%, uv resolution +2.43%, Ruff +1.52% (5/20 wins,
`p = 0.04139`), and ty +0.72%. The experiment is rejected. Residual
overlapping-copy work needs target-aware lowering or elimination of the copy;
replacing Darwin's tuned `memmove` with generic scalar loads and stores is not
a win even for the hot constant sizes.

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

A loop-header extension tested the next conservative CFG step. It added at
most four integer block parameters only when a header had exactly two incoming
`jump` or `brif` edges, exactly one was a backedge, both immediate predecessor
blocks explicitly stored the same nonescaping stack location, and every
existing address-escape rule still passed. A focused loop filetest eliminated
the complete 16-byte stack slot and carried both values through block
parameters; the existing non-loop diamond remained unchanged.

The full ty build shrank by 17,600 bytes, but the profiled
`RawTableInner::find_or_find_insert_index_inner` body remained byte-for-byte
unchanged, including its 80-byte frame and loop index/stride loads and stores.
That loop's incoming values are defined in the predecessor region rather than
by stores in both immediate predecessor blocks. A 20-run ty screen was neutral:
`+0.25%` paired, 9/20 wins, two-sided sign-test `p = 0.82380`. The bounded
extension is rejected. A future stack-to-SSA attempt needs a real fixed-point
reaching-value analysis through the loop's predecessor region; adding block
parameters at progressively broader syntactic joins has no demonstrated hot
consumer.

A complementary acyclic extension did survive the application gate. When a
join has exactly two incoming `jump` or `brif` edges from distinct blocks, both
immediate predecessors explicitly store the same nonescaping integer stack
location, and all existing address-escape checks pass, the pass adds at most
four block parameters and forwards subsequent loads from them. The bound keeps
register-pressure risk small and the exact two-edge requirement avoids trying
to synthesize arguments for exception or jump-table control flow.

That last restriction is correctness-critical. The first prototype counted
only eligible predecessor edges and therefore added parameters to blocks that
also had `br_table` or `try_call` inputs, producing verifier failures when
those uncounted edges supplied no arguments. The retained implementation
counts every branch destination and skips the join unless both of its only two
incoming edges are eligible. Focused tests cover the positive diamond, a
predecessor with no matching store, and joins reached by both `br_table` and
`try_call`.

The original hot `hashbrown::find_or_find_insert_index_inner` body remained
byte-for-byte unchanged, including its 80-byte frame and terminal result
stores. The optimization instead generalized broadly enough to shrink all
three applications and produced a decisive Ruff improvement in the 50-run
matched gate:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -0.33% | 27/50 | 0.67181 |
| `uv lock --check`, offline | -0.70% | 28/50 | 0.47989 |
| `ruff check` over 1,592 fixtures | -2.11% | 40/50 | 0.0000239 |
| `ty check` over `scripts/ty_benchmark` | -0.21% | 32/50 | 0.06491 |

The uv, Ruff, and ty binaries shrink by 343,568 bytes (0.19%), 101,744 bytes
(0.09%), and 34,688 bytes (0.03%), respectively. Every correctness-probe exit
code and output digest matches. All 186 `cranelift-codegen` unit tests, all
1,218 CLIF filetests, the focused regression file, Clippy for
`cranelift-codegen`, and the complete stage-2 backend and standard-library
build pass. The complete measurement record is in
`/Users/zanie/code/tmp/cranelift-runtime-performance/bench-two-way-stack-join-all50/results.json`.

#### Rejected dominating-store join forwarding

A later ty profile exposed the next syntactic gap in the same stack-to-SSA
pass. `salsa::ActiveQuery::add_read_simple` computes two independent minima as
a dominating default stack store plus an overwrite in one arm of a diamond.
LLVM keeps both values in registers and uses conditional selects. Cranelift's
two-way join rule required both immediate predecessors to contain an explicit
store, so it retained a branch-and-stack merge for each value.

A bounded prototype carried known nonescaping integer stack values through
unique, acyclic `jump` or `brif` predecessor chains before applying the
existing two-edge, four-parameter join rule. It preserved the current overlap,
escape, exception-edge, and jump-table exclusions. Focused tests covered both
explicit `stack_store` operations and ordinary stores through an exact
`stack_addr`. The real `add_read_simple` symbol fell from 140 to 108 bytes, and
`ActiveQuery::add_read` fell from 292 to 260 bytes. LLVM remains substantially
tighter at 40 and 176 bytes respectively because it also selects rather than
branches and avoids the aggregate input copy.

All 186 `cranelift-codegen` library tests, all 1,218 CLIF filetests, the
focused regression file, `cargo check`, formatting, and the complete stage-2
compiler, backend, standard-library, and proc-macro build passed. The uv, Ruff,
and ty binaries nevertheless grew by 163,600 bytes (0.09%), 111,616 bytes
(0.10%), and 62,512 bytes (0.06%). The randomized 20-run application screen
preserved every exit code and output digest but did not support retention:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +2.46% | 6/20 | 0.11532 | 1.62x |
| `uv lock --check`, offline | -0.96% | 12/20 | 0.50344 | 1.43x |
| `ruff check` over 1,592 fixtures | +1.70% | 7/20 | 0.26318 | 1.81x |
| `ty check` over `scripts/ty_benchmark` | +0.30% | 10/20 | 1.00000 | 2.18x |

The exact backend is saved as
`dominating-stack-join/candidate.dylib` with SHA-256
`3e135dece3c52f1665bead6932225d5627e705df6c69676dac8bc03a3d376f7b`;
the screen is recorded in
`results/dominating-stack-join-screen20.json` under the benchmark scratch
directory. The mixed result is rejected without a 50-run gate, and the source
changes are reverted. A future stack-to-SSA extension should target a loop or
larger live region where stack traffic is sampled directly; local min/max
diamonds are structurally cleaner but too small to move these applications.

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

### Rejected dominated equality propagation

The remaining two-entry table in the hot hashbrown body was guarded by a
comparison that an earlier branch had already proved: its true edge established
that a loaded `u64` was one, but two intervening blocks prevented local
constant propagation. A bounded egraph experiment recovered equality facts
only when the true successor had that branch as its unique incoming edge and
dominated the use. It then folded later `brif` and `br_table` skeleton
instructions without changing pure values. In the profiled ty copy, the
redundant `u32::MAX` guard and indirect table disappear and the symbol span
falls from 1,624 to 1,572 bytes (-3.20%). The uv, Ruff, and ty binaries shrink
by 0.04%, 0.03%, and 0.02%, respectively.

Path facts must remain local to the control-flow skeleton. An initial prototype
subsumed the later pure comparison's eclass with a constant. Because Cranelift's
pure expressions can be shared with blocks outside the equality-controlled
region, that incorrectly changed uses on the opposite path; the application
correctness gate caught the resulting ty diagnostic explosion before timing.
The edge-local candidate restores the exact LLVM and retained-Cranelift
digests. A dedicated regression confirmed that a shared comparison remained
live on the false path while only the true path's branch was folded.

The safe candidate still failed its 20-run application screen:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -0.40% | 10/20 | 1.00000 |
| `uv lock --check`, offline | +0.65% | 4/20 | 0.01182 |
| `ruff check` over 1,592 fixtures | +1.54% | 9/20 | 0.82380 |
| `ty check` over `scripts/ty_benchmark` | +1.03% | 5/20 | 0.04139 |

Both uv resolution and ty regress decisively, so the candidate is rejected
without a 50-run gate. Dominated scalar facts remain useful for a future
range-analysis framework, but this isolated late control-flow cleanup is
another local size win whose layout effects outweigh its removed dispatches.

### Rejected known-receiver devirtualization

A fresh sample of the retained ty binary made hashbrown's
`RawTableInner::find_or_find_insert_index_inner` the dominant active Rust leaf.
Hashbrown deliberately shares this non-generic probe loop by passing its
generic equality closure as `&mut dyn FnMut`; LLVM later inlines the loop and
devirtualizes the closure call, while Cranelift retains both boundaries. A
reduced hashbrown insertion binary reproduced the shape.

The first candidate resolved a virtual method directly when optimized MIR had
exactly one nonescaped whole-local unsizing coercion for the receiver. The
vtable query then supplied the same concrete method instance that the dynamic
slot would contain. In the reduced binary, the indirect `blr` became a direct
`bl` to the closure shim. A second, narrowly triggered MIR budget admitted an
`#[inline]` body up to cost 1,200 only when inlining that body exposed such a
receiver. In the pinned applications, the complete candidate reduced retained
copies of `find_or_find_insert_index_inner` from 50 to 41 in uv, 30 to 23 in
Ruff, and 31 to 20 in ty.

The structural result did not translate into a stable application runtime
gain. Both variants used a matched 50-run LLVM/baseline/candidate gate:

| Workload | Direct-only change | Wins | Sign p | With targeted inlining | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.26% | 26/50 | 0.88772 | +0.36% | 22/50 | 0.47989 |
| `uv lock --check`, offline | -0.28% | 26/50 | 0.88772 | -0.65% | 26/50 | 0.88772 |
| `ruff check` over 1,592 fixtures | +0.48% | 21/50 | 0.32224 | +0.64% | 21/50 | 0.32224 |
| `ty check` over `scripts/ty_benchmark` | -0.58% | 30/50 | 0.20264 | +0.33% | 24/50 | 0.88772 |

The direct-only uv, Ruff, and ty binaries were 0.98%, 0.21%, and 0.55%
smaller; the complete candidate changed them by -1.01%, -0.21%, and -0.50%.
Every correctness digest matched. Neither variant meets the runtime bar, so
both were rejected despite their code-size and local control-flow wins. The
result narrows the call-overhead arc: removing one shared hash-table call and
one indirect equality call is insufficient on its own; a future candidate
needs to expose a larger loop transformation or eliminate a repeated static
leaf across the measured hot paths.

### Rejected integer-constant rematerialization policy

The next retained ty profile made `Type::hash` and
`FxHasher::write_isize` the second and third largest active Rust leaves. The
active Cranelift `Type::hash` copy is 6,780 bytes, calls `write_isize` 26
times, and contains 213 `movk` instructions. The corresponding LLVM copy is
664 bytes, contains no calls, materializes the FxHasher multiplier once, and
keeps the hash state in a register across the discriminant arms.

Cranelift's egraph rematerializes standalone integer constants in every use
block without consulting the target. Because an arbitrary AArch64 64-bit
constant can require a `mov` plus three `movk` instructions, a broad
upper-bound experiment disabled that rule while preserving rematerialization
of immediate-form arithmetic. This did not change any of the four retained
`Type::hash` copies. The active copy remained 6,780 bytes with exactly the
same opcode counts, including 213 `movk`, 77 `mul`, 26 `bl`, and 31 `ret`
instructions. The constants are already distinct frontend IR values in
separate enum arms; the generic egraph rematerialization rule does not create
their duplication.

The broad policy also lost the 20-run application screen:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +0.98% | 7/20 | 0.26318 |
| `uv lock --check`, offline | +2.38% | 4/20 | 0.01182 |
| `ruff check` over 1,592 fixtures | +1.13% | 7/20 | 0.26318 |
| `ty check` over `scripts/ty_benchmark` | +1.47% | 4/20 | 0.01182 |

Every correctness digest matched. The uv and Ruff binaries shrank by 0.01%
and 0.07%, while ty grew by 0.08%. The candidate is rejected without a 50-run
gate: it leaves the motivating function unchanged and decisively regresses uv
resolution and ty. A target-specific refinement of this rematerialization
rule is therefore not the next arc. Matching LLVM's hash shape instead needs
a larger transformation that inlines and merges the repeated enum-arm tails.

### Rejected repeated wide-constant entry hoisting

A sharing-aware follow-up tested the strongest version of the constant-placement
hypothesis. After egraph extraction, it counted uses of canonical values. An
`i64` constant outside the signed 32-bit range with at least eight uses was
materialized once in the entry block and exempted from per-block
rematerialization. A reduced eight-arm function then emitted one multiplier
definition shared by every arm.

The real `Type::hash` body changed substantially. Its active copy shrank from
6,780 to 5,784 bytes (-14.69%), `movk` fell from 213 to 3, and `mov` fell from
385 to 314. Cranelift kept the repeated multiplier in callee-saved `x19`.
That longer live range also increased loads from 227 to 258, stores from 77 to
78, and added a saved `x28`. Whole-binary sizes remained nearly flat: uv,
Ruff, and ty shrank by 0.01%, 0.02%, and 0.01%, respectively.

The structural win did not move the 50-run application gate:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +1.17% | 22/50 | 0.47989 |
| `uv lock --check`, offline | +0.89% | 21/50 | 0.32224 |
| `ruff check` over 1,592 fixtures | -0.37% | 26/50 | 0.88772 |
| `ty check` over `scripts/ty_benchmark` | +0.30% | 20/50 | 0.20264 |

Every correctness digest matched. The focused sharing, rematerialization, and
LICM filetests, all 185 `cranelift-codegen` unit tests, and the complete
stage-2 cg_clif and standard-library build passed. The candidate is rejected:
none of the representative workloads improved, while both uv rows and ty
trended backward. Hoisting LLVM's multiplier in isolation is insufficient
while Cranelift still retains 26 `write_isize` calls and fragmented enum-arm
tails. Future work on this body needs to merge the complete hash update, not
only share its literal.

### Rejected dominator-sibling LICM restoration

The `hash_bytes` profile exposed a second placement issue while investigating
its loop-local AArch64 multiplier. Egraph elaboration tracks active loops in a
mutable stack during dominator-tree traversal. Visiting an exit child pops the
loop, but the old iterative traversal restores only the scoped value map when
returning to the parent. If a loop-body sibling is visited afterward, it is
elaborated with an empty loop stack and misses LICM. A focused `vconst`
regression demonstrated that changing only the branch-child order changed
whether the constant was hoisted.

A correct prototype saved the loop entries popped on block entry and restored
them when that dominator scope closed. It fixed the reduced case, but also
broadened LICM across the applications enough to grow uv by 410,464 bytes
(0.15%), Ruff by 259,168 bytes (0.19%), and ty by 289,680 bytes (0.21%). The
20-run matched screen measured:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +1.91% | 7/20 | 0.26318 |
| `uv lock --check`, offline | +0.42% | 7/20 | 0.26318 |
| `ruff check` over 1,592 fixtures | -0.93% | 11/20 | 0.82380 |
| `ty check` over `scripts/ty_benchmark` | +2.08% | 6/20 | 0.11532 |

Every correctness probe matched, but three workloads and every binary moved
backward, with ty showing the largest regression. The restoration is rejected
before a 50-run gate. This is a real order-dependent optimizer defect, but
turning all previously missed cases into unconditional LICM is not a runtime
win. A future repair needs a placement cost model that accounts for longer
live ranges, register pressure, duplication, and loop frequency rather than
restoring the old heuristic globally. A simultaneous target-specific attempt
to keep wide AArch64 `i64` constants invariant did not hoist the multiplier in
the exact optimized `hash_bytes` CLIF and was also dropped.

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

### Native AArch64 `i8x8` comparisons and splats

A later profile returned to the same hash-table loop after the retained stack,
branch, and body-import work had changed its surroundings. cg_clif still
lowered Rust SIMD comparisons and splats one lane at a time even when
Cranelift had a native fixed-vector representation. The active hashbrown
`RawTableInner::find_or_find_insert_index_inner` copy was 1,624 bytes and
contained dozens of lane loads, scalar comparisons, and OR operations.

The retained lowering deliberately covers only AArch64 `i8x8`. Integer
`simd_eq`, `simd_ne`, and signed or unsigned ordered comparisons become one
Cranelift vector `icmp`; `simd_splat` becomes a native vector splat. All other
targets and vector shapes retain the existing per-lane path. In the profiled
hashbrown body, the generated AArch64 sequence now uses `dup.8b`, `cmeq.8b`,
and `cmgt.8b`. Its span falls from 1,624 to 764 bytes (-52.96%).

The narrow policy's 50-run matched gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +0.24% | 24/50 | 0.88772 | +68.5% | -0.203% |
| `uv lock --check`, offline | +0.56% | 22/50 | 0.47989 | +40.9% | -0.203% |
| `ruff check` over 1,592 fixtures | -0.34% | 26/50 | 0.88772 | +83.8% | -0.271% |
| `ty check` over `scripts/ty_benchmark` | -5.79% | 49/50 | 0.000000000000091 | +127.1% | -0.651% |

Both uv operations and Ruff are statistically neutral, ty improves
decisively, every binary shrinks, and all correctness-probe digests match.
The AArch64 NEON example covers equality, signed less-than, and splat. The
focused cg_clif check and complete stage-2 cg_clif, standard-library, and
proc-macro build pass.

The shape restriction is empirical rather than aesthetic. A preceding version
enabled the same comparison and splat lowering for every 64- and 128-bit
vector. It improved Ruff by 1.77% (33/50 wins, `p = 0.03284`) and ty by 5.55%
(50/50 wins), but regressed uv environment creation by 1.27% with only 14/50
wins (`p = 0.00260`); uv resolution was neutral at +0.51%. That broader policy
was rejected. Native SIMD should therefore expand one profiled shape at a time,
with surrounding operations moved to vectors before a shape is enabled
globally.

### Rejected bitcast-zero vector comparisons

A fresh matched LLVM build made the remaining loop gap concrete. LLVM emits no
standalone `RawTableInner::find_or_find_insert_index_inner` copies in ty; it
inlines the probe into callers, devirtualizes the equality closure, and keeps
the group and probe state in registers. Cranelift retains 31 local copies. The
active 636-byte copy also built a zero `i8x8` through `mov` and `fmov` before
each of two signed byte comparisons, while LLVM used AArch64's one-instruction
`cmlt.8b ..., #0` form.

Cranelift already selects the immediate-zero comparison when it can see a
`splat` or vector constant. Stack reinterpretation forwarding changed these
particular zeroes into `bitcast.i8x8` values, hiding the same fact from
lowering. A prototype carried zero recognition through a bitcast. It replaced
both instruction pairs in the active probe and shrank the body from 636 to 620
bytes. Across ty, 562 three-register `cmgt` sites became `cmlt ..., #0`.

The initial 20-run ty screen was neutral: -0.67% paired, 12/20 wins, and
`p = 0.50344`. At 50 runs the direction reversed to a 0.78% regression with
18/50 wins (`p = 0.06491`). The candidate and baseline medians were 108.08 ms
and 107.49 ms, while the fresh LLVM control was 48.42 ms; all correctness-probe
digests matched. Restricting the recognition to `i8x8` did not reduce the blast
radius: all 562 changed ty sites already had that result type, and the broad
and narrow candidates had identical machine text.

This is rejected. Re-spelling two comparisons cannot compensate for the
retained call boundary, vector spills, and loop-state traffic. Further work on
this probe needs a larger loop or call/register transformation, not another
isolated compare encoding. The 20- and 50-run evidence is recorded in
`bench-bitcast-zero-ty20/results.json` and
`bench-bitcast-zero-ty50/results.json` under the benchmark scratch directory.

### Rejected same-width stack reinterpretation forwarding

The retained `i8x8` lowering exposed four remaining vector-to-integer
round-trips in the same hashbrown body. cg_clif stored each comparison result
as `i8x8` and immediately reloaded the same eight bytes as `i64` for bitmask
tests. Cranelift's pre-legalization stack pass keys known values by both
location and type, so it could not forward those otherwise exact stores and
loads.

An experiment recognized a stored value covering the load's exact byte range
and replaced the memory round-trip with a same-size `bitcast`. The bitcast used
the target byte order because vector-to-scalar reinterpretation changes the
lane shape. Focused little- and big-endian tests covered both scalar-to-vector
and vector-to-scalar directions. In the reduced hot function, all four
comparison-result slots disappeared, the local stack allocation fell from 208
to 144 bytes, and the body shrank from 764 to 756 bytes.

The cleaner local result did not move the complete 50-run application gate:

| Workload | Previous Cranelift | Candidate | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | 56.59 ms | 56.82 ms | -0.13% | 26/50 | 0.88772 |
| `uv lock --check`, offline | 86.87 ms | 87.67 ms | +0.31% | 23/50 | 0.67181 |
| `ruff check` over 1,592 fixtures | 143.72 ms | 143.97 ms | -0.56% | 30/50 | 0.20264 |
| `ty check` over `scripts/ty_benchmark` | 108.84 ms | 108.21 ms | -0.59% | 30/50 | 0.20264 |

All correctness-probe digests matched. uv and ty shrank by 9,552 and 12,160
bytes, while Ruff grew by 5,632 bytes; every change is at most 0.01%. The
focused filetests, all 185 `cranelift-codegen` unit tests and 21 passing
doctests, cg_clif check, complete stage-2 cg_clif and standard-library build,
and all three application builds passed. The transformation is sound and
locally effective, but isolated transmute forwarding is runtime-neutral, so it
is rejected. The next profitable vector-stack step must remove the
address-taken group load or loop-invariant vector spill, or keep a larger
region vector-shaped, rather than only replacing result materialization.

### Rejected `i8x8` splat rematerialization

After same-width stack forwarding, the active hashbrown body still created a
dynamic `i8x8` hash-byte splat before an indirect equality call. Regalloc kept
the vector live across the call by spilling `q0` and reloading it in the probe
loop. A deliberately narrow egraph experiment marked only `i8x8` splats as
rematerializable. It kept the scalar byte live and recreated the vector beside
its comparison, eliminating that vector spill and reload and reducing the
local frame from 208 to 192 bytes.

The trade was not free. The hot body grew from 764 to 784 bytes because the
scalar lane occupied another callee-saved GPR and each loop iteration gained a
`dup`; Ruff grew by 7,056 bytes and ty by 5,440 bytes. A 20-run matched screen
against the retained native-`i8x8` policy measured:

| Workload | Previous Cranelift | Candidate | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: | ---: | ---: |
| `ruff check` over 1,592 fixtures | 144.37 ms | 143.82 ms | -0.06% | 10/20 | 1.00000 |
| `ty check` over `scripts/ty_benchmark` | 110.10 ms | 109.80 ms | +0.57% | 8/20 | 0.50344 |

Both correctness-probe digests matched. The change is neutral for Ruff and
trends backward for ty while growing both binaries, so it was rejected before
paying for the uv build. Removing one loop-invariant vector spill merely
exchanges vector pressure for scalar pressure and repeated work. The next
hashbrown stack candidate should eliminate the address-taken group load or
keep the whole group producer-consumer path in registers.

### Nonescaping direct stack-access forwarding

Rust's `ptr::read_unaligned` lowering materializes a `MaybeUninit` temporary by
taking its address, writing through an ordinary `store`, and reading it through
an ordinary `load`. Cranelift's pre-legalization stack pass previously treated
every `stack_addr` as an escape, so it could not see through that sequence even
when the address never left the function or the direct memory operations. In
the profiled hashbrown loop, this left the control-group value on the stack and
prevented the otherwise neutral same-width bitcast from removing the complete
round-trip.

The retained pass recognizes bounded ordinary loads and stores whose address
is exactly a `stack_addr`. It permits only those direct memory operands; calls,
stored pointers, derived addresses, stack maps, branch arguments, exception
contexts, and every other use still make the whole slot escape. Known values
now flow between explicit stack operations and direct memory operations, and
exact same-byte-range type changes use a target-endian `bitcast`. Ordinary
stores and the slot allocation are removed only when every read was forwarded.

The escape scan deliberately visits all instruction values rather than only
ordinary instruction arguments. An early version missed `BlockCall` arguments,
deleted a slot whose address flowed through a block parameter, and caused
`Iterator::position` to return a stack address as its index. A dedicated
control-flow regression now preserves that slot. The corrected backend also
passes standalone `position()` and clap `--version` reproductions that failed
under the broken version.

The hot `RawTableInner::find_or_find_insert_index_inner` frame falls from 208
to 96 bytes and its body from 764 to 700 bytes. The complete matched 50-run
gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +0.58% | 24/50 | 0.88772 | +66.70% | -0.095% |
| `uv lock --check`, offline | -0.19% | 26/50 | 0.88772 | +42.76% | -0.095% |
| `ruff check` over 1,592 fixtures | -1.46% | 30/50 | 0.20264 | +84.62% | -0.141% |
| `ty check` over `scripts/ty_benchmark` | -1.43% | 35/50 | 0.00660 | +138.25% | -0.112% |

Both uv operations and Ruff are statistically neutral, ty improves
decisively, every binary shrinks, and all correctness-probe digests match. The
focused little- and big-endian filetests, the branch-argument escape
regression, all 185 `cranelift-codegen` unit tests and 21 passing doctests, and
the complete stage-2 cg_clif, standard-library, and proc-macro build pass. This
is retained: the address-taken group load was the missing larger context that
turned isolated, runtime-neutral bitcast forwarding into an application win.

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

### Rejected constant-specialized precondition import

A later amplified ty profile made the remaining boundary more precise. The
mutable-pointer `add` and `sub` precondition helpers accounted for roughly
3,150 active samples combined and were absent as leaves from the matched LLVM
profile. Machine-code inspection showed a shared 88-byte helper with three hot
range checks and one cold panic tail. Of 4,080 direct calls to the three
profiled helper copies, 2,388 supplied constant count and element-size
arguments; 319 supplied a zero count. This is the post-monomorphization shape
that LLVM can specialize but the MIR inliner intentionally cannot see.

The source and CLIF bounds were measured rather than guessed. The pointer
helper has nine MIR blocks and 76 operations, then ten CLIF blocks and 42
instructions with one call in a terminal trap block. A first CLIF policy using
the existing six-block/32-instruction import limit changed none of the 4,646
mutable- and const-pointer `add`/`sub` calls in the complete ty binary. Raising
the cold-call-only instruction budget to 64 also changed none. The two exact
ty binaries moved by -2,176 and +3,312 bytes respectively, confirming unrelated
layout noise rather than the intended transform.

A guarded-specialization prototype then admitted only `#[rustc_no_mir_inline]`
source bodies up to ten blocks and 96 operations, required exactly one call in
a terminal trap block, and required at least two integer constants at the call
site. To avoid importing the panic-message global, it replaced the cold block
in the candidate with a call back to the canonical helper. The hot checks could
then be cloned and folded while a failing path would re-run the shared helper
and panic. The prototype also promoted matching one-level catalogue
dependencies and remapped only function names still referenced by attached
instructions.

The exact pointer probe still retained the original helper call and identical
caller machine code. The current post-monomorphization catalogue does not
preserve a usable candidate identity at this dependency call boundary, even
after the dependency is promoted. The experiment is rejected before an
application timing screen because it does not perform the intended transform.
Reopening this arc requires a first-class partial-inlining or guarded-call
specialization facility tied directly to the caller's `FuncRef`; another MIR,
CLIF, or import threshold change is not a plausible next step.

### Guarded post-monomorphization precondition import

The pointer-precondition profile remained the strongest concrete call-boundary
gap after the loop-wrapper experiment. Mutable-pointer `add` and `sub` helpers
accounted for roughly 3,150 active Cranelift samples and did not appear as
leaves in the matched LLVM profile. The earlier guarded prototype had already
identified the right semantic split, but its imported candidate was rejected
because dead panic-allocation names survived in the function parameter table.

The initial retained implementation makes that split first-class. Hinted
source roots up to ten MIR blocks and 96 operations are translated only as
possible guarded candidates; roots beyond the ordinary six-block/64-operation
limits cannot fall into the existing repeated-call policy. A guarded candidate
must have at most ten CLIF blocks and 64 instructions, return no value, have
exactly one result-free call in a cold trap block, and contain no load, store,
or trap in its hot region. Instructions with other side effects are rejected
there too.
The cold block is replaced with a call back to the canonical helper using the
original entry arguments, followed by the original trap. Optimization then
removes its dead stack slot. The candidate remains eligible only without live
stack state or a surviving global-value instruction.

At the call site, this guarded body initially bypassed the repeated-call
requirement only when at least two arguments were integer constants. This is
the post-monomorphization count-and-element-size shape that LLVM sees for
pointer checks. Dynamic-count calls retain the canonical helper. Candidate
remapping now visits only function names attached to surviving `call`,
`try_call`, or `func_addr` instructions, so dead panic-message allocation names
neither block the import nor cause unrelated definitions to be materialized.
The stage-2 bootstrap caught the required `func_addr` case before benchmarking:
omitting it produced unresolved formatting implementations, while the final
mapping links and runs the same formatting probe correctly.

The reduced pointer probe demonstrates both paths. A dynamic count retains its
direct helper call. Constant count and element size clone the pure arithmetic
guard, and only its cold failure edge calls the canonical helper. Valid
`add(1)`/`sub(1)` operations produce the expected pointers; an overflowing
`add(1)` reaches the original non-unwinding panic and aborts with the same
message. In the full ty binary, symbolized precondition-helper definitions and
branch references fall from 7,093 to 4,172.

The complete 50-run three-lane gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +0.05% | 25/50 | 1.00000 | +58.9% | +0.070% |
| `uv lock --check`, offline | +0.88% | 19/50 | 0.11892 | +39.4% | +0.070% |
| `ruff check` over 1,592 fixtures | +0.60% | 21/50 | 0.32224 | +77.4% | -0.714% |
| `ty check` over `scripts/ty_benchmark` | -1.02% | 32/50 | 0.06491 | +115.0% | -1.621% |

Higher-powered focused gates resolved the three noisy rows. uv lock was
-0.02% with 51/100 wins (`p = 0.92041`), Ruff was -0.58% with 58/100 wins
(`p = 0.13321`), and ty was -0.79% with 65/100 wins (`p = 0.00352`). uv
environment creation remained statistically neutral in the complete gate.
Every LLVM, preceding-Cranelift, and candidate exit code and output digest
matched. cg_clif check, the complete stage-2 backend build, the matching stage-2
standard-library and proc-macro build, and all three fresh application builds
passed. A final review tightened the hot-path purity check to reject every
other side-effecting instruction. The exact stage-2 rebuild is saved as
`candidate-v5-reviewed.dylib` with SHA-256
`4e5ecc82b065a5b1a839a33f452360165b991d0d1f38e0f066508c3befd522a4`;
its four optimized pointer-probe bodies are identical to the benchmarked
candidate after normalizing generated function and allocation numbering.
Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/guarded-precondition-all50.json`
and the adjacent `guarded-precondition-*-100.json` focused runs.

The guarded import is retained. It removes measured helper traffic and
decisively improves ty while both uv operations and Ruff remain neutral. The
canonical cold helper preserves panic formatting, location, and failure
semantics without cloning its globals into every caller.

A follow-up constant fold for `umulhi` of two integer constants removed the
multiply-high overflow test from the reduced inlined guard and cut another
100,976 bytes from uv, 76,160 bytes from Ruff, and 77,824 bytes from ty. The
focused egraph filetest passed, but the application gate rejected the fold.
Against the import-only policy, Ruff regressed by 1.18% paired with 40/100 wins
(`p = 0.05689`); against the preceding Cranelift baseline it regressed by
0.82% with 38/100 wins (`p = 0.02098`). ty, uv lock, and uv environment
creation were neutral relative to the import-only policy. The fold is not
retained: deleting the local constant branch changes surrounding extraction
or layout in a way that costs more in Ruff than it saves in the pointer guard.
Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/guarded-precondition-umulhi-all50.json`
and `guarded-precondition-umulhi-ruff100.json`.

### Dynamic-count guarded precondition import

A fresh profile of the retained policy left mutable-pointer `add` and `sub`
precondition helpers at 2,335 active ty samples. The dynamic-count pointer
probe explained why: the count remained variable, but the element size was
already an integer constant at the call boundary. Requiring two constant
arguments therefore kept the whole success-path guard out of line even though
one argument was enough to specialize it.

The retained extension admits an existing guarded candidate when any call
argument is an integer constant. No ordinary import policy changes. In the
dynamic-count pointer probe, the multiply-high overflow check, scaled offset,
and address check move into the caller; only the cold failure edge calls the
canonical helper. The valid `add` and `sub` probe still succeeds, while the
overflow probe reaches the same non-unwinding panic and message. Direct ty
branches to precondition helpers fall from 4,084 to 2,316 (-43.3%).

The complete 50-run three-lane gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.78% | 31/50 | 0.11892 | 1.64x | +0.061% |
| `uv lock --check`, offline | +0.63% | 22/50 | 0.47989 | 1.41x | +0.061% |
| `ruff check` over 1,592 fixtures | +0.52% | 23/50 | 0.67181 | 1.76x | +0.070% |
| `ty check` over `scripts/ty_benchmark` | -1.97% | 42/50 | 0.00000116 | 2.10x | +0.080% |

A focused 100-run ty replication measured -2.41% with 86/100 wins
(`p = 8.28e-14`). The other three workloads remain statistically neutral,
and every LLVM, preceding-Cranelift, and candidate exit code and output digest
matches. cg_clif check, the complete stage-2 backend, standard-library, and
proc-macro build, all three fresh application builds, and both pointer probes
pass. The exact backend is saved as
`guarded-precondition-one-constant/candidate.dylib` with SHA-256
`3dd8ab4124b5d2c2ce529b670c4bccc179ce7fa4751c6e301619c93169817a06`.
The extension is retained: it converts the profile-supported dynamic pointer
guards into caller-local checks and decisively improves ty for less than 0.1%
binary growth. Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/guarded-precondition-one-constant-all50.json`
and `guarded-precondition-one-constant-ty100.json`.

### Rejected module-frequency guarded precondition import

The retained policy still left 2,316 direct ty branches to precondition
helpers. Many had dynamic arguments but repeated across generated wrappers:
1,304 targeted `NonNull::new_unchecked`, 549 targeted
`unreachable_unchecked`, and 47 targeted `NonZero::new_unchecked`. A narrow
follow-up counted only calls to already-vetted guarded candidates across the
codegen unit and admitted a dynamic-argument guard after eight calls. Ordinary
import eligibility and profitability were unchanged.

The policy performed the intended transform. Direct precondition branches in
ty fell from 2,316 to 682. The three targeted families fell from 1,304 to 85,
549 to 155, and 47 to 26, respectively. A complete stage-2 backend,
standard-library, and proc-macro build and fresh uv, Ruff, and ty builds passed.
The candidate preserved all LLVM and retained-Cranelift application exit codes
and output digests. Its backend is saved as
`frequent-guarded-imports/candidate.dylib` with SHA-256
`4bb8090a330db437bd2facef2bccd293b30ecbabb467fb3e7979a6802d513e9c`.

The 20-run application screen rejected the tradeoff:

| Workload | Paired change | Wins | Sign p | Binary change |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.24% | 12/20 | 0.50344 | -0.007% |
| `uv lock --check`, offline | +1.92% | 7/20 | 0.26318 | -0.007% |
| `ruff check` over 1,592 fixtures | +0.20% | 9/20 | 0.82380 | +0.782% |
| `ty check` over `scripts/ty_benchmark` | -0.14% | 10/20 | 1.00000 | +1.765% |

No workload shows a directional statistical result, while the applications
that contain most of the removed branches grow materially. The module-frequency
exception is rejected without a 50-run gate. Aggregate static popularity is
not a sufficient profitability signal even for pure guarded imports; the next
extension needs execution frequency or a more precise caller-side cost model.
Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/frequent-guarded-imports-screen20.json`.

### Rejected `umulhi` zero-and-one folding

The retained dynamic-count guard import exposed a redundant arithmetic check
inside the hottest hashbrown loop. A pointer offset by one-byte elements still
lowered `umulhi(position, 1)` and branched on its result, although the high
half of any unsigned integer multiplied by zero or one is always zero. A
narrow egraph experiment folded only those two identities.

The focused optimizer test and complete stage-2 backend, standard-library, and
proc-macro build passed. In a fresh ty build, `umulh` instructions fell from
7,160 to 5,928 (-17.2%). The exact hot hashbrown body lost the multiply-high,
its constant materialization, and the associated branch. All LLVM,
retained-Cranelift, and candidate application exit codes and output digests
matched. The candidate backend is saved as
`umulhi-zero-one/candidate.dylib` with SHA-256
`3fee386cf1485dbaa879ed22d29f716f4bef6d0f1291ea01536bfbcbbe32367b`.

The 20-run application screen nevertheless moved backward in every paired
median:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +1.29% | 9/20 | 0.82380 |
| `uv lock --check`, offline | +2.68% | 9/20 | 0.82380 |
| `ruff check` over 1,592 fixtures | +0.26% | 10/20 | 1.00000 |
| `ty check` over `scripts/ty_benchmark` | +0.88% | 9/20 | 0.82380 |

The identity is rejected without a 50-run gate. Removing a locally redundant
overflow check perturbs extraction or layout without relieving the remaining
hashbrown frame, indirect equality call, or loop-carried stack traffic. Results
are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/umulhi-zero-one-screen20.json`.

### Rejected loop memory-forwarder import

The next retained profile still showed a real call to
`ptr::read_unaligned<uint8x8_t>` inside the hottest hashbrown probe loop. Its
final machine body is only one load through the source pointer and one store
through the implicit return pointer. A deliberately narrow
post-monomorphization exception therefore recognized already-safe imported
bodies with exactly that load-to-store shape and allowed a one-off import only
when the call instruction was in a natural loop.

The complete stage-2 backend, standard-library, and proc-macro build and fresh
Ruff and ty builds passed, but the intended transform did not fire. The hot ty
body was instruction-for-instruction unchanged: it retained the
`read_unaligned` call, the `sp+0x40` result slot, the copy to `sp+0x20`, and
the subsequent vector reload. Instrumenting the isolated catalogue explains
the mismatch between source and final machine shape. The monomorphized
`read_unaligned<uint8x8_t>` source has eight MIR blocks and 38 operations, so
it exceeds the ordinary six-block import limit. Its initial CLIF has nine
blocks, two eight-byte explicit stack slots, one global value, and two calls
for the precondition and copy path. It becomes a six-instruction machine
wrapper only after the normal complete optimization pipeline, too late for
the bounded catalogue selector.

The prototype is rejected before a runtime screen because it did not remove
the measured boundary. Its backend is preserved as
`loop-memory-forwarder/candidate.dylib` with SHA-256
`aabc9778d5dfabec22eae4cc6e12ef5fa8a4237f5ec941314485e02e2bfd6479`.
Reaching this helper through body import would require a broader multi-call,
stack-bearing catalogue pipeline, recreating already-rejected safety and
profitability territory. A future attempt should instead expose the final
optimized body to a later inliner or lower this primitive directly; another
one-off source-shape exception cannot see the profitable form.

### Rejected direct `read_unaligned` lowering

The phase-correct follow-up recognized Core's `ptr_read_unaligned` diagnostic
item at cg_clif's call-lowering boundary. With UB checks disabled, the broad
prototype loaded the source with alignment one and wrote the result directly
to the MIR destination. Scalar and scalar-pair results were completely loaded
before any destination store; memory-backed aggregates first passed through a
private aligned stack slot to preserve overlapping-source-and-destination
semantics. Builds with UB checks retained Core's original precondition path.

The complete stage-2 backend, standard-library, and proc-macro build passed.
A focused no-MIR-inlining control covered scalar, scalar-pair, eight-byte SIMD,
aggregate, zero-sized, and non-`Copy` values. The retained backend emitted six
Core `read_unaligned` bodies; the direct candidate emitted none, both binaries
ran successfully, and an explicit UB-checking build retained and passed the
Core path. The broad candidate backend is preserved as
`read-unaligned-direct/candidate.dylib` with SHA-256
`976d5558783db88b0721ca7ee833f6bd203e206fd619bf1a13e2855b22ad8bdb`.

The broad application build likewise removed all surviving Core
`read_unaligned` bodies: 13 to zero in uv, 10 to zero in Ruff, and 16 to zero
in ty. Its initial 20-run screen was mixed, however: uv environment creation
was +5.17% paired, uv resolution -0.30%, Ruff -1.41%, and ty -1.63%, with no
two-sided sign test below 0.115. A subsequent binary audit found that the
older saved Ruff and ty controls also differed materially from fresh builds
of the retained backend under the reconstructed stage-2 compiler, so this
screen is diagnostic rather than retention-quality evidence. Its full record
remains in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/read-unaligned-direct-screen20.json`.

The final experiment isolated the measured hashbrown operation: only an
unchecked `BackendRepr::SimdVector` whose total size is eight bytes was loaded
directly. The source uses an ordinary unaligned Cranelift vector load followed
by a by-value destination write, so no aligned-memory fact is attached. A
focused optimized probe removes the Core wrapper while the UB-checking control
retains it, and both pass. Fresh application builds against fresh retained
controls removed exactly the intended wrappers: 13 to 12 in uv, 10 to 9 in
Ruff, and 16 to 14 in ty. Their Mach-O `__TEXT` segment sizes were unchanged;
complete file sizes changed by only +8,032, +5,760, and +4,592 bytes. The
narrow backend is preserved as `read-unaligned-u8x8/candidate.dylib` with
SHA-256 `04c76520ce03e2b1f5d68bb8208fed70805534b8a1f07d75243f965b0747a0d0`.

The corrected 20-run application gate still showed no benefit:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +3.97% | 8/20 | 0.50344 |
| `uv lock --check`, offline | +1.00% | 9/20 | 0.82380 |
| `ruff check` over 1,592 fixtures | +1.69% | 9/20 | 0.82380 |
| `ty check` over `scripts/ty_benchmark` | -0.33% | 10/20 | 1.00000 |

Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/read-unaligned-u8x8-screen20.json`.
The narrow rule is rejected without a 50-run gate. Eliminating the wrapper in
isolation does not keep the group value and surrounding probe state in
registers, devirtualize the equality closure, or inline the complete loop into
its caller. The next attempt at this boundary needs a later optimized-body
inliner or a larger loop transformation rather than another
primitive-specific call expansion.

### Rejected transparent SIMD-wrapper SSA

The next experiment attacked the remaining copy rather than the Core helper.
It allowed only non-SIMD AArch64 wrappers with an eight-byte SIMD backend
representation to use one Cranelift variable. True SIMD locals stayed
stack-backed, transparent field projections reused the wrapper variable, and
static lane reads used `extractlane`; dynamic lane reads retained a safe spill
fallback. This was intended to keep hashbrown's `Group(uint8x8_t)` in SSA while
leaving the `read_unaligned` call result in its existing return slot.

The complete stage-2 backend, standard-library, and proc-macro build passed,
as did a fresh hashbrown build and execution probe and fresh uv, Ruff, and ty
profiling builds. The candidate backend is preserved as
`transparent-simd-wrapper-ssa/candidate.dylib` with SHA-256
`bda5ea38df9961dc4a61a80b1bb79c3b2b478bc2916a95193f69e5f26fec4280`.
The application binaries changed by +24,320 bytes for uv, +24,016 bytes for
Ruff, and +20,896 bytes for ty, confirming that the rule reached other
wrappers.

It did not reach the measured boundary. Both profiled ty copies of
`find_or_find_insert_index_inner` retained the same 80-byte frame, the same
688- and 608-byte body sizes, the `read_unaligned` result slot, the copy into
the `Group` slot, and the subsequent vector reloads. The source passes
`&group` to `find_insert_index_in_group`. cg_clif's SSA analysis treats that
immutable reference as address observation and marks the entire local
non-SSA before its representation is considered.

The prototype is therefore rejected without a runtime screen: it changed
unrelated code but removed none of the measured instructions. A viable
follow-up must either materialize a read-only address-taken local only at its
borrow use while retaining an SSA value elsewhere, or eliminate/import the
small by-reference helper. Simply admitting more SIMD-backed types to the
existing all-or-nothing local policy cannot affect this loop.

### Rejected shared-borrow and native `i8x8` SSA

The follow-up implemented both missing pieces from the wrapper-only result.
An eight-byte transparent wrapper borrowed immutably kept a Cranelift variable
plus synchronized stack storage for the borrow, and native AArch64 `i8x8`
locals used Cranelift variables with explicit lane extraction and insertion.
SIMD locals constructed through aggregate field writes remained memory-backed;
that narrow exclusion was required because cg_clif's aggregate writer treats a
SIMD value as a struct containing an array rather than as one vector value.

The complete stage-2 backend, standard-library, and proc-macro build passed,
as did a fresh hashbrown build and execution probe and fresh uv, Ruff, and ty
profiling builds. The candidate backend is preserved as
`shared-borrow-i8x8-ssa/candidate.dylib` with SHA-256
`7bf8824a6bda9cf44825aae523ca1d420a355b40a866f8fca5a12a26178b230e`.
The candidate grew uv by 13,136 bytes, Ruff by 10,048 bytes, and ty by 11,008
bytes.

This version did reach the intended boundary, but only partially. In each
application, one hashbrown `find_or_find_insert_index_inner` body fell from
608 to 600 bytes while the companion 688-byte body was unchanged. The focused
hashbrown probe fell from 664 to 656 bytes and stopped reloading its control
group from an eight-byte scalar stack slot. Keeping the vector live across the
indirect equality callback instead required a vector spill, however, and grew
that probe's frame from 80 to 96 bytes.

The matched 20-run application gate found no runtime benefit:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -1.05% | 11/20 | 0.82380 |
| `uv lock --check`, offline | -0.76% | 11/20 | 0.82380 |
| `ruff check` over 1,592 fixtures | +2.26% | 7/20 | 0.26318 |
| `ty check` over `scripts/ty_benchmark` | +1.05% | 9/20 | 0.82380 |

Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/bench-shared-borrow-i8x8-ssa-all20/results.json`.
The prototype is rejected without a 50-run gate. Replacing one scalar reload
with a larger live vector did not reduce total pressure across the callback,
and the broad local-representation machinery increased both implementation
complexity and binary size. The next vector-region attempt needs to remove or
devirtualize that callback, or keep enough surrounding state in registers to
avoid exchanging one spill form for another.

### Finalization correctness fixes

The final whole-suite gate found two backend correctness gaps that the
application probes did not exercise.

First, compiling `zeroize 1.8.2` at optimization level 1 reached an invalid
`iconst.i128` and made the Cranelift verifier panic. Primitive `i128` and
`u128` constants already used two `i64` constants plus `iconcat`, but a scalar
niche newtype such as `NonZero<i128>` fell through the generic scalar path.
cg_clif now uses the same two-half construction for scalar-layout `i128`
newtypes. The verifier also reports an invalid `iconst` controlling type as a
normal verifier error instead of panicking while calculating its bounds. The
exact `zeroize` reproduction, a `NonZeroI128` volatile-write regression, the
focused verifier tests, cg_clif check, and complete stage-2 build pass.

Second, SRS bootstrap enabled cg_clif's opt-in unwinding feature only on
`x86_64-unknown-linux-gnu`. On Apple silicon, `std::intrinsics::catch_unwind`
therefore lowered its callback as a plain indirect call. Three uv async tests
that intentionally catch a debug assertion passed under LLVM and failed under
Cranelift. Bootstrap now enables unwinding for `aarch64-apple-darwin` as well.
A fresh Cranelift build of the exact `uv-auth` reproduction catches the panic,
and the three tests pass in the final complete suite.

These are prerequisites, not performance wins. The final runtime and size
numbers use the corrected unwinding-enabled backend; the earlier Apple-silicon
Cranelift binaries are not valid correctness controls.

The final remote macOS gate exposed two additional parts of the same boundary.
An unwinder transfers control to a landing pad without recreating transient
register values from before the throwing call. cg_clif now lowers the
`catch_unwind` try-call with Cranelift's `PreserveAll` convention. On an
exceptional edge that convention deliberately treats every allocatable
register as clobbered, so only values live into the catch block are spilled;
the ordinary argument ABI is unchanged. A broader experiment made every Apple
try-call clobber every register. It fixed the focused panic reproduction but
grew the full Ruff/ty link enough to exceed AArch64's branch range, so it was
rejected in favor of the intrinsic-specific change.

Mach-O weak coalescing then exposed an object-level metadata bug. The linker
coalesced hidden-weak CGU copies to one text address but retained multiple FDEs
at that address. Each FDE pointed at the LSDA generated for its original copy,
whose post-monomorphization call layout could differ. The system unwinder could
select a stale record, fail to find the active call site, and return
`_URC_FATAL_PHASE1_ERROR`. A traced Ruff failure had two FDEs for the same
`ruff::commands::check::lint_path` address and terminated in exactly that way.
Until Mach-O can associate each FDE and LSDA atom with its weak function atom,
cg_clif keeps CGU-local and fallback copies local when Mach-O unwinding is
enabled. ELF retains hidden-weak coalescing and its size and runtime wins. The
four affected Ruff packages pass all 626 tests with the target-scoped policy,
including every expected-panic case that previously aborted.

### Selective exception metadata and Mach-O unwind atoms

The initial unwinding implementation attached a personality-bearing CIE and
an LSDA to every function whenever cg_clif's `unwinding` feature was enabled.
Functions without exception handlers still need ordinary frame descriptions
so a panic can unwind through them, but on ELF they do not need a personality
or language-specific data area. cg_clif keeps separate plain and augmented CIEs
there and selects the augmented CIE only when a finalized machine call site has
an exception handler. LSDA generation follows the same condition.

Mach-O is the deliberate exception. Its FDEs use one consistent augmented CIE
shape. Functions with machine call sites receive a real LSDA that includes
unwind-through entries for ordinary calls; truly call-free functions encode a
null LSDA. Encoding that null required preserving DWARF's all-zero sentinel
rather than applying the PC-relative base to zero. This avoids both nounwind
gaps and empty tables while keeping Apple's linker and system unwinder on one
CIE shape.

The original selective Mach-O experiment changed metadata rather than machine
text. Before the final suite exposed the duplicate-FDE boundary, its pinned
profiling binaries shrank substantially while their `__text` section sizes
remained identical. These are historical measurements; the final Mach-O policy
does not claim this file-size saving.

| Binary | Previous Cranelift | Selective LSDA | Change | Candidate / LLVM |
| --- | ---: | ---: | ---: | ---: |
| uv | 293.8 MB | 276.9 MB | -5.76% | 2.77x |
| Ruff | 141.3 MB | 133.2 MB | -5.72% | 2.96x |
| ty | 145.5 MB | 136.7 MB | -6.08% | 2.88x |

For example, Ruff's `__gcc_except_tab` falls from 5.66 MB to 3.09 MB,
`__unwind_info` from 2.99 MB to 1.91 MB, `__eh_frame` from 7.98 MB to
7.24 MB, and `__LINKEDIT` from 69.73 MB to 66.03 MB. uv's exception table
falls from 12.52 MB to 7.41 MB, and ty's from 5.54 MB to 2.77 MB.

The complete 50-run runtime gate is neutral, as expected for metadata that is
not read on the successful benchmark paths:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -0.66% | 28/50 | 0.47989 |
| `uv lock --check`, offline | +0.20% | 22/50 | 0.47989 |
| `ruff check` over 1,592 fixtures | +0.11% | 24/50 | 0.88772 |
| `ty check` over `scripts/ty_benchmark` | +0.15% | 22/50 | 0.47989 |

Every application correctness probe retains the exact LLVM and preceding
Cranelift exit code and output digests. A mixed object-level probe contains a
plain `zR` CIE used by 25 ordinary FDEs and an augmented `zLPR` CIE used by
only the five handler-bearing FDEs; its panic is still caught. The complete
stage-2 cg_clif, standard-library, proc-macro, and rustdoc build passes, as does
uv's exact
`keyring::tests::fetch_url_no_host` expected-panic regression. The full
repository-suite rerun is recorded separately below.

### Dead nontrapping load elimination

Machine lowering previously treated every load as a side-effecting root. That
is necessary for lowering's memory-order colors and its rules for folding and
sinking live loads, but it is too conservative for root emission: an unused
load marked `notrap` has no observable effect. The retained change separates
those two decisions. Every load keeps the existing lowering-side-effect color
and live-load behavior, while an instruction is emitted as an unused root only
when it has an observable side effect. Unused potentially trapping loads still
execute and preserve their trap.

A focused AArch64 filetest covers both sides of the boundary: an unused
`notrap` load compiles to `ret`, while an otherwise identical trapping load
still emits `ldr` with its heap-out-of-bounds trap. In the profiled ty binary,
the representative hot
`RawTableInner::find_or_find_insert_index_inner` body shrinks from 700 to 640
bytes by dropping dead literal-pool loads without changing the placement of
live loads.

The complete 50-run matched gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -2.16% | 34/50 | 0.01535 | +65.2% | -0.424% |
| `uv lock --check`, offline | -0.07% | 25/50 | 1.00000 | +42.0% | -0.424% |
| `ruff check` over 1,592 fixtures | -0.22% | 26/50 | 0.88772 | +79.7% | -0.612% |
| `ty check` over `scripts/ty_benchmark` | +0.32% | 21/50 | 0.32224 | +129.4% | -0.679% |

uv environment creation improves decisively; the other three runtime rows are
statistically neutral; and every binary shrinks by 0.4-0.7%. All application
exit codes and output digests match LLVM and the preceding Cranelift build.
The complete stage-2 backend, standard-library, and proc-macro build, all 186
`cranelift-codegen` unit tests, 21 passing doctests, and all 1,217 Cranelift
filetests pass. This is retained as a broadly useful dead-code and code-size
improvement with one demonstrated application-runtime win.

### Local dead-store elimination across readonly loads

Cranelift's alias analysis documented dead-store elimination as future work,
but the general transform is constrained by trapping stores and by loads that
may observe the overwritten state. A retained local subset now removes an
earlier ordinary store only when a later nontrapping store in the same block
has the exact same SSA address, offset, and type. Any ordinary load, other
store, call, trap, fence, or opaque side effect ends the proof. A nontrapping
`readonly` load may remain between the stores because its memory cannot alias
the write and it cannot expose the earlier state by trapping.

cg_clif supplies the missing Rust alias fact conservatively. Direct shared
function arguments whose pointee is `Freeze` already receive `ReadOnly` and
`NoAlias` ABI attributes under the LLVM backend. cg_clif now preserves the
same fact on the direct dereference and its derived field/index pointers, so
their loads carry Cranelift's `readonly` flag. Copies of the reference, raw
pointers, mutable references, non-`Freeze` pointees, and indirect ABI shapes
remain unqualified.

The complete 50-run matched gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -2.41% | 33/50 | 0.03284 | +62.4% | -0.004% |
| `uv lock --check`, offline | -1.57% | 33/50 | 0.03284 | +41.2% | -0.004% |
| `ruff check` over 1,592 fixtures | -0.30% | 27/50 | 0.67181 | +76.0% | -0.019% |
| `ty check` over `scripts/ty_benchmark` | -0.86% | 32/50 | 0.06491 | +125.9% | -0.017% |

Both uv workloads improve decisively, Ruff remains neutral, ty trends toward
an improvement, and all application exit codes and output digests match. The
binaries shrink by 10,384 bytes for uv, 25,104 bytes for Ruff, and 23,232
bytes for ty.

The profiled `Type::hash` body identifies the boundary of this subset. Its
shared `Type` loads are now correctly `readonly`, but every imported hash
update is separated from the next by a real branch with a return or trap path.
Following unconditional jump scaffolding across blocks therefore removes none
of its 102 stores and was rejected. Collapsing that body toward LLVM's 664-byte
shape needs memory-SSA promotion that carries the hasher state through
branches and materializes it at observers, not a broader local dead-store
rule or a terminal-only store sink.

### Rejected terminal-store sinking

The fresh profile after the dead-slot trivial-import change removed
`BuildHasherDefault<FxHasher>::build_hasher` from the active leaves. It put
hashbrown's `find_or_find_insert_index_inner` first and `Type::hash` second.
Four hot copies of `Type::hash` were still 6,100 bytes apiece, with 102 stores
through the hasher pointer and 31 returns in each copy.

A deliberately narrow prototype grouped identical ordinary `store notrap`
instructions with the same resolved address, offset, type, and memory flags.
It shared a store only when every source path had completed all other work and
reached a void return through `nop`s and argument-free unconditional jumps.
It would not cross a call, load, trap, other memory operation, potentially
trapping store, or value-returning path. Focused positive and barrier
filetests, all 186 `cranelift-codegen` unit tests, and 21 passing doctests
passed.

The prototype reduced each profiled `Type::hash` copy from 102 stores to 29,
from 31 returns to 9, and from 6,100 to 5,744 bytes. The complete 50-run
application gate nevertheless rejected it:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +3.84% | 7/50 | 0.00000021 |
| `uv lock --check`, offline | +3.09% | 12/50 | 0.00031 |
| `ruff check` over 1,592 fixtures | +0.20% | 25/50 | 1.00000 |
| `ty check` over `scripts/ty_benchmark` | +0.56% | 17/50 | 0.03284 |

Every candidate correctness probe retained the preceding Cranelift exit code
and output digests. Both uv workloads and ty regress decisively while Ruff is
neutral, so the prototype is not retained. The local code-size win is real,
but merging cold terminal paths changes layout and adds value-carrying edges
without eliminating the hot-path hasher traffic. The next memory-state arc
must promote or forward the hasher value across the active branch region and
materialize it only at true observers; terminal tail sharing alone optimizes
the wrong boundary.

A direct audit of the retained optimized CLIF later showed that this proposed
whole-CFG promotion is already present in the relevant sense. Each
`Type::hash` copy has one initial load of the `FxHasher` state, then carries
that value through SSA computations across its dispatch tree. The 102 stores
are on mutually exclusive leaf paths, so an ordinary invocation executes only
one of them. The body has one nested call that genuinely observes and mutates
the hasher, one cold trap, 31 returns, 15 branch tables, and roughly 650 blocks.
Adding a state block parameter to every non-entry block would therefore remove
no repeated hot-path load and would merely move the leaf materializations to
returns, traps, and the real call boundary. That converges on the rejected
terminal-store experiment while adding far more value-carrying edges. The
whole-CFG memory-state prototype is rejected before a rebuild on structural
grounds. Closing the remaining `Type::hash` gap needs better enum dispatch,
tail/code-layout compaction, or a machine-level transform that does not add
hot branch edges; generic memory-SSA promotion is not the missing optimization
for this body.

### Rejected loop-wrapper body import

The matched Cranelift and LLVM profiles ruled out the apparent `memmove` lead
as a backend-specific lowering gap. Both binaries call Darwin's tuned routine
with dynamic byte counts for real vector and table relocation, and the LLVM ty
binary contains more static `memmove` references than the retained Cranelift
binary. The sharper difference was parameter hashing: retained Cranelift kept
an 88-byte `Parameter::hash_slice` loop wrapper and called a roughly 736-byte
`Parameter::hash` body on every iteration, while LLVM fused the parameter hash
into its slice loop.

A bounded post-monomorphization import prototype targeted exactly that shape.
It recognized void wrappers with at most 24 blocks and 96 live instructions,
exactly one direct result-free call in a natural loop, and no other call. Only
that call could import a larger callee: source MIR remained capped at 64 blocks
and 512 operations, and optimized CLIF at 160 blocks and 384 instructions with
no live stack slot, dynamic stack slot, global value, stack limit, or inline
assembly. Existing repeated-call and trivial-import rules remained unchanged.

The intended transformation fired. The standalone `Parameter::hash` symbol
and loop call disappeared, and its dispatch body moved into
`Parameter::hash_slice`. The policy also shrank uv by 47,200 bytes (0.03%),
Ruff by 898,496 bytes (0.84%), and ty by 1,948,752 bytes (1.73%). A 20-run
ty-only screen initially measured -1.81% paired with 13/20 wins, but a
100-run screen settled at -0.65% with 59/100 wins (`p = 0.08863`). The
complete 50-run three-lane application gate then measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +0.55% | 23/50 | 0.67181 | +63.9% |
| `uv lock --check`, offline | +0.51% | 23/50 | 0.67181 | +37.7% |
| `ruff check` over 1,592 fixtures | -0.22% | 26/50 | 0.88772 | +77.8% |
| `ty check` over `scripts/ty_benchmark` | -0.44% | 29/50 | 0.32224 | +117.4% |

Every LLVM, retained-Cranelift, and candidate exit code and output digest
matched. cg_clif check, the complete stage-2 backend build, and the matching
stage-2 standard-library and proc-macro build passed. Results are in
`/Users/zanie/code/tmp/cranelift-runtime-performance/results/loop-wrapper-import-all50.json`.

The policy is rejected for runtime. It makes a genuine size improvement, but
neither the target ty operation nor any other workload improves decisively,
and both uv rows move slightly backward. Importing a large dispatch body into
a hot loop is therefore not justified by loop placement alone. The remaining
call arc needs a stronger dynamic frequency and cost signal, or a transform
that also reduces the imported body's branches and observer calls rather than
only deleting the outer call boundary.

### Rejected call-bounded wide-constant sharing

The earlier whole-function multiplier hoist had kept the value in a
callee-saved register, added spills, and left 26 calls in `Type::hash`. After
the retained repeated-call and trivial-import policies removed those call
boundaries, the fresh active copy still contained 306 `movk` instructions.
A follow-up therefore reused only a highly repeated wide `i64` constant's
nearest dominating definition and discarded all available definitions at
every call. It did not move an instruction to the entry block, preserve a
value across a call, or change the CFG.

This was a much stronger local result than entry hoisting. The active
`Type::hash` copy fell from 6,100 to 4,484 bytes (-26.5%) and from 1,525 to
1,121 instructions. `movk` fell from 306 to 3 and `mov` from 264 to 163,
while the 102 stores, 31 returns, and frame/link-register-only prologue were
unchanged. A focused AArch64 filetest verified that eight repeated constants
share one definition on each side of a call, while the call remains a hard
boundary.

The broad call-bounded candidate nevertheless remained runtime-neutral in the
complete 50-run gate:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +0.04% | 24/50 | 0.88772 |
| `uv lock --check`, offline | +0.66% | 23/50 | 0.67181 |
| `ruff check` over 1,592 fixtures | +0.26% | 22/50 | 0.47989 |
| `ty check` over `scripts/ty_benchmark` | -0.45% | 30/50 | 0.20264 |

A target-cost refinement limited the transform to AArch64 constants that
need all four 16-bit materialization pieces: every lane differs from both the
zero and all-ones bases. It preserved the complete `Type::hash` structural
win and stopped changing cheaper constants, but only reshuffled neutral
signals:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +1.27% | 20/50 | 0.20264 |
| `uv lock --check`, offline | +0.01% | 25/50 | 1.00000 |
| `ruff check` over 1,592 fixtures | -0.72% | 27/50 | 0.67181 |
| `ty check` over `scripts/ty_benchmark` | +0.75% | 21/50 | 0.32224 |

All application exit codes and output digests matched. Both prototypes passed
their focused filetest, `cranelift-codegen` check, stage-2 backend build, and
standard-library/proc-macro build. Neither improves a representative workload,
so both are rejected. The repeated literals are substantial static bloat, but
they are not the measured runtime bottleneck. Further `Type::hash` work should
reduce dynamic hasher stores, branches, or observer traffic rather than share
another literal.

### Rejected one-off tail-wrapper import

The same retained profile put `Type::eq` among the hottest Rust leaves. Its
dominant nominal-instance arm calls a 24-byte
`NominalInstanceType::eq` wrapper whose only work is to call
`NominalInstanceInner::eq`, normalize the boolean result, and return. A
bounded post-monomorphization experiment tried to import one-off wrappers with
exactly one direct call and otherwise only `nop`, jump, and return
instructions. It deliberately left the real callee out of line.

The first two-block post-translation gate admitted nothing because cg_clif's
synthetic entry and MIR blocks make even this wrapper three CLIF blocks.
Allowing the existing composed-block ceiling reached the intended wrappers,
but the stage-2 compiler build then failed while linking libc's build script:
the imported wrapper referenced a local cross-CGU
`SpecToString::spec_to_string` symbol that was declared but not materialized
in the caller CGU. The candidate stopped before application builds or timing.

This validates the call-free invariant on imported catalogue bodies. A local
callee cannot safely survive cross-CGU composition merely because the wrapper
is trivial. Cranelift's existing `return_call` is not a drop-in alternative:
it requires the special `tail` calling convention, while cg_clif emits Rust's
system ABI and explicitly does not support Rust tail calls. Removing this
boundary requires a proven system-ABI sibling-call subset, or a complete
availability and emission model for the transitive callee; relaxing the
one-off import structure is rejected.

The focused barrier filetest, all 1,218 Cranelift filetests, all 186
`cranelift-codegen` unit tests, 21 passing doctests, a complete cg_clif check,
and the full stage-2 backend, standard-library, and proc-macro build pass.

#### Rejected tail-wrapper import retry with complete materialization

The one-call wrapper policy was retried after post-inlining references became
materializable. The exact hinted-or-unhinted gate admitted only candidates
with one direct call and otherwise `nop`, jump, or return instructions under
the existing six-block and 32-instruction ceilings. Making this policy link
through the stage-2 bootstrap exposed three distinct availability gaps in the
prototype: a materialized definition was redeclared with local linkage,
already-materialized post-inlining roots did not carry their recorded callee
edges into the stronger closure, and generated MIR shims such as drop glue
were excluded from that closure. Preserving the hidden-weak declaration,
seeding only references reachable from newly live roots, and admitting
MIR-backed non-intrinsic, non-virtual shims made the complete stage-2 compiler,
standard-library, proc-macro, and backend build pass.

The intended ty transform then occurred. `NominalInstanceType::eq` disappeared
from the binary and the nominal arm in `Type::eq` called
`NominalInstanceInner::eq` directly. Because that inner definition crossed a
CGU boundary, however, the call became an `adrp`/`add`/`blr` sequence to a
hidden-weak target rather than a direct `bl`. `Type::eq` grew from 1,092 to
1,116 bytes. The complete binaries nevertheless shrank by 3,095,184 bytes for
uv, 3,240,128 bytes for Ruff, and 538,416 bytes for ty, showing that the
runtime result was not a simple static-size effect.

The matched 20-run three-lane gate measured the candidate against the retained
Cranelift control:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -3.06% | 16/20 | 0.01182 | +64.0% |
| `uv lock --check`, offline | +0.02% | 10/20 | 1.00000 | +45.2% |
| `ruff check` over 1,592 fixtures | +4.24% | 5/20 | 0.04139 | +91.2% |
| `ty check` over `scripts/ty_benchmark` | +7.43% | 2/20 | 0.00040 | +141.4% |

All application exit codes and output digests matched. The exact backend is
saved as `one-call-wrapper-complete-materialization/candidate.dylib` with
SHA-256 `a6e2a25cfbc28cce28eb6219ed064b84bea7a4ac715641f2f3370d8aed62814c`;
the trial record is
`bench-one-call-wrapper-complete-materialization-all20/results.json`.
The broad policy is rejected and its source changes are reverted. The retained
backend was restored. Eliminating a 24-byte wrapper is insufficient when it
turns direct calls into indirect cross-CGU calls and perturbs many unrelated
wrappers. Further `Type::eq` work should either preserve direct-call locality
or optimize the inner equality body itself.

### Rejected frequent multi-failure guarded import

The fresh retained profile left `boxcar::Index::location` as another prominent
Cranelift-only leaf. The retained ty binary had 253 direct calls across ten
CGU-local copies, while LLVM had absorbed every copy. The earlier
module-frequency experiments could not reach this helper: its initial CLIF had
nine blocks, four sized stack slots, and two cold failure calls, so it failed
the stack-free and single-failure guarded-import boundary.

A phase-correct retry translated an unhinted root only when its source MIR fit
the existing ten-block, 96-operation guarded ceiling and contained at least
two calls. It pre-optimized only stack-bearing catalogue bodies, counted live
rather than removed DFG instructions, and admitted a direct store only when it
targeted the function's struct-return pointer. Multiple cold failure blocks
were replaced with calls back to the canonical helper followed by their
original traps. The optimized `location` candidate had seven blocks, 25 live
instructions, no live stack slot, and one merged cold exit. To avoid making
aggregate popularity a general hotness proxy, only candidates with at least
two cold exits in the original body and eight calls in the codegen unit used
the module-frequency exception; existing hinted constant specialization
remained limited to single-failure candidates.

The intended transformation fired. Direct ty calls to `Index::location` fell
from 253 to 14. The remaining calls preserve canonical cold behavior or occur
in low-frequency codegen units. uv, Ruff, and ty grew by 12,288, 9,808, and
26,512 bytes, respectively. A 100-run ty-only screen was directionally
favorable but not decisive at -0.57% paired with 58/100 wins
(`p = 0.13321`). The complete 50-run three-lane application gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +1.74% | 16/50 | 0.01535 | +63.5% |
| `uv lock --check`, offline | -0.53% | 26/50 | 0.88772 | +40.2% |
| `ruff check` over 1,592 fixtures | -0.78% | 28/50 | 0.47989 | +83.0% |
| `ty check` over `scripts/ty_benchmark` | +0.82% | 24/50 | 0.88772 | +125.5% |

All application exit codes and output digests matched. The complete stage-2
compiler, backend, standard-library, and proc-macro build passed. The exact
backend is saved as
`multi-failure-guarded-frequency/stage2-candidate.dylib` with SHA-256
`73a083d9ef61df28725378b2e22f94f124e88710134d5612927458e0349fc5c3`;
the final trial record is
`bench-multi-failure-guarded-frequency-all50/results.json`.

The policy is rejected because it decisively regresses uv environment
creation and does not improve the target ty row. Removing the hot helper call
without reducing its aggregate-return stores or surrounding caller traffic is
not profitable. A future `boxcar` arc should simplify the complete index and
bucket computation in the caller, not broaden post-monomorphization import
availability again.

### Rejected readonly-load tail merging

The next retained profile put `NominalInstanceInner::eq` among ty's hottest
remaining Cranelift-only leaves. LLVM compiled the equivalent nested-enum
equality wrapper to roughly 200 bytes, while Cranelift repeated the same
`salsa::Id` payload comparisons across multiple arms in a 628-byte body.

A bounded post-egraph prototype structurally matched only functions with at
most 128 blocks and blocks with at most eight instructions. It could prove
equivalence across pure parameter-forwarding trampolines, but redirected only
duplicate tails containing a nontrapping readonly load. Calls, stores, traps,
other side effects, differing instruction metadata, and mismatched external
SSA values or branch arguments remained barriers. The extracted hot CLIF body
fell from 608 to 344 bytes, and the real ty symbol fell from 628 to 364 bytes
(-42.0%). uv was byte-identical, Ruff shrank by 16,848 bytes, and ty grew by
2,656 bytes overall.

The complete stage-2 compiler, backend, standard-library, and proc-macro build
passed. All 187 `cranelift-codegen` library tests, all 1,218 CLIF filetests,
and fresh uv, Ruff, and ty builds also passed. The randomized 20-run
three-lane application screen preserved every exit code and output digest but
showed no runtime benefit:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +0.21% | 9/20 | 0.82380 | 1.61x |
| `uv lock --check`, offline | -0.58% | 12/20 | 0.50344 | 1.37x |
| `ruff check` over 1,592 fixtures | +0.46% | 9/20 | 0.82380 | 1.78x |
| `ty check` over `scripts/ty_benchmark` | +0.005% | 10/20 | 1.00000 | 2.17x |

The exact backend is saved as
`readonly-load-tail-merge/stage2-candidate.dylib` with SHA-256
`d534b6e8312041737a703c6afe3effa5b577be15210f46d21235b82665a20969`;
the screen is recorded in
`results/readonly-load-tail-merge-screen20.json` under the benchmark scratch
directory.

The prototype is rejected and its source changes are reverted. Tail merging
is a real local code-size win, but this equality body is too small a fraction
of the user-visible ty operation to justify another global pass. Further work
on this hotspot needs to eliminate surrounding enum dispatch or combine with a
broader proven transformation before another application gate.

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

### Coalesced CGU-local function copies

Rust's partitioner gives every codegen unit a private copy of reachable
drop glue, external generics, and local `#[inline]` functions. LLVM can inline
or erase most of those copies before object emission. Cranelift instead kept
dozens of identical hot bodies under object-local symbols, so the static
linker had no opportunity to choose one definition for the final executable.
In the retained ty binary, for example, 31 symbol-table entries matched
`RawTableInner::find_or_find_insert_index_inner`, and 554 matched pointer
`precondition_check` helpers.

Cranelift's module linkage model now has a weak linkage whose symbol remains
hidden to the current static linkage unit. cg_clif selects it only for the
partitioner's `inlined` function copies with internal Rust linkage and default
visibility. Object emission maps it to a linkage-scoped weak symbol: ELF,
Mach-O, and COFF serialization tests verify the representation. The symbol is
not exported or dynamically preemptible, but another codegen unit's definition
with the same Rust symbol name may replace it during static linking. The
retained ty binary has two matching hashbrown probe entries and 25 matching
precondition helpers after coalescing.

The 50-run matched application gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -3.07% | 38/50 | 0.00031 | +62.3% | -33.85% |
| `uv lock --check`, offline | -2.38% | 33/50 | 0.03284 | +40.8% | -33.85% |
| `ruff check` over 1,592 fixtures | -1.24% | 30/50 | 0.20264 | +79.4% | -16.82% |
| `ty check` over `scripts/ty_benchmark` | -1.79% | 35/50 | 0.00660 | +119.9% | -13.58% |

Every application median improves, the uv and ty changes are statistically
significant, and every correctness-probe exit code and output digest matches
both the previous Cranelift policy and LLVM. Ruff's direction is positive but
not independently significant. The profiling binaries fall from 272.6 to
180.4 MB for uv, 129.2 to 107.5 MB for Ruff, and 130.6 to 112.9 MB for ty.
The focused Cranelift module and object suites pass, including emitted weak
symbol checks for x86-64 ELF, AArch64 Mach-O, and x86-64 COFF. A complete
stage-2 cg_clif, standard-library, and proc-macro build and all three pinned
application builds also pass. The evidence is recorded in
`bench-hidden-weak-all50/results.json` under the benchmark scratch directory.

### Materialized post-monomorphization references

The rejected one-off tail-wrapper experiment exposed a correctness hole that
was independent of its profitability policy. Optimized CGU partitioning may
omit local copies and conflict-mangled global copies on the assumption that
the backend will inline every reference. cg_clif's bounded inliner may retain
one of those calls or function addresses. It may also introduce a reference
only after partitioning, when no CGU has been assigned responsibility for its
definition. Function references nested in constants and allocations were not
part of cg_clif's existing direct-call census either.

cg_clif now records direct function constants and nested allocation
references, carries references introduced while remapping imported bodies
back to the destination module, and materializes the transitive closure of
available definitions that cannot otherwise be assumed to exist. The emitted
fallback definitions use the same hidden weak linkage as other partitioner
local copies, so duplicate CGUs may still coalesce them. Naked and explicitly
external functions remain outside this fallback.

This fixes the undefined `Display::fmt` references that appeared while linking
the `psm` and `libc` build scripts with Apple's linker. Both reproductions and
the complete stage-2 compiler and cg_clif build now pass. The change is kept as
a correctness prerequisite and committed independently from the aggregate
optimization below.

The final Ruff/ty suite exposed a second edge in that same correctness
boundary. Emitting a hidden-weak fallback for
`Box::new<ArcInner<snapbox::Assert>>` introduced cleanup drop glue that had not
been visible to CGU partitioning. The original closure mixed newly discovered
references back into the ordinary CGU reference map, where the nonempty
`DropGlue` instance was neither an ordinary MIR item nor a LocalCopy. Another
CGU contained only a private definition, so the fallback object failed to link.

Fallback materialization now consults the complete partition metadata for
nonempty drop glue that it discovers. If every partition-assigned definition
is object-private and the current CGU does not already define it, cg_clif emits
the glue and its codegenable transitive closure privately in the fallback
object. Glue with any externally linkable definition keeps the existing path,
as do unrelated references. This repairs only genuinely unresolved late
references instead of cloning ordinary glue graphs or globally retaining their
code. The exact failing Ruff fixture now links from a clean target with a
private, self-contained glue closure in the fallback CGU.

The first remote Linux stage-2 build exposed the corresponding ordinary-item
edge. Cranelift linked `zerovec_derive` with a function address for the private
`Debug::fmt` implementation of its `OwnULETy`, while the only definition
remained internal to another CGU. Loading the proc-macro therefore failed with
an undefined Rust symbol before the application suites could start.

cg_clif now computes the set of functions whose complete partition metadata
contains only non-inlined internal definitions once before parallel codegen.
Before post-monomorphization processing, a CGU that actually retains a
reference to one of those functions emits a local definition unless it already
owns one. This is deliberately narrower than closing over every late ordinary
reference: transitive private items are discovered by the same partition set,
while externally available dependencies retain their existing linkage. The
late drop-glue closure remains the only path that recursively materializes
definitions absent from partition metadata. A clean Linux stage-2 build and
the full cross-platform application suites are the acceptance gate for this
extension.

### Direct construction into indirect aggregate returns

A fresh amplified ty profile after CGU-local coalescing again put Darwin
`memmove` near the top. Interposition attributed three copies of 488, 488, and
496 bytes to `ConstraintSetBuilder::new`. Optimized MIR first constructed a
`ConstraintSetStorage` temporary, copied it into `UnsafeCell`, copied that into
`RefCell`, and finally copied the wrapper into the indirect return place.
LLVM's later SROA removes the chain; cg_clif previously allocated every MIR
temporary independently and preserved it.

cg_clif now recognizes a conservative destination chain for memory-backed
temporaries. The source must have exactly one whole-local definition and one
use, its address must never be observed, its type must exactly match the
nonzero field layout, and the chain must terminate at an indirect return
place. Scalar and scalar-pair SSA values are explicitly excluded. Eligible
temporaries are allocated directly in the final nested return field. The
ordinary by-reference assignment recognizes the resulting identical source
and destination pointer and omits the self-copy. Tuples, enum variants,
unions, address-observed locals, direct returns, and ambiguous use chains keep
the existing path.

In the exact profiled constructor, machine code falls from 1,048 to 628 bytes,
the 1,760-byte frame disappears, and all three libc copies are gone. Fresh,
matched application binaries shrink by 314,048 bytes for uv (0.174%), 86,736
bytes for Ruff (0.081%), and 152,256 bytes for ty (0.135%). A 4,096-copy
amplified ty corpus produces the identical diagnostic-stream checksum in the
control and candidate.

The complete 50-run matched operation gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -0.80% | 30/50 | 0.20264 | 1.61x | -0.174% |
| `uv lock --check`, offline | -0.17% | 26/50 | 0.88772 | 1.40x | -0.174% |
| `ruff check` over 1,592 fixtures | +0.06% | 24/50 | 0.88772 | 1.78x | -0.081% |
| `ty check` over `scripts/ty_benchmark` | -1.76% | 33/50 | 0.03284 | 2.19x | -0.135% |

Ty reaches the sign-test threshold independently, both uv rows are favorable
or neutral, Ruff is neutral, every binary shrinks, and every
correctness-probe exit code and output digest matches. A separate 20-run
three-lane screen supplies the LLVM ratios in the table. The complete stage-2
compiler, cg_clif, standard-library, and proc-macro builds and all three fresh
application builds pass. The optimization is retained; the full matched
repository-suite rerun remains the finalization gate.

#### Rejected transparent aggregate call aliases

The retained ty profile exposed the opposite copy direction in
`salsa::ActiveQuery::add_read_simple`. Its incoming `DatabaseKeyIndex` is a
12-byte indirect, readonly argument. MIR wraps it in the one-field
`QueryEdge` struct and immediately moves that wrapper into
`IndexMap::insert_full`. LLVM reuses the incoming pointer, combines the two
preceding min/max diamonds with conditional selects, and tail-calls the map
operation. cg_clif instead copied the three four-byte fields into a private
stack slot and passed that slot.

A conservative frontend prototype let a memory-backed one-field aggregate
temporary alias its source storage only when both layouts had identical size
and alignment, the field started at offset zero, neither local's address was
observed, the source had no other use, and the wrapper's sole use moved it
directly into a call. Scalar and scalar-pair values, multi-field aggregates,
enums, non-call consumers, copies with later source uses, and ambiguous local
lifetimes remained unchanged. Focused `Copy` and owning `String` wrapper
examples both passed; the 12-byte example compiled to only prologue, call, and
epilogue, confirming that the loads, stores, and local slot disappeared
without duplicating a drop.

The real `ActiveQuery::add_read_simple` symbol fell from 140 to 108 bytes and
`ActiveQuery::add_read` from 292 to 252 bytes. The complete stage-2 compiler,
backend, standard-library, proc-macro, and pinned application builds passed.
The uv, Ruff, and ty binaries grew slightly, by 5,264, 4,192, and 5,488 bytes
(all less than 0.005%). The randomized 20-run gate preserved every correctness
probe but was runtime-neutral:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | +2.34% | 9/20 | 0.82380 | 1.59x |
| `uv lock --check`, offline | +0.48% | 9/20 | 0.82380 | 1.42x |
| `ruff check` over 1,592 fixtures | +0.001% | 10/20 | 1.00000 | 1.80x |
| `ty check` over `scripts/ty_benchmark` | -0.41% | 11/20 | 0.82380 | 2.24x |

The exact backend is saved as
`transparent-aggregate-alias/candidate.dylib` with SHA-256
`ca1f8bc109c4caf3c29410291b2f7fab1cc0eb152d2b6d830dfa09640cef9b26`;
the screen is recorded in
`results/transparent-aggregate-alias-screen20.json` under the benchmark
scratch directory. The rule is rejected without a 50-run gate, and its source
changes are reverted. LLVM's compact wrapper combines input reuse, select
formation, and a tail call; removing only the three-field copy does not move
the application boundary.

### Rejected immutable aggregate vector copies

The profile after direct return construction still sampled
`ConstraintSetBuilder::new` 568 times as an active leaf. Its remaining 628-byte
body copied the same immutable 32-byte aggregate constant into thirteen return
fields using four integer loads and four stores per field. LLVM's 240-byte body
uses two 128-bit loads and stores. Merely preserving the constant allocation's
read-only provenance did not change Cranelift's machine code: its load
optimization does not currently reuse those values across the intervening
stores.

An Apple-AArch64 experiment used 128-bit vector registers for immutable
16-128-byte copies. The constructor fell to 420 bytes, but the exact 20-run
application screen was mixed:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -0.86% | 13/20 | 0.26318 |
| `uv lock --check`, offline | +0.11% | 10/20 | 1.00000 |
| `ruff check` over 1,592 fixtures | +1.31% | 7/20 | 0.26318 |
| `ty check` over `scripts/ty_benchmark` | -0.71% | 12/20 | 0.50344 |

The uv, Ruff, and ty binary changes were -29,776, +6,272, and -24,032
bytes. Narrowing the rule to the exact profiled 32-byte size made every binary
smaller by 9,472, 7,520, and 5,376 bytes, respectively, but reversed the
runtime signal:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | +1.55% | 7/20 | 0.26318 |
| `uv lock --check`, offline | -0.23% | 10/20 | 1.00000 |
| `ruff check` over 1,592 fixtures | +0.17% | 10/20 | 1.00000 |
| `ty check` over `scripts/ty_benchmark` | +0.80% | 7/20 | 0.26318 |

Neither policy proceeds to 50 runs. The structurally attractive constructor
change is too local, and the broader policy trends backward for Ruff. This
experiment also exposed a benchmark provenance requirement: spelling the candidate
backend as `cranelift` and the control as an absolute path changes crate
disambiguators and produced false megabyte-scale size differences. The valid
pairs loaded both backend dylibs through the same fixed path and have matching
Rust symbol hashes. Results are in
`bench-readonly-vector-exact-all20/results.json` and
`bench-readonly-vector32-exact-all20/results.json` under the benchmark scratch
directory.

### Exact unsigned branch-fact propagation

The retained ty profile next put `rustc_hash::hash_bytes` at 862 active-stack
samples. Its 1,064-byte Cranelift body repeated literal unsigned length checks
after the same condition had already selected the only incoming edge. The
`len >= 8` path rechecked both `len >= 8` and its complement `len < 8`; the
four-byte path repeated the same pair. Each redundant branch kept a cold
bounds-panic block and its call sequence alive. LLVM absorbs this helper
completely and has no standalone symbol.

Cranelift now propagates one exact unsigned comparison fact through blocks
with a single predecessor. A fact may pass through an unconditional jump and
matches an identical, complemented, or operand-swapped comparison. Integer
constants are compared by typed value rather than SSA identity, so separately
materialized literals still match. The rule deliberately excludes signed and
equality comparisons, blocks with multiple incoming edges, transitive
arithmetic implications, and global replacement of the pure comparison value.
Only the later `brif` condition becomes constant; other uses retain their
original semantics.

All four target branches disappear from `hash_bytes`, whose machine body falls
from 1,064 to 936 bytes. The rule also removes repeated bounds branches more
broadly: fresh uv, Ruff, and ty binaries shrink by 309,424 bytes (0.172%),
418,464 bytes (0.390%), and 154,688 bytes (0.137%), respectively.

The complete 50-run matched operation gate measured:

| Workload | Paired change | Wins | Sign p | Candidate vs LLVM | Binary change |
| --- | ---: | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -1.38% | 36/50 | 0.00260 | 1.59x | -0.172% |
| `uv lock --check`, offline | +1.02% | 21/50 | 0.32224 | 1.39x | -0.172% |
| `ruff check` over 1,592 fixtures | -0.72% | 29/50 | 0.32224 | 1.78x | -0.390% |
| `ty check` over `scripts/ty_benchmark` | -0.26% | 27/50 | 0.67181 | 2.15x | -0.137% |

The uv lock row reversed an initially favorable 20-run screen, so it received
an independent focused 50-run replication. The replication measured -0.54%
with 32/50 wins and `p = 0.06491`. Thus uv environment creation improves
decisively, the independent lock result is favorable, Ruff and ty remain
favorable or neutral, every binary shrinks, and all application exit codes and
output digests match. The bounded rule is retained.

All 186 `cranelift-codegen` library tests, all 1,219 CLIF filetests, the focused
true-edge, false-edge, complement, swapped-operand, jump-chain, merge, and
signed-comparison regressions, including preservation of non-branch comparison
uses, a no-dependency Clippy run, the complete stage-2
compiler/backend/standard-library build, and fresh pinned uv/Ruff/ty builds pass.
The backend is saved as `unsigned-branch-facts/candidate.dylib` with
SHA-256
`d447feb632c2025bffd7da3b2de558f9d2ead7639d338424c388a8f49c336ac4`.
The main and focused results are recorded in
`results/unsigned-branch-facts-all50.json` and
`results/unsigned-branch-facts-uv-lock-rep50.json`. The full matched repository
suites remain the finalization gate rather than a per-arc prerequisite.

#### Rejected zero-comparison equivalence

The remaining `hash_bytes` bounds path branches first on `len != 0` and then,
in its only successor, on `len >u 0` before loading byte zero. A bounded
follow-up canonicalized only equality and inequality against a typed zero to
the equivalent unsigned `<= 0` and `> 0` predicates. Ordinary equality facts
remained excluded. Focused tests covered both identities, swapped operands,
and rejection of equality against a nonzero constant.

The complete stage-2 build, Clippy, fresh pinned application builds, and all
correctness probes passed. The rule fired more broadly and shrank uv, Ruff,
and ty by another 6,640, 5,232, and 21,216 bytes, respectively. It did not,
however, shrink the target `hash_bytes` machine body below the retained 936
bytes. The complete 50-run operation gate against the exact unsigned-fact
policy measured:

| Workload | Paired change | Wins | Sign p |
| --- | ---: | ---: | ---: |
| `uv venv --clear` | -0.96% | 30/50 | 0.20264 |
| `uv lock --check`, offline | -0.36% | 27/50 | 0.67181 |
| `ruff check` over 1,592 fixtures | +0.23% | 24/50 | 0.88772 |
| `ty check` over `scripts/ty_benchmark` | +0.64% | 22/50 | 0.47989 |

No row improves decisively, Ruff and ty move backward, and the original hot
body is unchanged. The extension is rejected as a size-only win and its source
and focused tests are reverted. The backend is preserved as
`zero-comparison-facts/candidate.dylib` with SHA-256
`9ffc28fbd06e9760224d9ac0a6498569f61938f27512e34db762a712b61fa181`.
The initial screen and complete gate are recorded in
`results/zero-comparison-facts-screen20.json` and
`results/zero-comparison-facts-all50.json`.

### MIR-lifetime physical stack-slot reuse

A fresh ty profile after the branch-fact work put
`TypeRelationChecker::check_type_pair` at 305 active samples in ten seconds.
The retained Cranelift body was 24,240 bytes with a 6,960-byte local frame;
its optimized CLIF contained 627 sized stack slots. The matched LLVM body was
11,852 bytes with a 688-byte local frame. This is a frontend allocation gap as
well as a register-allocation gap: cg_clif had intentionally ignored MIR
`StorageLive` and `StorageDead` statements and reserved independent frame
storage for every memory-backed local.

The first prototype assigned mutually exclusive MIR locals the same Cranelift
stack-slot identity. It cut the hot frame to 3,968 bytes, but it also collapsed
alias identities before optimization, grew the body to about 25,420 bytes, and
regressed the focused ty gate by 2.30% with only 4/20 wins (`p = 0.01182`). That
form is rejected.

The retained design keeps every logical stack slot distinct through Cranelift
optimization and records only that a later slot may reuse an earlier slot's
physical allocation. Machine lowering assigns both slots the same frame offset.
cg_clif computes conflicts with `MaybeStorageLive`, then first-fit colors only
nonzero, memory-backed locals with exact size and alignment. SSA locals,
aggregate destinations, dynamically realigned locals, oversized values, and
functions above the 4,096-local analysis cap keep one allocation per local.
The embedder remains responsible for proving disjoint lifetimes; Cranelift's
verifier additionally rejects invalid, forward, and chained reuse references.
Inlining remaps the allocation reference with the copied slots.

This preserves alias analysis while reducing `check_type_pair` to a 3,952-byte
local frame and 23,556 bytes of code, 43.2% and 2.82% below the retained
Cranelift body. Fresh uv, Ruff, and ty profiling binaries shrink by 181,536,
13,792, and 14,368 bytes, respectively. The randomized 20-run application gate
measured:

| Workload | Paired change | Wins | Sign p | Binary change |
| --- | ---: | ---: | ---: | ---: |
| `uv venv --clear` | -2.17% | 13/20 | 0.26318 | -0.101% |
| `uv lock --check`, offline | +0.76% | 9/20 | 0.82380 | -0.101% |
| `ruff check` over 1,592 fixtures | -1.94% | 16/20 | 0.01182 | -0.013% |
| `ty check` over `scripts/ty_benchmark` | +0.12% | 10/20 | 1.00000 | -0.013% |

Ruff improves decisively, uv environment creation is favorable, and the lock
and ty rows are neutral. Every correctness-probe exit code and output digest
matches the retained backend. AArch64 lowering, inlining, parser, and verifier
filetests cover the physical allocation and structural contract. All 191
`cranelift-codegen` and 41 `cranelift-reader` library tests, all 1,222 CLIF
filetests, a no-dependency Clippy run for both Cranelift crates, and the
complete matching stage-2 backend build pass. The benchmark is recorded in
`results/mir-physical-stack-slot-reuse-all20.json`; the benchmarked backend had
SHA-256 `3bcbe8acad763fb8fc63c086d745380016ad349dec3f5a634e4222a88e5b4c44`.
The final hardened backend, which adds verifier checks and avoids allocating
the quadratic conflict matrix above the cap without changing the benchmarked
paths, is saved as `mir-physical-stack-slot-reuse/candidate.dylib` with SHA-256
`15262c75f834a4637b6d87a4369799da8ce8887b04f3457d7f444eaa48c2a794`.
The full uv, Ruff, and ty repository suites remain the finalization gate.

## Full-Suite Backend Validation

### Final remote cross-platform gate

The final gate ran on dedicated, unsandboxed remote hosts rather than the local
development machine: a 16-vCPU, 64-GB x86_64 Linux instance and a native
12-vCPU, 56-GB Apple-silicon macOS instance. Both hosts used SRS backend source
`7497012ee`, uv `6c963dd3cb0e`, and Ruff/ty `e0eb28d6345b`. LLVM and Cranelift
used fresh, backend-specific target directories. The SRS artifact cache,
incremental compilation, and incremental linker were disabled.

| Host | Suite | LLVM | Cranelift | Interpretation |
| --- | --- | ---: | ---: | --- |
| Linux | uv, complete CI feature set | 3,901 passed, 34 failed, 6 skipped | 3,901 passed, 34 failed, 6 skipped | Exact same failure-name set; unavailable Secret Service/native auth plus three host reflink assumptions |
| Linux | Ruff/ty, all features | 7,962 passed, 44 skipped | 7,962 passed, 44 skipped | Clean |
| Linux | Ruff/ty doctests | Passed | Passed | Clean |
| macOS | uv, complete CI feature set | 3,866 passed, 3 skipped | 3,866 passed, 3 skipped | Clean, including keychain and expected-panic coverage |
| macOS | Ruff/ty, all features | 7,962 passed, 44 skipped | 7,962 passed, 44 skipped | Clean |
| macOS | Ruff/ty doctests | Passed | Passed | Clean |

The Linux uv command intentionally included the unavailable service tests
rather than hiding them. The normalized 34-name failure sets have identical
SHA-256 `dfcbd2d15e2a166e4cd6237556aee52b97061928ac2650e62fd8c07b497fecfd`
in both lanes. The first Linux Ruff/ty pass also demonstrated an identical
14-name harness failure set when nested `cargo locate-project` inherited the
wrong rustup selection; both complete reruns pass with the repository's stable
nested-Cargo environment. On macOS, seven Ruff server integration tests were
rerun in both lanes with each lane's freshly built uv executable on `PATH`.

The gate found and fixed the Mach-O duplicate-FDE and catch-state defects
described above. Earlier suite runs that still contained either defect are
diagnostic history, not final controls.

### Selective-LSDA post-change gate

The selective-LSDA backend reran the complete Cranelift application gate from
fresh target directories at SRS `131741680`. The LLVM source, compiler,
profile, and application revisions are unchanged, and the compiler change is
isolated to `rustc_codegen_cranelift`, so the earlier pinned LLVM lane remains
the control. The operation timings below are useful for detecting gross drift,
not as generated-code measurements.

| Suite and boundary | LLVM control | Previous Cranelift | Selective LSDA |
| --- | ---: | ---: | ---: |
| uv cold build + Nextest | 1,046.07 s | 2,663.75 s | 2,484.31 s |
| uv Nextest phase | 902.769 s | 2,536.885 s | 2,375.965 s |
| Ruff/ty cold build + Nextest | 1,435.69 s | 1,417.33 s | 1,384.31 s |
| Ruff/ty Nextest phase | 1,276.496 s | 1,274.531 s | 1,277.983 s |
| Ruff/ty post-Nextest doctests | 22.49 s | 25.15 s | 24.65 s |

Ruff/ty is the clean invariant. The selective lane completed all 7,962 tests
with the exact same failure-name set as both controls: 7,922 passed, 40 failed,
and 44 skipped. All 40 failures are the known 30-second FSEvents sentinel
waits. The complete doctest command again passes 194 tests, ignores 12, and
has no failures across 48 crates.

uv also completed all 3,866 tests without a compiler crash, but the external
environment had degraded since the pinned control run. It reported 2,932
passed, 927 failed, seven timed out, and three skipped. All 260 previous
Cranelift failure or timeout names are present, plus 674 new names dominated
by blocked package endpoints waiting roughly 40-53 seconds and their resulting
snapshot differences. Those additions give no evidence of a backend
regression, but the environmental drift prevents direct attribution. The three
`uv-auth` expected-panic tests that originally exposed missing Apple-silicon
unwinding all pass in the complete run. The full uv result is therefore a
useful no-crash and unwind-correctness gate, but its failure count and wall
time are not a valid LLVM comparison under the changed network behavior.

### Final policy

The final unwinding-enabled policy ran from clean target directories at SRS
`1ce2acef4`. These are single-run operational timings rather than
block-balanced generated-code benchmarks: they include a cold build, thousands
of short subprocesses, filesystem work, live-network failures, and fixed test
timeouts.

| Suite and boundary | LLVM | Cranelift | Cranelift change |
| --- | ---: | ---: | ---: |
| uv cold build + Nextest | 1,046.07 s | 2,663.75 s | +154.6% |
| uv Nextest phase | 902.769 s | 2,536.885 s | +181.0% |
| Ruff/ty cold build + Nextest | 1,435.69 s | 1,417.33 s | -1.3% |
| Ruff/ty Nextest phase | 1,276.496 s | 1,274.531 s | -0.2% |
| Ruff/ty post-Nextest doctests | 22.49 s | 25.15 s | +11.8% |

The uv lane completed all 3,866 tests in both backends. LLVM reported 3,749
passed, 112 failed, five timed out, and three skipped. Cranelift reported 3,606
passed, 242 failed, 18 timed out, and three skipped. The 117 LLVM failure or
timeout names are all present in the Cranelift set; Cranelift has 143
additional names and LLVM has none that are absent under Cranelift. This host
cannot reach PyPI, the packse fixture site, and several other live endpoints,
and macOS keychain operations return `Operation not permitted`. The additional
Cranelift failures are dominated by those operations reaching command or
Nextest deadlines under slower generated code. They are an important
operational effect, but not evidence by themselves of a miscompile. The four
tracked uv snapshot updates are byte-identical between lanes. Most
importantly, the `uv-auth` expected-panic tests that exposed missing unwinding
now pass.

The Ruff/ty lane completed all 7,962 tests in both backends with the exact same
failure set: 7,922 passed, 40 failed, and 44 skipped. All 40 failures are ty
file-watching tests whose FSEvents sentinel is not delivered in this sandbox;
each waits roughly 30 seconds. Those fixed waits consume almost the entire
Nextest phase and make the apparent tie unsuitable as a runtime-performance
claim. The complete doctest command passes in both lanes.

The Nextest plugin must be invoked directly with the SRS Cargo wrapper in
`CARGO`. `cargo +toolchain nextest` lets the plugin invoke `cargo-srs-real`
directly and bypasses the wrapper's target-codegen flags, silently turning the
purported LLVM lane into Cranelift. With `BACKEND` set separately to `llvm`
and `cranelift`, the matched command shape was:

```bash
CARGO="$HOME/.rustup/toolchains/srs-cranelift-small-copy-stage2/bin/cargo" \
SRS_TARGET_CODEGEN_BACKEND="$BACKEND" \
SRS_CARGO_ARTIFACT_CACHE=0 \
SRS_ARTIFACT_CACHE=0 \
SRS_INCREMENTAL_LINKER=0 \
SLD_INCREMENTAL=0 \
CARGO_INCREMENTAL=0 \
CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang \
RUSTC="$SRS/rust/build/aarch64-apple-darwin/stage2/bin/rustc" \
    cargo-nextest nextest run <repository arguments>
```

uv used its complete no-default-feature CI feature set, `--workspace`, and
`--profile ci-macos`. Ruff/ty used `--all-features --profile ci`, followed by
`cargo test --all-features --doc` with the matched stage-2 rustdoc and backend.
The balanced four-operation matrix above remains the decision gate for
generated-code runtime; the full uv operation is nevertheless a useful warning
that the 1.5-2.3x application gap can cross real test deadlines.

### Earlier 60/500/100 policy

An earlier 60/500/100 candidate also ran the repositories' complete macOS test
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
subsequent scalar boolean-range and boolean-indexed branch-table policies
passed focused scalar, vector, and branch-argument filetests, all 185
`cranelift-codegen` unit tests, cg_clif check, complete stage-2 backend and
standard-library builds, and matched 50-run correctness probes. The current
native `i8x8` policy additionally passed its AArch64 execution example, cg_clif
check, complete stage-2 cg_clif, standard-library, and proc-macro build, and
matched application correctness probes. The rejected same-width stack
reinterpretation experiment then passed endian-specific filetests, the
Cranelift codegen unit and doc tests, cg_clif check, another complete stage-2
cg_clif and standard-library build, and matched application correctness probes.
The retained nonescaping direct-stack policy then passed the same unit and doc
tests, its direct-access and branch-argument escape filetests, standalone
`Iterator::position` and clap reproductions, a complete stage-2 cg_clif,
standard-library, and proc-macro build, pinned application builds, and the
matched four-workload 50-run gate.
The final policy then passed the complete matched application probes and the
full Ruff/ty doctest set, and ran both full Nextest suites to completion without
a compiler crash. Its full-suite gate also found and fixed the scalar `i128`
newtype constant and Apple-silicon unwinding defects described above.
The subsequent selective-LSDA policy passed a complete stage-2 build, a mixed
plain/catching object-level unwind probe, uv's exact expected-panic regression,
fresh pinned application builds, the matched four-workload 50-run gate, all
three complete application suites, and the complete Ruff/ty doctest set.
The current cg_clif sysroot
harness cannot provide additional coverage: its stdlib patch no longer applies
to this Rust snapshot, and its standalone JIT smoke aborts in rustc query TLS
before entering the test program.

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

Run the expensive final suite on a host with its required network, keychain,
filesystem, and FSEvents capabilities. When that is impossible, compare exact
failure identities and tracked snapshot diffs, and report deadline crossings
as operational performance failures rather than silently excluding them or
calling them compiler miscompiles.

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

A final CLIF audit of hashbrown's
`find_or_find_insert_index` did not find that consumer: its inner iteration is
a nonzero bitmask plus count-trailing-zeros operation, not a repeated scalar
bound that an isolated CFG range fact can remove. Guarded
post-monomorphization import now removes pointer-precondition boundaries with
at least one constant argument without cloning their panic paths. Remaining
unspecialized calls need a different interprocedural proof, not another local
range rule. Do not manufacture a generic CFG-range patch without a newly
profiled repeated consumer.

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
than microbenchmarks alone. The `Type::hash` experiments narrow this arc: the
generic integer-constant rematerialization rule was not responsible for its
repeated AArch64 literal sequences, and disabling the rule regressed two
application rows. Explicitly sharing heavily reused wide constants across the
CFG then removed nearly every repeated literal sequence and shrank the hot
body by 14.69%, but remained runtime-neutral. Sharing-aware placement still
matters generally, but this body needs the placement win coupled with call and
tail merging rather than as an isolated extraction change.

Marking indirect CTFE constant loads as generally pure was also tested and
rejected. The safe prototype eagerly loaded only scalar and scalar-pair
constants already accepted by cg_clif's direct CLIF type helpers, with
`readonly`, `notrap`, and `can_move`; other ABI shapes remained by-reference.
This qualification matters: broader representation-based versions found both
missing CLIF type cases during the stage-2 build and a direct-value versus pair
ABI mismatch while building ty. The narrowed version passed the stage-2
compiler build, removed dead literal-pool loads from the hot hashbrown probe,
and cut that body from 700 to 668 bytes. Ruff shrank by 579,360 bytes and ty by
262,496 bytes, although uv grew by 20,832 bytes. The complete 20-run screen
still moved every workload backward: uv environment creation +0.79%, uv
resolution +2.03%, Ruff +1.85%, and ty +0.72%, with no sign-test below 0.26.
Purity therefore needs a placement or dead-use criterion; exposing every
eligible immutable load to the egraph is not profitable by itself.

### 4. Reduce call, register, and stack overhead

The inlining result proves that call boundaries matter, but more indiscriminate
inlining quickly becomes counterproductive. Continue with targeted policies:

- incorporate call-site frequency and callee size into MIR defaults;
- import small post-monomorphization bodies across codegen units instead of
  repeatedly raising the local hinted-call threshold;
- reduce and fix the broader cg_clif inliner miscompile before admitting
  callees with stack slots, global values, nested calls, or larger control-flow
  graphs beyond the guarded cold-fallback subset;
- investigate the open work to let
  [regalloc manage callee-saved registers](https://github.com/bytecodealliance/wasmtime/issues/7727);
- remove [unused stack slots](https://github.com/bytecodealliance/wasmtime/issues/6661)
  and [dead stores](https://github.com/bytecodealliance/wasmtime/issues/4167);
- reduce redundant cleanup metadata now that personalities and LSDA are
  limited to functions with machine exception handlers; and
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
roadmap items. Forwarding same-width, differently typed stores and loads with
native-endian bitcasts removed four more vector materializations and cut the
hot hashbrown frame by 64 bytes, but the complete 50-run gate was neutral.
Extending the same analysis to nonescaping direct accesses then removes the
address-taken group round-trip, cuts the frame from 208 to 96 bytes, and
improves ty by 1.43% without moving the other workloads. Further stack work in
this loop should keep a still larger producer-consumer region in registers or
reduce pressure across the indirect equality call rather than add another
local transmute or rematerialization policy. Carrying a dominating scalar
store through small acyclic joins also reduced Salsa's local min/max stack
traffic, but the four-operation screen rejected it; that syntactic extension
is not a substitute for removing a sampled loop-carried spill. A matched
profile before the repeated-call import showed
hashbrown's `find_or_find_insert_index_inner` and cross-CGU
`FxHasher::write_isize` as prominent Cranelift-only leaves while LLVM had
absorbed them into callers.
Weakly coalescing the partitioner's CGU-local copies removes the duplicate
fallback bodies that remain when Cranelift cannot absorb them: it cuts
13.6-33.8% from the three binaries and improves all four application medians.
This does not remove the surviving call boundary, devirtualize the equality
closure, or reduce the active probe's register pressure. Those remain the
next runtime arc; the linker result should not be used as a reason to broaden
inlining without a call-site profitability signal.
That optimization is now ELF-only while unwinding is enabled. Mach-O coalesces
the weak text atoms but currently leaves each copy's FDE and LSDA behind, so a
correct macOS implementation needs the unwind atoms to follow the winning weak
definition before the size win can be restored there. Merely re-enabling weak
linkage would reintroduce phase-one panic failures.
Known-receiver devirtualization removed some copies of the hashbrown loop and
replaced its indirect equality call in a reduction, but both the direct-only
and targeted-inlining variants were runtime-neutral in 50-run application
gates and were rejected.
Direct construction into nested indirect-return fields completes another
profiled stack arc: it removes three 488-496-byte copies and the complete
1,760-byte frame from `ConstraintSetBuilder::new`, shrinks all three binaries,
improves ty decisively, leaves both uv operations favorable or neutral, and is
neutral for Ruff. The retained proof is deliberately limited to
single-definition, single-use memory-backed temporaries whose address is never
observed. Extending destination reuse to tuples, enum variants, or non-return
roots requires a separately proven alias and lifetime model rather than
broadening this rule by shape alone. A later exact-storage alias for a
one-field aggregate moved into an indirect call safely removed Salsa's 12-byte
input copy but was neutral in all four application rows. That negative result
narrows the next call-boundary arc to transformations that also remove
surrounding control flow or the call itself, rather than another isolated
aggregate copy.
Using MIR storage lifetimes for physical stack-slot reuse then addresses the
broader frame problem without merging Cranelift alias identities. It cuts the
profiled type-relation frame by 43%, improves Ruff decisively, and leaves both
uv resolution and ty neutral. Further frame work should preserve that
post-optimization allocation boundary rather than sharing logical slots in the
frontend, which was a measured regression.
Read-only provenance alone did not let Cranelift reuse the repeated constant
loads left in that constructor. Replacing immutable 16-128-byte copies with
AArch64 vectors made the constructor substantially smaller but trended
backward for Ruff; an exact 32-byte rule then moved uv environment creation
and ty backward. Both were rejected. The next aggregate arc needs to eliminate
or share the complete initialization rather than only use wider registers for
each independent copy.

An untargeted, one-level post-monomorphization body-import experiment removed
all of the profiled `FxHasher::write_isize` calls but produced a mixed runtime
screen and was rejected. Requiring four static call sites per caller retains
the hot `Type::hash` transformation, improves every application median, and
leaves one-off boundaries intact. A dynamic profile or stronger call-site cost
model remains the next call arc for nontrivial bodies, not broader body
availability by itself.
A narrower follow-up also rejected static body size as a substitute for that
profitability signal. It allowed one-off imported bodies with at most one live
non-control Cranelift instruction and composed them through one unhinted MIR
dependency with at most two blocks and four operations, targeting the hot
`BuildHasherDefault<FxHasher>::build_hasher` forwarder. A fresh stage-2 build
left the 91 matching ty definitions and branch references unchanged while
growing Ruff by 14,320 bytes and ty by 11,136 bytes. The experiment therefore
stopped before a runtime gate: the tiny source dependency did not make the
root an eligible call-free import, and admitting more unhinted dependencies
would recreate the rejected broad-availability policy.
The later dead-slot audit supplied the missing discriminator. Pre-optimizing
only stack-bearing catalogue bodies, retaining them only when all slots become
empty, and admitting only a composed unhinted body with one live instruction
finally removes all 54 direct ty calls to the target forwarder. It improves ty
by 1.41%, leaves the other application rows neutral, and shrinks all three
binaries by 1.1-3.8%. This is the bounded one-off exception; larger bodies
still require repeated calls or a stronger profile-derived cost model.
Aggregating the repeated-call signal across the whole codegen unit was also
rejected. Allowing an otherwise eligible imported body after eight direct
calls across all callers removed another 570 direct calls from ty and shrank
Ruff and ty by 32,544 and 56,416 bytes, respectively, but did not remove the
profiled `BuildHasherDefault<FxHasher>::build_hasher` boundary. In a 20-run
application screen, the candidate was neutral on ty (+0.20% paired median,
9/20 wins) while Ruff moved backward (+3.82%, 8/20 wins). This leaves the
per-caller repeated-call signal intact; a useful extension needs to price the
specific call boundary instead of treating aggregate popularity as
profitability.

The final unwinding gate made the size side of this arc urgent. Narrowing
personality and LSDA emission to functions with machine exception handlers is
now complete: it cuts 5.7-6.1% from the full profiling binaries, leaves their
machine text unchanged, and is runtime-neutral in the application gate. The
plain CIE remains on every unwinding function, while the augmented CIE and
LSDA are reserved for catching functions. Further unwind-size work should
profile redundant cleanup call sites and compact-unwind conversion rather
than risk removing the frame descriptions needed to unwind through ordinary
functions.

The remaining 1.4-2.2x LLVM gap shows that local representation fixes alone
will not close the runtime difference.

### 5. Expand native intrinsic coverage continuously

Missing intrinsics turn performance comparisons into compatibility tests and
can force slower fallback code. Add a benchmark startup smoke before every
timing matrix, and treat any unsupported intrinsic as a prerequisite fix.
Prefer native Cranelift instructions when available; use small, reviewed inline
assembly lowerings only where the backend has no equivalent yet.

The retained AArch64 `i8x8` comparison and splat lowering is the first
profile-driven native-vector step in this arc. It halves the hot hashbrown
body and improves ty by 5.79% without moving uv or Ruff. The rejected broad
64- and 128-bit version is the important guardrail: enabling native operations
piecemeal can add vector-to-lane materialization in surrounding scalarized
code and significantly regress another application. Removing four exact
vector-to-scalar stack round-trips after the narrow lowering improved local
code without moving any application. Combining it with nonescaping direct
stack-access forwarding removes the full control-group round-trip and improves
ty by another 1.43%, demonstrating that conversion removal needs enough
surrounding context. The next expansion must keep a larger producer-consumer
region vector-shaped rather than add another isolated conversion.
Rematerializing the loop-invariant `i8x8` splat likewise removed
its vector spill but exchanged it for another live scalar register and a
per-iteration `dup`; Ruff was neutral and ty trended backward. Expand coverage
by profiled vector shape and operation cluster, and require the complete
matched application gate before making any shape global.

## Reproduction Shape

On Darwin, `cargo +srs` currently defaults target codegen to LLVM. Select
Cranelift explicitly; otherwise a purported Cranelift comparison is actually
LLVM.

```bash
SRS_ROOT=/path/to/srs
RUSTC="$SRS_ROOT/rust/build/host/stage2/bin/rustc" \
RUSTDOC="$SRS_ROOT/rust/build/host/stage2/bin/rustdoc" \
SRS_TARGET_CODEGEN_BACKEND=cranelift \
SRS_ARTIFACT_CACHE=0 \
SRS_INCREMENTAL_LINKER=0 \
CARGO_INCREMENTAL=0 \
CARGO_PROFILE_PROFILING_LTO=false \
CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang \
CARGO_TARGET_DIR="$HOME/code/tmp/cranelift-runtime/ruff-cranelift" \
    cargo +srs build --profile profiling --bin ruff --bin ty
```

The explicit `RUSTC` and `RUSTDOC` are required when testing an uninstalled
worktree backend. `cargo +srs` alone selects the installed SRS snapshot; it
does not automatically use a freshly rebuilt compiler in another worktree.
Reusing only `+srs` silently produced a stale candidate during the LICM
experiment, which the ty correctness probe caught because that snapshot lacked
the retained AArch64 CRC lowering. That result was discarded and the valid
screen above was rebuilt from fresh targets with the worktree stage-2 paths.

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
