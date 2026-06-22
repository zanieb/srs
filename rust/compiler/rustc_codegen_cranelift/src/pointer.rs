//! Defines [`Pointer`] which is used to improve the quality of the generated clif ir for pointer
//! operations.

use cranelift_codegen::ir::immediates::Offset32;
use rustc_abi::Align;

use crate::prelude::*;

/// A pointer pointing either to a certain address, a certain stack slot or nothing.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Pointer {
    base: PointerBase,
    offset: Offset32,
    readonly: bool,
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum PointerBase {
    Addr(Value),
    Stack(StackSlot),
    Dangling(Align),
}

impl Pointer {
    pub(crate) fn new(addr: Value) -> Self {
        Pointer { base: PointerBase::Addr(addr), offset: Offset32::new(0), readonly: false }
    }

    /// Create a pointer whose loads access memory immutable for this function invocation.
    pub(crate) fn new_readonly(addr: Value) -> Self {
        Pointer { base: PointerBase::Addr(addr), offset: Offset32::new(0), readonly: true }
    }

    pub(crate) fn stack_slot(stack_slot: StackSlot) -> Self {
        Pointer { base: PointerBase::Stack(stack_slot), offset: Offset32::new(0), readonly: false }
    }

    pub(crate) fn dangling(align: Align) -> Self {
        Pointer { base: PointerBase::Dangling(align), offset: Offset32::new(0), readonly: false }
    }

    pub(crate) fn debug_base_and_offset(self) -> (PointerBase, Offset32) {
        (self.base, self.offset)
    }

    /// Compare represented storage locations. `readonly` is an access fact, not part of the
    /// address identity.
    pub(crate) fn has_same_location(self, other: Pointer) -> bool {
        let same_base = match (self.base, other.base) {
            (PointerBase::Addr(a), PointerBase::Addr(b)) => a == b,
            (PointerBase::Stack(a), PointerBase::Stack(b)) => a == b,
            (PointerBase::Dangling(a), PointerBase::Dangling(b)) => a == b,
            _ => false,
        };
        same_base && self.offset == other.offset
    }

    pub(crate) fn get_addr(self, fx: &mut FunctionCx<'_, '_, '_>) -> Value {
        match self.base {
            PointerBase::Addr(base_addr) => {
                let offset: i64 = self.offset.into();
                if offset == 0 { base_addr } else { fx.bcx.ins().iadd_imm(base_addr, offset) }
            }
            PointerBase::Stack(stack_slot) => {
                fx.bcx.ins().stack_addr(fx.pointer_type, stack_slot, self.offset)
            }
            PointerBase::Dangling(align) => {
                fx.bcx.ins().iconst(fx.pointer_type, i64::try_from(align.bytes()).unwrap())
            }
        }
    }

    pub(crate) fn offset(self, fx: &mut FunctionCx<'_, '_, '_>, extra_offset: Offset32) -> Self {
        self.offset_i64(fx, extra_offset.into())
    }

    pub(crate) fn offset_i64(self, fx: &mut FunctionCx<'_, '_, '_>, extra_offset: i64) -> Self {
        if let Some(new_offset) = self.offset.try_add_i64(extra_offset) {
            Pointer { base: self.base, offset: new_offset, readonly: self.readonly }
        } else {
            let base_offset: i64 = self.offset.into();
            if let Some(new_offset) = base_offset.checked_add(extra_offset) {
                let base_addr = match self.base {
                    PointerBase::Addr(addr) => addr,
                    PointerBase::Stack(stack_slot) => {
                        fx.bcx.ins().stack_addr(fx.pointer_type, stack_slot, 0)
                    }
                    PointerBase::Dangling(align) => {
                        fx.bcx.ins().iconst(fx.pointer_type, i64::try_from(align.bytes()).unwrap())
                    }
                };
                let addr = fx.bcx.ins().iadd_imm(base_addr, new_offset);
                Pointer {
                    base: PointerBase::Addr(addr),
                    offset: Offset32::new(0),
                    readonly: self.readonly,
                }
            } else {
                panic!(
                    "self.offset ({}) + extra_offset ({}) not representable in i64",
                    base_offset, extra_offset
                );
            }
        }
    }

