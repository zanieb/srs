.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_multi_grown_archive_first
_incremental_multi_grown_archive_first:
    .cfi_startproc
    .cfi_escape 0x0f, 0x02, 0x8f, 0x20
    mov w0, #20
#ifdef GROW_TEXT
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
    b _incremental_multi_grown_archive_added_private
#endif
    ret
    .cfi_endproc

.section __TEXT,__const
.p2align 2
L_incremental_multi_grown_archive_metadata_target:
    .long 7

.section __DATA,__const
.p2align 3
.globl _incremental_multi_grown_archive_metadata
_incremental_multi_grown_archive_metadata:
    .quad L_incremental_multi_grown_archive_metadata_target
#ifdef GROW_TEXT
    .long 24
#else
    .long 20
#endif
    .long 17
