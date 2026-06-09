.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_multi_grown_archive_second
_incremental_multi_grown_archive_second:
#ifdef GROW_TEXT
    .cfi_startproc
    .cfi_escape 0x0f, 0x02, 0x8f, 0x20
#endif
    mov w0, #22
#ifdef GROW_TEXT
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
#endif
    ret
#ifdef GROW_TEXT
    .cfi_endproc
#endif
L_incremental_multi_grown_archive_second_end:

#ifdef GROW_TEXT
.private_extern _incremental_multi_grown_archive_added_private
.globl _incremental_multi_grown_archive_added_private
_incremental_multi_grown_archive_added_private:
    mov w0, #24
    ret
#endif

#ifndef GROW_TEXT
.section __LD,__compact_unwind,regular,debug
.p2align 3
    .quad _incremental_multi_grown_archive_second
    .long L_incremental_multi_grown_archive_second_end - _incremental_multi_grown_archive_second
    .long 0x02000000
    .quad 0
    .quad 0
#endif
