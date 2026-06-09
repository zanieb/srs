.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_value
_incremental_added_archive_value:
#ifdef ACTIVATE_ADDED_MEMBER
    stp x29, x30, [sp, #-16]!
    mov x29, sp
    bl _incremental_added_archive_extra
    mov w8, w0
    bl _incremental_added_archive_extra_second
    add w0, w8, w0
    ldp x29, x30, [sp], #16
    ret
#else
    mov w0, #42
    ret
#endif
