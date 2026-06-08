//#RunEnabled:true

.section __TEXT,__text
.p2align 2
.globl _main
_main:
    adrp x12, _boundary_value@PAGE
    add x12, x12, _boundary_value@PAGEOFF

    adrp x8, section$start$__DATA$_BOUNDARY@PAGE
    add x8, x8, section$start$__DATA$_BOUNDARY@PAGEOFF
    adrp x9, section$end$__DATA$_BOUNDARY@PAGE
    add x9, x9, section$end$__DATA$_BOUNDARY@PAGEOFF
    sub x9, x9, x8
    mov w0, #1
    cmp x9, #8
    b.ne 1f

    adrp x10, section$start$__DATA$_MISSING@PAGE
    add x10, x10, section$start$__DATA$_MISSING@PAGEOFF
    adrp x11, section$end$__DATA$_MISSING@PAGE
    add x11, x11, section$end$__DATA$_MISSING@PAGEOFF
    mov w0, #2
    cmp x10, x11
    b.ne 1f

    adrp x10, section$start$__DATA$__mod_init_func@PAGE
    add x10, x10, section$start$__DATA$__mod_init_func@PAGEOFF
    adrp x11, section$end$__DATA$__mod_init_func@PAGE
    add x11, x11, section$end$__DATA$__mod_init_func@PAGEOFF
    mov w0, #3
    cmp x10, x11
    b.ne 1f

    adrp x10, section$start$__TEXT$_BOUNDARY@PAGE
    add x10, x10, section$start$__TEXT$_BOUNDARY@PAGEOFF
    adrp x11, section$end$__TEXT$_BOUNDARY@PAGE
    add x11, x11, section$end$__TEXT$_BOUNDARY@PAGEOFF
    mov w0, #4
    cmp x10, x11
    b.ne 1f

    mov w0, #42
1:
    mov x16, #1
    svc #0x80

.section __DATA,_BOUNDARY
.p2align 3
.globl _boundary_value
_boundary_value:
.quad 0x123456789abcdef0
