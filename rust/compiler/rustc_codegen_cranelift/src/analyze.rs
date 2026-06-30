//! SSA analysis

use rustc_index::IndexVec;
use rustc_middle::mir::StatementKind::*;

use crate::prelude::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SsaKind {
    NotSsa,
    MaybeSsa,
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
