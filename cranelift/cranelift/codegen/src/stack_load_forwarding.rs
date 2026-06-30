//! Store-to-load forwarding for explicit stack slots before legalization.

use alloc::vec::Vec;

use crate::cursor::{Cursor, CursorPosition, FuncCursor};
use crate::ir::{
    Block, Endianness, Function, Inst, InstBuilder, InstructionData, MemFlagsData, Opcode,
    StackSlot, Type, Value,
};
use crate::{FxHashMap, FxHashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct StackLoc {
    slot: StackSlot,
    offset: i32,
    ty: Type,
}

#[derive(Clone, Copy, Debug)]
enum DirectStackAccess {
    Load {
        loc: StackLoc,
        addr: Value,
    },
    Store {
        loc: StackLoc,
        addr: Value,
        value: Value,
    },
}

#[derive(Clone, Copy, Debug)]
struct PredecessorEdge {
    block: Block,
    inst: Inst,
    branch_index: usize,
    eligible: bool,
}

fn direct_stack_access(
    func: &Function,
    inst: Inst,
    stack_addrs: &FxHashMap<Value, (StackSlot, i32)>,
) -> Option<DirectStackAccess> {
    let (addr, offset, ty, value) = match func.dfg.insts[inst] {
        InstructionData::Load {
            opcode: Opcode::Load,
            arg,
            offset,
            ..
        } => {
            let result = func.dfg.first_result(inst);
            (arg, offset, func.dfg.value_type(result), None)
        }
        InstructionData::Store {
            opcode: Opcode::Store,
            args,
            offset,
            ..
        } => {
            let stored = func.dfg.resolve_aliases(args[0]);
            (args[1], offset, func.dfg.value_type(stored), Some(stored))
        }
        _ => return None,
    };
    let addr = func.dfg.resolve_aliases(addr);
    let &(slot, base_offset) = stack_addrs.get(&addr)?;
    let offset = base_offset.checked_add(offset.into())?;
    let loc = StackLoc { slot, offset, ty };
    if offset < 0 || loc.end() > i64::from(func.sized_stack_slots[slot].size) {
        return None;
    }
    Some(match value {
        Some(value) => DirectStackAccess::Store { loc, addr, value },
        None => DirectStackAccess::Load { loc, addr },
    })
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

fn forward_known_load(
    cursor: &mut FuncCursor<'_>,
    inst: Inst,
    loc: StackLoc,
    values: &mut FxHashMap<StackLoc, Value>,
    endianness: Endianness,
) {
    let result = cursor.func.dfg.first_result(inst);
    if loc.ty.bytes() == 0 {
        return;
    }
    let stored = values
        .get(&loc)
        .copied()
        .or_else(|| reinterpret_exact_load(cursor, loc, values, endianness))
        .or_else(|| assemble_integer_load(cursor, loc, values, endianness));
    if let Some(stored) = stored {
        cursor.func.dfg.clear_results(inst);
        cursor.func.dfg.change_to_alias(result, stored);
        cursor.remove_inst_and_step_back();
    } else {
        values.insert(loc, result);
    }
}

fn block_stack_stores(
    func: &Function,
    block: Block,
    stack_addrs: &FxHashMap<Value, (StackSlot, i32)>,
    address_taken: &FxHashSet<StackSlot>,
) -> FxHashMap<StackLoc, Value> {
    let mut values = FxHashMap::<StackLoc, Value>::default();
    for inst in func.layout.block_insts(block) {
        let store = match func.dfg.insts[inst] {
            InstructionData::StackStore {
                opcode: Opcode::StackStore,
                arg,
                stack_slot,
                offset,
            } if !address_taken.contains(&stack_slot) => {
                let value = func.dfg.resolve_aliases(arg);
                Some((
                    StackLoc {
                        slot: stack_slot,
                        offset: offset.into(),
                        ty: func.dfg.value_type(value),
                    },
                    value,
                ))
            }
            _ => match direct_stack_access(func, inst, stack_addrs) {
                Some(DirectStackAccess::Store { loc, value, .. })
                    if !address_taken.contains(&loc.slot) =>
                {
                    Some((loc, value))
                }
                _ => None,
            },
        };
        if let Some((loc, value)) = store {
            values.retain(|known, _| !known.overlaps(loc));
            values.insert(loc, value);
        }
    }
    values
}

/// Promote a bounded set of stack values stored by both sides of a two-way join.
fn add_two_way_join_stack_params(
    func: &mut Function,
    stack_addrs: &FxHashMap<Value, (StackSlot, i32)>,
    address_taken: &FxHashSet<StackSlot>,
) -> FxHashMap<Block, FxHashMap<StackLoc, Value>> {
    const MAX_JOIN_STACK_PARAMS: usize = 4;

    // Count every incoming edge before deciding whether a join is eligible. In particular, a
    // `br_table` or `try_call` edge cannot be ignored: adding a block parameter without adding an
    // argument to that edge would make the function invalid.
    let mut predecessors = FxHashMap::<Block, Vec<PredecessorEdge>>::default();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            let eligible = matches!(func.dfg.insts[inst].opcode(), Opcode::Jump | Opcode::Brif);
            for (branch_index, destination) in func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
                .iter()
                .enumerate()
            {
                predecessors
                    .entry(destination.block(&func.dfg.value_lists))
                    .or_default()
                    .push(PredecessorEdge {
                        block,
                        inst,
                        branch_index,
                        eligible,
                    });
            }
        }
    }

    let mut join_values = FxHashMap::default();
    for (join, edges) in predecessors {
        if edges.len() != 2
            || edges[0].block == edges[1].block
            || edges.iter().any(|edge| !edge.eligible)
            || func.layout.entry_block() == Some(join)
        {
            continue;
        }

        let first = block_stack_stores(func, edges[0].block, stack_addrs, address_taken);
        let second = block_stack_stores(func, edges[1].block, stack_addrs, address_taken);
        let mut locations = first
            .keys()
            .filter(|loc| loc.ty.is_int() && second.contains_key(loc))
            .copied()
            .collect::<Vec<_>>();
        locations.sort_unstable_by_key(|loc| (loc.slot.as_u32(), loc.offset, loc.ty.bits()));
        locations.truncate(MAX_JOIN_STACK_PARAMS);

        for loc in locations {
            let param = func.dfg.append_block_param(join, loc.ty);
            for (edge, value) in [(&edges[0], first[&loc]), (&edges[1], second[&loc])] {
                let dfg = &mut func.dfg;
                let destinations = dfg.insts[edge.inst]
                    .branch_destination_mut(&mut dfg.jump_tables, &mut dfg.exception_tables);
                destinations[edge.branch_index].append_argument(value, &mut dfg.value_lists);
            }
            join_values
                .entry(join)
                .or_insert_with(FxHashMap::default)
                .insert(loc, param);
        }
    }
    join_values
}

