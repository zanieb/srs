.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_extra
_incremental_added_archive_extra:
    stp x29, x30, [sp, #-16]!
    mov x29, sp
    bl _incremental_added_archive_extra_second
    adrp x8, L_incremental_added_archive_increment@PAGE
    ldr w8, [x8, L_incremental_added_archive_increment@PAGEOFF]
    add w0, w0, w8
    ldp x29, x30, [sp], #16
    ret

.section __TEXT,__const
.p2align 2
L_incremental_added_archive_increment:
    .long 20
