.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_reserve
_incremental_added_archive_reserve:
    adrp x8, L_incremental_added_archive_reserve_seed@PAGE
    ldr w0, [x8, L_incremental_added_archive_reserve_seed@PAGEOFF]
    ret

.section __TEXT,__const
.p2align 2
L_incremental_added_archive_reserve_seed:
    .long 0
