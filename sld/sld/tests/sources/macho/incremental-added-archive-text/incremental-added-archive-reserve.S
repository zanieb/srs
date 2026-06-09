.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_runtime
_incremental_added_archive_runtime:
    .cfi_startproc
    stp x29, x30, [sp, #-16]!
    .cfi_escape 0x0f, 0x02, 0x8f, 0x10
    .cfi_offset w29, -16
    .cfi_offset w30, -8
    mov x29, sp
    bl _incremental_added_archive_value
    ldp x29, x30, [sp], #16
    .cfi_def_cfa sp, 0
    .cfi_restore w29
    .cfi_restore w30
    ret
    .cfi_endproc

.p2align 2
.globl _incremental_added_archive_reserve
_incremental_added_archive_reserve:
    adrp x8, L_incremental_added_archive_reserve_seed@PAGE
    ldr w0, [x8, L_incremental_added_archive_reserve_seed@PAGEOFF]
    adrp x8, L_incremental_added_archive_data_reserve_seed@PAGE
    ldr w8, [x8, L_incremental_added_archive_data_reserve_seed@PAGEOFF]
    add w0, w0, w8
    ret

.section __TEXT,__const
.p2align 2
L_incremental_added_archive_reserve_seed:
    .long 0

.section __DATA,__const
.p2align 3
L_incremental_added_archive_data_reserve_seed:
    .quad 0
