//#Config:incremental-added-archive-text
//#Object:runtime.c
//#Archive:incremental-added-archive-root.S
//#RunEnabled:true
//#DiffEnabled:false
//#SldExtraLinkArgs:--incremental-padding-percent=100
//#TestIncremental:true
//#TestIncrementalCompareFull:false
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedInput:incremental-added-archive-root.a
//#TestIncrementalChangedCompArgs:-DACTIVATE_ADDED_MEMBER=1
//#TestIncrementalChangedAddedArchiveMember:aaa_incremental_added_archive.1.changed.rcgu.o=incremental-added-archive-extra.S
//#TestIncrementalChangedAddedArchiveMember:zzz_incremental_added_archive.2.changed.rcgu.o=incremental-added-archive-extra-second.S
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:3
//#TestIncrementalChangedCompareFull:false
//#TestIncrementalChangedRun:true

#include "../common/runtime.h"

extern int incremental_added_archive_value(void);

void main(void) {
    exit_syscall(incremental_added_archive_value() == 42 ? 42 : 1);
}
