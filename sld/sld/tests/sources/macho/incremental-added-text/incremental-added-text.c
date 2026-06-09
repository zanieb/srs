//#Config:incremental-added-text
//#Object:runtime.c
//#Object:incremental-added-text-value.S
//#RunEnabled:true
//#DiffEnabled:false
//#SldExtraLinkArgs:--incremental-padding-percent=100
//#TestIncremental:true
//#TestIncrementalCompareFull:false
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedInput:incremental-added-text-value.S.o
//#TestIncrementalChangedCompArgs:-DADD_TEXT=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:1
//#TestIncrementalChangedCompareFull:false
//#TestIncrementalChangedRun:true

#include "../common/runtime.h"

extern int incremental_added_text_anchor;

void main(void) {
    exit_syscall(incremental_added_text_anchor == 42 ? 42 : 1);
}
