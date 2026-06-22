//! SSA analysis

use rustc_index::IndexVec;
use rustc_middle::mir::StatementKind::*;
use rustc_middle::mir::visit::{MutatingUseContext, PlaceContext, Visitor};

use crate::prelude::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SsaKind {
    NotSsa,
    MaybeSsa,
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct AggregateDestination {
    pub(crate) parent: Local,
    pub(crate) field: FieldIdx,
}

impl SsaKind {
    pub(crate) fn is_ssa<'tcx>(self, fx: &FunctionCx<'_, '_, 'tcx>, ty: Ty<'tcx>) -> bool {
        self == SsaKind::MaybeSsa && (fx.clif_type(ty).is_some() || fx.clif_pair_type(ty).is_some())
    }
}

pub(crate) fn analyze(fx: &FunctionCx<'_, '_, '_>) -> IndexVec<Local, SsaKind> {
    let mut flag_map =
        fx.mir.local_decls.iter().map(|_| SsaKind::MaybeSsa).collect::<IndexVec<Local, SsaKind>>();

    for bb in fx.mir.basic_blocks.iter() {
        for stmt in bb.statements.iter() {
            match &stmt.kind {
                Assign(place_and_rval) => match &place_and_rval.1 {
                    // An indirect place takes the address of the pointee, not of the pointer
                    // local that forms the base of the place.
                    Rvalue::Ref(_, _, place) | Rvalue::RawPtr(_, place)
                        if !place.as_ref().is_indirect_first_projection() =>
                    {
                        flag_map[place.local] = SsaKind::NotSsa;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    flag_map
}

/// Find memory-backed locals whose only use moves them into a struct field on the path to the
/// return place. Reusing the final destination for these locals avoids large chains of aggregate
/// copies that LLVM's SROA normally removes after MIR optimization.
pub(crate) fn aggregate_destinations(
    fx: &FunctionCx<'_, '_, '_>,
) -> IndexVec<Local, Option<AggregateDestination>> {
    #[derive(Copy, Clone, Default)]
    struct LocalUse {
        definitions: usize,
        uses: usize,
        address_observed: bool,
    }

    struct UseVisitor {
        locals: IndexVec<Local, LocalUse>,
    }

    impl<'tcx> Visitor<'tcx> for UseVisitor {
        fn visit_place(&mut self, place: &Place<'tcx>, context: PlaceContext, _location: Location) {
            if matches!(context, PlaceContext::NonUse(_)) {
                return;
            }

            let usage = &mut self.locals[place.local];
            usage.address_observed |= context.may_observe_address();
            if place.projection.is_empty()
                && matches!(
                    context,
                    PlaceContext::MutatingUse(MutatingUseContext::Call | MutatingUseContext::Store)
                )
            {
                usage.definitions += 1;
            } else {
                usage.uses += 1;
            }
        }
    }

    let mut visitor =
        UseVisitor { locals: IndexVec::from_elem_n(LocalUse::default(), fx.mir.local_decls.len()) };
    visitor.visit_body(fx.mir);

    let mut destinations = IndexVec::from_elem_n(None, fx.mir.local_decls.len());
    for block in fx.mir.basic_blocks.iter() {
        for statement in &block.statements {
            let Assign(place_and_rvalue) = &statement.kind else { continue };
            let Some(parent) = place_and_rvalue.0.as_local() else { continue };
            let Rvalue::Aggregate(kind, operands) = &place_and_rvalue.1 else { continue };
            let mir::AggregateKind::Adt(def_id, variant, _, _, active_field) = **kind else {
                continue;
            };
            if active_field.is_some()
                || variant != FIRST_VARIANT
                || !fx.tcx.adt_def(def_id).is_struct()
                || visitor.locals[parent].address_observed
            {
                continue;
            }

            let parent_ty = fx.monomorphize(fx.mir.local_decls[parent].ty);
            let parent_layout = fx.layout_of(parent_ty);
            for (field, operand) in operands.iter_enumerated() {
                let (Operand::Copy(source_place) | Operand::Move(source_place)) = operand else {
                    continue;
                };
                let Some(source) = source_place.as_local() else { continue };
                let usage = visitor.locals[source];
                if source == parent
                    || fx.mir.local_kind(source) != LocalKind::Temp
                    || usage.definitions != 1
                    || usage.uses != 1
                    || usage.address_observed
                {
                    continue;
                }

                let source_ty = fx.monomorphize(fx.mir.local_decls[source].ty);
                let field_layout = parent_layout.field(&*fx, field.index());
                if source_ty == field_layout.ty
                    && field_layout.size != Size::ZERO
                    && fx.clif_type(source_ty).is_none()
                    && fx.clif_pair_type(source_ty).is_none()
                {
                    destinations[source] = Some(AggregateDestination { parent, field });
                }
            }
        }
    }

    destinations
}
