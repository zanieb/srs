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
//#TestIncrementalChangedAddedArchiveMember:incremental_added_archive.1.changed.rcgu.o=incremental-added-archive-extra.S
//#TestIncrementalChangedExpectPatch:false
//#TestIncrementalChangedFallbackReason:archive members changed
//#TestIncrementalChangedCompareFull:false
//#TestIncrementalChangedRestore:true
//#TestIncrementalChangedRun:true

#include "../common/runtime.h"

extern int incremental_added_archive_value(void);

void main(void) {
    exit_syscall(incremental_added_archive_value() == 42 ? 42 : 1);
}
