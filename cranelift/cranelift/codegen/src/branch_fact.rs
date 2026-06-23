//! Local propagation of comparison facts established by control flow.
//!
//! This intentionally recognizes only exact unsigned comparisons and their
//! complements along single-predecessor paths. It rewrites the later branch
//! condition, rather than the comparison value, so other uses of the pure
//! comparison keep their original semantics.

use crate::cursor::{Cursor, FuncCursor};
use crate::entity::SecondaryMap;
use crate::flowgraph::ControlFlowGraph;
use crate::ir::condcodes::{CondCode, IntCC};
use crate::ir::immediates::Imm64;
use crate::ir::{
    Block, Function, InstBuilder, InstructionData, Opcode, Type, Value, ValueDef, types,
};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComparisonOperand {
    Value(Value),
    Constant(Type, u128),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Comparison {
    cond: IntCC,
    lhs: ComparisonOperand,
    rhs: ComparisonOperand,
}

impl Comparison {
    fn complement(self) -> Self {
        Self {
            cond: self.cond.complement(),
            ..self
        }
    }

    fn equivalent(self, other: Self) -> bool {
        self == other
            || (self.cond == other.cond.swap_args()
                && self.lhs == other.rhs
                && self.rhs == other.lhs)
    }

    fn proves(self, other: Self) -> Option<bool> {
        if self.equivalent(other) {
            Some(true)
        } else if self.equivalent(other.complement()) {
            Some(false)
        } else {
            None
        }
    }
}

pub(crate) fn fold_redundant_unsigned_branches(func: &mut Function, cfg: &ControlFlowGraph) {
    let mut facts = SecondaryMap::<Block, Option<Option<Comparison>>>::new();
    facts.resize(func.dfg.num_blocks());

    let mut folds = Vec::new();
    for block in &func.layout {
        let Some(fact) = incoming_fact(func, cfg, &mut facts, block) else {
            continue;
        };
        let Some(inst) = func.layout.last_inst(block) else {
            continue;
        };
        let InstructionData::Brif { arg, .. } = func.dfg.insts[inst] else {
            continue;
        };
        let Some(comparison) = comparison_from_value(func, arg) else {
            continue;
        };
        if let Some(value) = fact.proves(comparison) {
            folds.push((inst, value));
        }
    }

    let mut cursor = FuncCursor::new(func);
    for (inst, value) in folds {
        cursor.goto_inst(inst);
        let constant = cursor.ins().iconst(types::I8, i64::from(value));
        let InstructionData::Brif { arg, .. } = &mut cursor.func.dfg.insts[inst] else {
            unreachable!();
        };
        *arg = constant;
    }
}

fn incoming_fact(
    func: &Function,
    cfg: &ControlFlowGraph,
    facts: &mut SecondaryMap<Block, Option<Option<Comparison>>>,
    block: Block,
) -> Option<Comparison> {
    if let Some(fact) = facts[block] {
        return fact;
    }

    // Mark the block before following jump chains so loops terminate
    // conservatively instead of recursing indefinitely.
    facts[block] = Some(None);

    let mut predecessors = cfg.pred_iter(block);
    let predecessor = predecessors.next()?;
    if predecessors.next().is_some() {
        return None;
    }

    let fact = match func.dfg.insts[predecessor.inst] {
        InstructionData::Brif { arg, blocks, .. } => {
            let then_block = blocks[0].block(&func.dfg.value_lists);
            let else_block = blocks[1].block(&func.dfg.value_lists);
            let comparison = comparison_from_value(func, arg)?;
            match (then_block == block, else_block == block) {
                (true, false) => Some(comparison),
                (false, true) => Some(comparison.complement()),
                _ => None,
            }
        }
        InstructionData::Jump { .. } => incoming_fact(func, cfg, facts, predecessor.block),
        _ => None,
    };

    facts[block] = Some(fact);
    fact
}

fn comparison_from_value(func: &Function, value: Value) -> Option<Comparison> {
    let value = func.dfg.resolve_aliases(value);
    let ValueDef::Result(inst, 0) = func.dfg.value_def(value) else {
        return None;
    };

    let comparison = match func.dfg.insts[inst] {
        InstructionData::IntCompare { cond, args, .. } => Comparison {
            cond,
            lhs: comparison_operand(func, args[0]),
            rhs: comparison_operand(func, args[1]),
        },
        InstructionData::IntCompareImm { cond, arg, imm, .. } => Comparison {
            cond,
            lhs: comparison_operand(func, arg),
            rhs: comparison_constant(func.dfg.value_type(arg), imm),
        },
        _ => return None,
    };

    match comparison.cond {
        IntCC::UnsignedLessThan
        | IntCC::UnsignedLessThanOrEqual
        | IntCC::UnsignedGreaterThan
        | IntCC::UnsignedGreaterThanOrEqual => Some(comparison),
        _ => None,
    }
}

fn comparison_operand(func: &Function, value: Value) -> ComparisonOperand {
    let value = func.dfg.resolve_aliases(value);
    if let ValueDef::Result(inst, 0) = func.dfg.value_def(value)
        && let InstructionData::UnaryImm {
            opcode: Opcode::Iconst,
            imm,
        } = func.dfg.insts[inst]
    {
        comparison_constant(func.dfg.value_type(value), imm)
    } else {
        ComparisonOperand::Value(value)
    }
}

fn comparison_constant(ty: Type, imm: Imm64) -> ComparisonOperand {
    let bits = ty.bits();
    let mask = if bits >= 128 {
        u128::MAX
    } else {
        (1_u128 << bits) - 1
    };
    ComparisonOperand::Constant(ty, (imm.bits() as i128 as u128) & mask)
}