/// Forward loads from preceding stores to non-address-taken stack slots.
///
/// Stack-slot operations are legalized to ordinary loads and stores before alias analysis. At
/// that point, an unrelated store to another offset advances the coarse `other` memory version
/// and can hide an otherwise immediate store-to-load opportunity. Before legalization, distinct
/// explicit slots and offsets are still available directly in the instruction data.
pub(crate) fn forward_stack_loads(func: &mut Function, endianness: Endianness) {
    let mut stack_addrs = FxHashMap::default();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            if let InstructionData::StackLoad {
                opcode: Opcode::StackAddr,
                stack_slot,
                offset,
            } = func.dfg.insts[inst]
            {
                stack_addrs.insert(func.dfg.first_result(inst), (stack_slot, offset.into()));
            }
        }
    }

    let mut address_taken = FxHashSet::default();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            let access = direct_stack_access(func, inst, &stack_addrs);
            let allowed_address = access.map(|access| match access {
                DirectStackAccess::Load { addr, .. } | DirectStackAccess::Store { addr, .. } => {
                    addr
                }
            });
            if let Some(DirectStackAccess::Store { value, .. }) = access
                && let Some(&(slot, _)) = stack_addrs.get(&value)
            {
                address_taken.insert(slot);
            }
            for arg in func.dfg.inst_values(inst) {
                let arg = func.dfg.resolve_aliases(arg);
                if Some(arg) == allowed_address {
                    continue;
                }
                if let Some(&(slot, _)) = stack_addrs.get(&arg) {
                    address_taken.insert(slot);
                }
            }
            if let Some(entries) = func.dfg.user_stack_map_entries(inst) {
                address_taken.extend(entries.iter().map(|entry| entry.slot));
            }
        }
    }

    let join_values = add_two_way_join_stack_params(func, &stack_addrs, &address_taken);

    let mut unique_predecessors = FxHashMap::<Block, Block>::default();
    let mut multiple_predecessors = FxHashSet::default();
    for block in func.layout.blocks() {
        for successor in func.block_successors(block) {
            if multiple_predecessors.contains(&successor) {
                continue;
            }
            if let Some(&predecessor) = unique_predecessors.get(&successor) {
                if predecessor != block {
                    unique_predecessors.remove(&successor);
                    multiple_predecessors.insert(successor);
                }
            } else {
                unique_predecessors.insert(successor, block);
            }
        }
    }

    let mut outgoing_values = FxHashMap::<Block, FxHashMap<StackLoc, Value>>::default();
    let mut cursor = FuncCursor::new(func);
    while let Some(block) = cursor.next_block() {
        let mut values = unique_predecessors
            .get(&block)
            .and_then(|predecessor| outgoing_values.get(predecessor))
            .cloned()
            .unwrap_or_default();
        if let Some(incoming) = join_values.get(&block) {
            values.extend(incoming);
        }
        while let Some(inst) = cursor.next_inst() {
            let direct_access = direct_stack_access(cursor.func, inst, &stack_addrs);
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
                    forward_known_load(&mut cursor, inst, loc, &mut values, endianness);
                }
                _ => match direct_access {
                    Some(DirectStackAccess::Store { loc, value, .. })
                        if !address_taken.contains(&loc.slot) =>
                    {
                        if loc.ty.bytes() == 0 {
                            continue;
                        }
                        values.retain(|known, _| !known.overlaps(loc));
                        values.insert(loc, value);
                    }
                    Some(DirectStackAccess::Load { loc, .. })
                        if !address_taken.contains(&loc.slot) =>
                    {
                        forward_known_load(&mut cursor, inst, loc, &mut values, endianness);
                    }
                    _ => {}
                },
            }
        }
        outgoing_values.insert(block, values);
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
            } else if let Some(DirectStackAccess::Load { loc, .. }) =
                direct_stack_access(cursor.func, inst, &stack_addrs)
            {
                observed.insert(loc.slot);
            }
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
            } else if let Some(DirectStackAccess::Store { loc, .. }) =
                direct_stack_access(cursor.func, inst, &stack_addrs)
                && !observed.contains(&loc.slot)
            {
                cursor.remove_inst_and_step_back();
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
}

