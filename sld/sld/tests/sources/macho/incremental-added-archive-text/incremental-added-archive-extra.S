.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_extra
_incremental_added_archive_extra:
    stp x29, x30, [sp, #-16]!
    mov x29, sp
    bl _incremental_added_archive_extra_second
    adrp x8, L_incremental_added_archive_increment_pointer@PAGE
    ldr x8, [x8, L_incremental_added_archive_increment_pointer@PAGEOFF]
    ldr w8, [x8]
    add w0, w0, w8
    ldp x29, x30, [sp], #16
    ret

.section __TEXT,__const
.p2align 2
L_incremental_added_archive_increment:
    .long 20

.section __DATA,__const
.p2align 3
L_incremental_added_archive_increment_pointer:
    .quad L_incremental_added_archive_increment
