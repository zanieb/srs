.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_multi_grown_archive_unwind_reserve
_incremental_multi_grown_archive_unwind_reserve:
    .cfi_startproc
    .cfi_escape 0x0f, 0x02, 0x8f, 0x20
    ret
    .cfi_endproc
