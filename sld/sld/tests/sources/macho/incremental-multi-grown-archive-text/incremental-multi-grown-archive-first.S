.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_multi_grown_archive_first
_incremental_multi_grown_archive_first:
    mov w0, #20
#ifdef GROW_TEXT
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
#endif
    ret
