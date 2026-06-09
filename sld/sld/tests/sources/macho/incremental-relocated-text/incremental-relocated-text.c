//#AbstractConfig:incremental-relocated-text-base
//#Object:runtime.c
//#Object:incremental-relocated-text-value.S
//#Object:incremental-relocated-text-cross-input.S
//#DiffEnabled:false
//#TestIncremental:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedInput:incremental-relocated-text-value.S.o
//#TestIncrementalChangedSection:__text
//#Config:stable-relocations:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedImmediately:true
//#TestIncrementalChangedSectionOffset:1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedRun:true
//#TestIncrementalChangedSymbolBytes:_incremental_relocated_text_value=0x40068052
//#TestIncrementalChangedLogContains:loaded records for 1 changed input file before loading inputs
//#TestIncrementalChangedLogNotContains:metadata-only changed-input patch unavailable before loading inputs
//#TestIncrementalChangedLogNotContains:filtered-record changed-input patch unavailable before loading inputs
//#TestIncrementalChangedPreservesIndexedRecords:true
//#TestIncrementalStateContains:\t4\t2684354771\t0\t
//#TestIncrementalStateContains:\t4\t2684354756\t0\t
//#TestIncrementalStateContains:\t4\t2684354770\t0\t
//#TestIncrementalStateContains:5f696e6372656d656e74616c5f72656c6f63617465645f746578745f746172676574
//#Config:stable-relocations-repeated:stable-relocations
//#TestIncrementalChangedCompareFull:false
//#TestIncrementalChangedRestore:true
//#TestIncrementalChangedReapply:true
//#Config:changed-relocation-word:incremental-relocated-text-base
//#RunEnabled:false
//#TestIncrementalUnsignedMachOOutput:true
//#TestIncrementalChangedSectionOffset:4
//#TestIncrementalChangedExpectPatch:false
//#TestIncrementalChangedFallbackReason:changed Mach-O text relocation bytes
//#Config:moved-relocations:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DMOVE_RELOCATIONS=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:1
//#TestIncrementalChangedRun:true
//#TestIncrementalChangedPreservesIndexedRecords:true
//#Config:moved-target:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DMOVE_TARGET=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:2
//#TestIncrementalChangedRun:true
//#Config:grown-text:incremental-relocated-text-base
//#RunEnabled:true
//#SldExtraLinkArgs:--incremental-padding-percent=100
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalCompareFull:false
//#TestIncrementalChangedCompArgs:-DGROW_TEXT=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:1
//#TestIncrementalChangedCompareFull:false
//#TestIncrementalChangedRestore:true
//#TestIncrementalChangedRun:true
//#Config:retargeted-relocation:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DRETARGET=1
//#TestIncrementalChangedExpectPatch:false
//#TestIncrementalChangedFallbackReason:Mach-O symbol catalog target changed sections
//#TestIncrementalChangedRun:true
//#Config:moved-cross-input-targets:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DMOVE_CROSS_INPUT_TARGETS=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:2
//#TestIncrementalChangedRun:true
//#Config:added-relocations:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DADD_RELOCATIONS=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:1
//#TestIncrementalChangedRun:true
//#TestIncrementalChangedLogContains:metadata-only changed-input patch unavailable before loading inputs: missing patch metadata for
//#TestIncrementalChangedLogContains:loaded records for 1 changed input file before loading inputs
//#TestIncrementalChangedLogContains:filtered-record changed-input patch requires complete Mach-O resolutions before loading inputs: changed Mach-O text symbol resolutions need the complete resolution catalog
//#TestIncrementalChangedLogNotContains:filtered-record changed-input patch unavailable before loading inputs
//#TestIncrementalChangedLogNotContains:full relink: input file changed:
//#TestIncrementalChangedPreservesIndexedRecords:true
//#Config:removed-relocations:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DREMOVE_RELOCATIONS=1
//#TestIncrementalChangedExpectPatch:false
//#TestIncrementalChangedFallbackReason:removed Mach-O text relocation target may change symbol liveness
//#TestIncrementalChangedRun:true
//#Config:reordered-relocations:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DREORDER_RELOCATIONS=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:1
//#TestIncrementalChangedRun:true

#include "../common/runtime.h"

extern int incremental_relocated_text_value(void);
extern int incremental_cross_input_branch_value(void);
extern int incremental_cross_input_page_value(void);

int incremental_relocated_text_helper(void) { return 0; }

void main(void) {
    int value = incremental_relocated_text_value();
    int branch_value = incremental_cross_input_branch_value();
    int page_value = incremental_cross_input_page_value();
    exit_syscall((value == 42 || value == 50) && branch_value == 42 && page_value == 42 ? 42 : 1);
}
