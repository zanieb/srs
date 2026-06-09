//#Config:incremental-added-archive-text
//#Object:runtime.c
//#Object:incremental-added-archive-reserve.S
//#Archive:incremental-added-archive-root.S,incremental-added-archive-zz-retired.S
//#RunEnabled:true
//#DiffEnabled:false
//#SldExtraLinkArgs:--incremental-padding-percent=300 -dead_strip
//#TestIncremental:true
//#TestIncrementalCompareFull:false
//#TestIncrementalPrivateSignedMachOOutput:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedInput:incremental-added-archive-root.a
//#TestIncrementalChangedCompArgs:-DACTIVATE_ADDED_MEMBER=1
//#TestIncrementalChangedAddedArchiveMember:aaa_incremental_added_archive.1.changed.rcgu.o=incremental-added-archive-extra.S
//#TestIncrementalChangedAddedArchiveMember:zzz_incremental_added_archive.2.changed.rcgu.o=incremental-added-archive-extra-second.S
//#TestIncrementalChangedRenameAddedArchiveMembers:true
//#TestIncrementalChangedExpectPatch:true
//#TestIncrementalChangedPatchedSectionCount:5
//#TestIncrementalChangedCompareFull:false
//#TestIncrementalChangedRun:true
//#TestIncrementalChangedSymbolBytes:_incremental_added_archive_extra_second=0xc0028052
//#TestIncrementalChangedNoSym:_incremental_added_archive_retired

#include "../common/runtime.h"

extern int incremental_added_archive_value(void);
extern int incremental_added_archive_reserve(void);

void main(void) {
    exit_syscall(
        incremental_added_archive_value() == 42 &&
                incremental_added_archive_reserve() == 0
            ? 42
            : 1);
}
