//#AbstractConfig:incremental-relocated-text-base
//#Object:runtime.c
//#Object:incremental-relocated-text-value.S
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
//#TestIncrementalStateContains:\t4\t2684354771\t0\t
//#TestIncrementalStateContains:\t4\t2684354756\t0\t
//#TestIncrementalStateContains:\t4\t2684354770\t0\t
//#TestIncrementalStateContains:5f696e6372656d656e74616c5f72656c6f63617465645f746578745f746172676574
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
//#Config:moved-target:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DMOVE_TARGET=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:2
//#TestIncrementalChangedRun:true
//#Config:retargeted-relocation:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedCompArgs:-DRETARGET=1
//#TestIncrementalChangedExpectPatch:false
//#TestIncrementalChangedFallbackReason:changed Mach-O text relocation target
//#TestIncrementalChangedRun:true

#include "../common/runtime.h"

extern int incremental_relocated_text_value(void);

int incremental_relocated_text_helper(void) { return 0; }

void main(void) {
    int value = incremental_relocated_text_value();
    exit_syscall(value == 42 || value == 50 ? 42 : 1);
}
