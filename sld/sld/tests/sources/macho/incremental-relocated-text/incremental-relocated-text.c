//#AbstractConfig:incremental-relocated-text-base
//#Object:runtime.c
//#Object:incremental-relocated-text-value.s
//#DiffEnabled:false
//#TestIncremental:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedInput:incremental-relocated-text-value.s.o
//#TestIncrementalChangedSection:__text
//#Config:stable-relocations:incremental-relocated-text-base
//#RunEnabled:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChangedImmediately:true
//#TestIncrementalChangedSectionOffset:1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedRun:true
//#TestIncrementalChangedSymbolBytes:_incremental_relocated_text_value=0x40068052
//#Config:changed-relocation-word:incremental-relocated-text-base
//#RunEnabled:false
//#TestIncrementalUnsignedMachOOutput:true
//#TestIncrementalChangedSectionOffset:4
//#TestIncrementalChangedExpectPatch:false
//#TestIncrementalChangedFallbackReason:changed Mach-O text relocation bytes

#include "../common/runtime.h"

extern int incremental_relocated_text_value(void);

int incremental_relocated_text_helper(void) { return 0; }

void main(void) {
    int value = incremental_relocated_text_value();
    exit_syscall(value == 42 || value == 50 ? 42 : 1);
}
