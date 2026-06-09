.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_extra
_incremental_added_archive_extra:
    .cfi_startproc
    stp x29, x30, [sp, #-32]!
    str x19, [sp, #16]
    .cfi_escape 0x0f, 0x02, 0x8f, 0x20
    .cfi_offset w30, -24
    .cfi_offset w29, -32
    .cfi_offset w19, -16
    mov x29, xzr
    bl _incremental_added_archive_extra_second
    mov w19, w0
    adrp x0, _incremental_added_archive_runtime@PAGE
    add x0, x0, _incremental_added_archive_runtime@PAGEOFF
    add x0, x0, #12
    bl _incremental_added_archive_verify_unwind
    cbz w0, L_incremental_added_archive_unwind_failed
    adrp x8, L_incremental_added_archive_increment_pointer@PAGE
    ldr x8, [x8, L_incremental_added_archive_increment_pointer@PAGEOFF]
    ldr w8, [x8]
    add w0, w19, w8
    b L_incremental_added_archive_epilogue
L_incremental_added_archive_unwind_failed:
    mov w0, wzr
L_incremental_added_archive_epilogue:
    ldr x19, [sp, #16]
    ldp x29, x30, [sp], #32
    .cfi_def_cfa sp, 0
    .cfi_restore w19
    .cfi_restore w29
    .cfi_restore w30
    ret
    .cfi_endproc

.section __TEXT,__const
.p2align 2
L_incremental_added_archive_increment:
    .long 20

.section __DATA,__const
.p2align 3
L_incremental_added_archive_increment_pointer:
    .quad L_incremental_added_archive_increment
