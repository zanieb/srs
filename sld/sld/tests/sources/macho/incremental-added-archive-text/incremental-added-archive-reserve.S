.section __TEXT,__text,regular,pure_instructions
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
