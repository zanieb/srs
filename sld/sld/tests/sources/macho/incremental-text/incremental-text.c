//#Config:incremental-text
//#Object:runtime.c
//#Object:incremental-text-value.s
//#RunEnabled:true
//#DiffEnabled:false
//#TestIncremental:true
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedImmediately:true
//#TestIncrementalChangedInput:incremental-text-value.s.o
//#TestIncrementalChangedSection:__text
//#TestIncrementalChangedSectionOffset:1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedRun:true
//#TestIncrementalChangedSymbolBytes:_incremental_text_value=0x40068052

#include "../common/runtime.h"

extern int incremental_text_value(void);

void main(void) {
    int value = incremental_text_value();
    exit_syscall(value == 42 || value == 50 ? 42 : 1);
}
