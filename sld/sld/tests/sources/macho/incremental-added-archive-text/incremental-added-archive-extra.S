.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_extra
_incremental_added_archive_extra:
    stp x29, x30, [sp, #-16]!
    mov x29, sp
    bl _incremental_added_archive_extra_second
    add w0, w0, #20
    ldp x29, x30, [sp], #16
    ret
