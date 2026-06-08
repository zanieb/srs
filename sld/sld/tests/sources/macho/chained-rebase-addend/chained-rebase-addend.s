//#RunEnabled:true

.section __TEXT,__text
.p2align 2
.globl _main
_main:
    adrp x8, _pointer@PAGE
    add x8, x8, _pointer@PAGEOFF
    ldr x8, [x8]

    adrp x9, _target@PAGE
    add x9, x9, _target@PAGEOFF
    add x9, x9, #8

    cmp x8, x9
    b.ne 1f

    adrp x10, _tagged_pointer@PAGE
    add x10, x10, _tagged_pointer@PAGEOFF
    ldr x10, [x10]

    adrp x11, _target@PAGE
    add x11, x11, _target@PAGEOFF
    mov x12, #0x80
    lsl x12, x12, #56
    orr x11, x11, x12

    cmp x10, x11
    b.ne 1f
    mov w0, #42
    b 2f
1:
    mov w0, #1
2:
    mov x16, #1
    svc #0x80

.section __DATA,__const
.p2align 3
.globl _pointer
_pointer:
    .quad _target + 8

.globl _tagged_pointer
_tagged_pointer:
    .quad _target + 0x8000000000000000

.p2align 3
.globl _target
_target:
    .quad 0x1111
    .quad 0x2222