/// Reinterpret a value already covering exactly the bytes requested by a load.
fn reinterpret_exact_load(
    cursor: &mut FuncCursor<'_>,
    load: StackLoc,
    values: &FxHashMap<StackLoc, Value>,
    endianness: Endianness,
) -> Option<Value> {
    let stored = values.iter().find_map(|(&loc, &value)| {
        (loc.slot == load.slot
            && loc.offset == load.offset
            && loc.end() == load.end()
            && loc.ty != load.ty)
            .then_some(value)
    })?;
    let flags = MemFlagsData::new().with_endianness(endianness);
    Some(cursor.ins().bitcast(load.ty, flags, stored))
}

/// Assemble a wider integer load from adjacent, completely covering integer values.
///
/// This handles aggregate lowering patterns such as eight `i8` stores followed by one `i64`
/// load. Reject gaps, overlaps, partial coverage, and non-integer values.
fn assemble_integer_load(
    cursor: &mut FuncCursor<'_>,
    load: StackLoc,
    values: &FxHashMap<StackLoc, Value>,
    endianness: Endianness,
) -> Option<Value> {
    if !load.ty.is_int() || load.ty.bytes() <= 1 || load.ty.bits() > 64 {
        return None;
    }

    let mut pieces = values
        .iter()
        .filter_map(|(&loc, &value)| {
            (loc.slot == load.slot
                && loc.offset >= load.offset
                && loc.end() <= load.end()
                && loc.ty.is_int()
                && loc.ty.bytes() < load.ty.bytes())
            .then_some((loc, value))
        })
        .collect::<Vec<_>>();
    pieces.sort_unstable_by_key(|(loc, _)| (loc.offset, loc.ty.bytes()));

    let mut next_offset = i64::from(load.offset);
    for (loc, _) in &pieces {
        if i64::from(loc.offset) != next_offset {
            return None;
        }
        next_offset = loc.end();
    }
    if pieces.is_empty() || next_offset != load.end() {
        return None;
    }

    let mut assembled = None;
    for (loc, value) in pieces {
        let mut value = cursor.ins().uextend(load.ty, value);
        let byte_shift = match endianness {
            Endianness::Little => i64::from(loc.offset - load.offset),
            Endianness::Big => load.end() - loc.end(),
        };
        if byte_shift != 0 {
            value = cursor.ins().ishl_imm(value, byte_shift * 8);
        }
        assembled = Some(match assembled {
            Some(previous) => cursor.ins().bor(previous, value),
            None => value,
        });
    }
    assembled
}
