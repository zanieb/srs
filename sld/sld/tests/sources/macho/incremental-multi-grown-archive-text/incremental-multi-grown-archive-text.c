//#Config:incremental-multi-grown-archive-text
//#Object:runtime.c
//#Object:incremental-multi-grown-archive-unwind-reserve.S
//#Archive:incremental-multi-grown-archive-first.S,incremental-multi-grown-archive-second.S
//#RunEnabled:true
//#DiffEnabled:false
//#SldExtraLinkArgs:--incremental-padding-percent=100
//#TestIncremental:true
//#TestIncrementalCompareFull:false
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedInput:incremental-multi-grown-archive-first.a
//#TestIncrementalChangedCompArgs:-DGROW_TEXT=1
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:6
//#TestIncrementalChangedCompareFull:false
//#TestIncrementalChangedRun:true
//#TestIncrementalChangedRestore:true
//#TestIncrementalChangedRestoreCompareOriginal:false
//#TestIncrementalChangedReapply:true
//#TestIncrementalChangedSymbolBytes:_incremental_multi_grown_archive_first=0x800280520004001100040011000400110004001101000014
//#TestIncrementalChangedSymbolBytes:_incremental_multi_grown_archive_added_private=0x00038052c0035fd6
//#TestIncrementalChangedRestoreSymbolBytes:_incremental_multi_grown_archive_first=0x80028052c0035fd6
//#TestIncrementalChangedLogNotContains:changed Mach-O object grew more than one text section
//#TestIncrementalChangedLogNotContains:full relink: input file changed:

#include "../common/runtime.h"

extern int incremental_multi_grown_archive_first(void);
extern int incremental_multi_grown_archive_second(void);

void main(void) {
    int first = incremental_multi_grown_archive_first();
    int second = incremental_multi_grown_archive_second();
    int initial = first == 20 && second == 22;
    int grown = first == 24 && second == 26;
    exit_syscall(initial || grown ? 42 : 1);
}