    pub(crate) fn offset_value(self, fx: &mut FunctionCx<'_, '_, '_>, extra_offset: Value) -> Self {
        match self.base {
            PointerBase::Addr(addr) => Pointer {
                base: PointerBase::Addr(fx.bcx.ins().iadd(addr, extra_offset)),
                offset: self.offset,
                readonly: self.readonly,
            },
            PointerBase::Stack(stack_slot) => {
                let base_addr = fx.bcx.ins().stack_addr(fx.pointer_type, stack_slot, self.offset);
                Pointer {
                    base: PointerBase::Addr(fx.bcx.ins().iadd(base_addr, extra_offset)),
                    offset: Offset32::new(0),
                    readonly: self.readonly,
                }
            }
            PointerBase::Dangling(align) => {
                let addr =
                    fx.bcx.ins().iconst(fx.pointer_type, i64::try_from(align.bytes()).unwrap());
                Pointer {
                    base: PointerBase::Addr(fx.bcx.ins().iadd(addr, extra_offset)),
                    offset: self.offset,
                    readonly: self.readonly,
                }
            }
        }
    }

    pub(crate) fn load(
        self,
        fx: &mut FunctionCx<'_, '_, '_>,
        ty: Type,
        mut flags: MemFlags,
    ) -> Value {
        if self.readonly {
            flags.set_readonly();
        }
        match self.base {
            PointerBase::Addr(base_addr) => fx.bcx.ins().load(ty, flags, base_addr, self.offset),
            PointerBase::Stack(stack_slot) => fx.bcx.ins().stack_load(ty, stack_slot, self.offset),
            PointerBase::Dangling(_align) => unreachable!(),
        }
    }

    pub(crate) fn store(self, fx: &mut FunctionCx<'_, '_, '_>, value: Value, flags: MemFlags) {
        match self.base {
            PointerBase::Addr(base_addr) => {
                fx.bcx.ins().store(flags, value, base_addr, self.offset);
            }
            PointerBase::Stack(stack_slot) => {
                fx.bcx.ins().stack_store(value, stack_slot, self.offset);
            }
            PointerBase::Dangling(_align) => unreachable!(),
        }
    }

    /// Copy a constant number of non-overlapping bytes into this pointer.
    ///
    /// Keep bounded copies that involve a known stack slot in explicit stack operations. This
    /// lets Cranelift forward values through the temporary slot before `stack_addr` legalization
    /// hides the slot identity. Arbitrary-pointer copies retain the frontend's profiled threshold.
    pub(crate) fn copy_from_nonoverlapping(
        self,
        fx: &mut FunctionCx<'_, '_, '_>,
        source: Pointer,
        size: u64,
        dest_align: u8,
        source_align: u8,
        mut flags: MemFlags,
    ) {
        const MAX_STACK_COPY_REGISTERS: u64 = 16;

        if size == 0 {
            return;
        }

        let involves_stack = matches!(self.base, PointerBase::Stack(_))
            || matches!(source.base, PointerBase::Stack(_));
        let access_size = (1_u64 << size.trailing_zeros()).min(8);
        let register_count = size / access_size;

        if involves_stack && register_count <= MAX_STACK_COPY_REGISTERS {
            if u64::from(dest_align) >= access_size && u64::from(source_align) >= access_size {
                flags.set_aligned();
            }
            let ty = Type::int((access_size * 8) as u16).unwrap();
            let values: Vec<_> = (0..register_count)
                .map(|index| {
                    let offset = Offset32::new((access_size * index).try_into().unwrap());
                    (source.offset(fx, offset).load(fx, ty, flags), offset)
                })
                .collect();
            for (value, offset) in values {
                self.offset(fx, offset).store(fx, value, flags);
            }
        } else {
            let source = source.get_addr(fx);
            let dest = self.get_addr(fx);
            fx.bcx.emit_small_memory_copy(
                fx.target_config,
                dest,
                source,
                size,
                dest_align,
                source_align,
                true,
                flags,
            );
        }
    }
}
