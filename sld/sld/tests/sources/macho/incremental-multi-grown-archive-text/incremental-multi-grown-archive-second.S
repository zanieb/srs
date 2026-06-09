.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_multi_grown_archive_second
_incremental_multi_grown_archive_second:
    .cfi_startproc
    mov w0, #22
#ifdef GROW_TEXT
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
    add w0, w0, #1
#endif
    ret
    .cfi_endproc

#ifdef GROW_TEXT
.private_extern _incremental_multi_grown_archive_added_private
.globl _incremental_multi_grown_archive_added_private
_incremental_multi_grown_archive_added_private:
    mov w0, #24
    ret
#endif
