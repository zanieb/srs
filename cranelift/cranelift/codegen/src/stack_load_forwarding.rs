//! Store-to-load forwarding for explicit stack slots before legalization.

use crate::cursor::{Cursor, CursorPosition, FuncCursor};
use crate::ir::{Function, InstructionData, Opcode, StackSlot, Type, Value};
use crate::{FxHashMap, FxHashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct StackLoc {
    slot: StackSlot,
    offset: i32,
    ty: Type,
}

impl StackLoc {
    fn end(self) -> i64 {
        i64::from(self.offset) + i64::from(self.ty.bytes())
    }

    fn overlaps(self, other: Self) -> bool {
        self.slot == other.slot
            && i64::from(self.offset) < other.end()
            && i64::from(other.offset) < self.end()
    }
}

/// Forward loads from preceding stores to non-address-taken stack slots.
///
/// Stack-slot operations are legalized to ordinary loads and stores before alias analysis. At
/// that point, an unrelated store to another offset advances the coarse `other` memory version
/// and can hide an otherwise immediate store-to-load opportunity. Before legalization, distinct
/// explicit slots and offsets are still available directly in the instruction data.
pub(crate) fn forward_stack_loads(func: &mut Function) {
    let mut address_taken = FxHashSet::default();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            if let InstructionData::StackLoad {
                opcode: Opcode::StackAddr,
                stack_slot,
                ..
            } = func.dfg.insts[inst]
            {
                address_taken.insert(stack_slot);
            }
            if let Some(entries) = func.dfg.user_stack_map_entries(inst) {
                address_taken.extend(entries.iter().map(|entry| entry.slot));
            }
        }
    }

    let mut cursor = FuncCursor::new(func);
    while cursor.next_block().is_some() {
        let mut values = FxHashMap::<StackLoc, Value>::default();
        while let Some(inst) = cursor.next_inst() {
            match cursor.func.dfg.insts[inst] {
                InstructionData::StackStore {
                    opcode: Opcode::StackStore,
                    arg,
                    stack_slot,
                    offset,
                } if !address_taken.contains(&stack_slot) => {
                    let arg = cursor.func.dfg.resolve_aliases(arg);
                    let loc = StackLoc {
                        slot: stack_slot,
                        offset: offset.into(),
                        ty: cursor.func.dfg.value_type(arg),
                    };
                    if loc.ty.bytes() == 0 {
                        continue;
                    }
                    values.retain(|known, _| !known.overlaps(loc));
                    values.insert(loc, arg);
                }
                InstructionData::StackLoad {
                    opcode: Opcode::StackLoad,
                    stack_slot,
                    offset,
                } if !address_taken.contains(&stack_slot) => {
                    let result = cursor.func.dfg.first_result(inst);
                    let loc = StackLoc {
                        slot: stack_slot,
                        offset: offset.into(),
                        ty: cursor.func.dfg.value_type(result),
                    };
                    if loc.ty.bytes() == 0 {
                        continue;
                    }
                    if let Some(&stored) = values.get(&loc) {
                        cursor.func.dfg.clear_results(inst);
                        cursor.func.dfg.change_to_alias(result, stored);
                        cursor.remove_inst_and_step_back();
                    } else {
                        values.insert(loc, result);
                    }
                }
                _ => {}
            }
        }
    }

    // Once every read from a non-address-taken slot was forwarded, its stores are unobservable.
    let mut observed = address_taken;
    for block in cursor.func.layout.blocks() {
        for inst in cursor.func.layout.block_insts(block) {
            if let InstructionData::StackLoad {
                opcode: Opcode::StackLoad,
                stack_slot,
                ..
            } = cursor.func.dfg.insts[inst]
            {
                observed.insert(stack_slot);
            }
        }
    }

    // `PrimaryMap` stack-slot indices cannot be removed without renumbering every remaining slot.
    // An unobserved, unkeyed slot can instead be made empty so it does not reserve stack-frame
    // space. Keep keyed slots intact because embedders may use their metadata externally.
    for (slot, data) in cursor.func.sized_stack_slots.iter_mut() {
        if !observed.contains(&slot) && data.key.is_none() {
            data.size = 0;
        }
    }

    cursor.set_position(CursorPosition::Nowhere);
    while cursor.next_block().is_some() {
        while let Some(inst) = cursor.next_inst() {
            if let InstructionData::StackStore {
                opcode: Opcode::StackStore,
                stack_slot,
                ..
            } = cursor.func.dfg.insts[inst]
                && !observed.contains(&stack_slot)
            {
                cursor.remove_inst_and_step_back();
            }
        }
    }
}
