use super::MatchedPatchSection;
use super::ReservedRangeRecord;
use super::Result;
use super::SectionRecord;
use super::SharedText;
use super::archive_member_patch_identifier;
use super::parse_patch_input_ref;
use std::collections::BTreeSet;
use std::fmt;
use std::ops::Range;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct MachOObjectSite {
    pub(super) input_file: SharedText,
    pub(super) input: SharedText,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum MachOObjectIdentityKind {
    Direct,
    ArchiveMember(Vec<u8>),
    Exact(SharedText),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct MachOObjectIdentity {
    input_file: SharedText,
    kind: MachOObjectIdentityKind,
}

impl MachOObjectIdentity {
    fn from_site(site: &MachOObjectSite, normalize_rust_archive_inputs: bool) -> Result<Self> {
        let kind = if !normalize_rust_archive_inputs {
            MachOObjectIdentityKind::Exact(site.input.clone())
        } else {
            match parse_patch_input_ref(site.input_file.as_str(), site.input.as_str())? {
                None => MachOObjectIdentityKind::Direct,
                Some(parsed) if !parsed.identifier.is_empty() => {
                    MachOObjectIdentityKind::ArchiveMember(archive_member_patch_identifier(
                        &parsed.identifier,
                    ))
                }
                Some(_) => MachOObjectIdentityKind::Exact(site.input.clone()),
            }
        };
        Ok(Self {
            input_file: site.input_file.clone(),
            kind,
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct MachOSectionSite {
    pub(super) object: MachOObjectSite,
    pub(super) section_index: u32,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct MachOSectionOwnershipKey {
    object: MachOObjectIdentity,
    section_index: u32,
}

impl MachOSectionOwnershipKey {
    fn from_site(site: &MachOSectionSite, normalize_rust_archive_inputs: bool) -> Result<Self> {
        Ok(Self {
            object: MachOObjectIdentity::from_site(&site.object, normalize_rust_archive_inputs)?,
            section_index: site.section_index,
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct MachORelocationSite {
    pub(super) section: MachOSectionSite,
    pub(super) relocation_offset: u64,
    pub(super) kind: u32,
    pub(super) addend: i64,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct MachOSymbolSite {
    pub(super) section: MachOSectionSite,
    pub(super) section_offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct MachOOutputAllocation {
    pub(super) output_offset: u64,
    pub(super) output_size: u64,
}

impl MachOOutputAllocation {
    fn end(self) -> Option<u64> {
        self.output_offset.checked_add(self.output_size)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOOutputOwner {
    pub(super) allocation: MachOOutputAllocation,
    pub(super) previous: MachOSectionSite,
    key: MachOSectionOwnershipKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOReservedOutputOwner {
    pub(super) output_section_id: u32,
    pub(super) alignment_exponent: u8,
    pub(super) allocation: MachOOutputAllocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOUnwindTerminatorSite {
    pub(super) output_section_id: u32,
    pub(super) allocation: MachOOutputAllocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOSectionTransitionCandidate {
    pub(super) previous: MachOSectionSite,
    pub(super) current: MachOSectionSite,
    previous_key: MachOSectionOwnershipKey,
    current_key: MachOSectionOwnershipKey,
    pub(super) previous_allocation: MachOOutputAllocation,
    pub(super) current_allocation: MachOOutputAllocation,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum MachOCoverageGeneration {
    Previous,
    Current,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum MachOCoverageKind {
    SectionBytes,
    RelocationField,
    SymbolMetadata,
    UnwindMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOCoverageClaim {
    pub(super) generation: MachOCoverageGeneration,
    pub(super) object: MachOObjectSite,
    /// Absolute byte range in the containing input file, including any archive member offset.
    pub(super) range: Range<usize>,
    pub(super) kind: MachOCoverageKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOCoverageRequirement {
    pub(super) generation: MachOCoverageGeneration,
    pub(super) object: MachOObjectSite,
    /// Absolute byte range in the containing input file, including any archive member offset.
    pub(super) range: Range<usize>,
    pub(super) kind: MachOCoverageKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct MachOTransitionCoverage {
    requirements: Vec<MachOCoverageRequirement>,
    claims: Vec<MachOCoverageClaim>,
}

impl MachOTransitionCoverage {
    pub(super) fn requirements(&self) -> &[MachOCoverageRequirement] {
        &self.requirements
    }

    pub(super) fn claims(&self) -> &[MachOCoverageClaim] {
        &self.claims
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOCohortInventory {
    owners: Vec<MachOOutputOwner>,
    reserved_owners: Vec<MachOReservedOutputOwner>,
    unwind_terminators: Vec<MachOUnwindTerminatorSite>,
    candidates: Vec<MachOSectionTransitionCandidate>,
    coverage_requirements: Vec<MachOCoverageRequirement>,
    coverage_claims: Vec<MachOCoverageClaim>,
}

impl MachOCohortInventory {
    pub(super) fn owners(&self) -> &[MachOOutputOwner] {
        &self.owners
    }

    pub(super) fn candidates(&self) -> &[MachOSectionTransitionCandidate] {
        &self.candidates
    }

    pub(super) fn reserved_owners(&self) -> &[MachOReservedOutputOwner] {
        &self.reserved_owners
    }

    pub(super) fn unwind_terminators(&self) -> &[MachOUnwindTerminatorSite] {
        &self.unwind_terminators
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOSectionOwnershipTransition {
    pub(super) owner: MachOOutputOwner,
    pub(super) current: MachOSectionSite,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum MachOOwnershipDiagnostic {
    InvalidOutputOwner {
        owner: MachOOutputOwner,
    },
    OverlappingOutputOwners {
        first: MachOOutputOwner,
        second: MachOOutputOwner,
    },
    ChangedOutputAllocation {
        candidate: MachOSectionTransitionCandidate,
    },
    MissingPreviousOwner {
        candidate: MachOSectionTransitionCandidate,
    },
    AmbiguousPreviousOwner {
        candidate: MachOSectionTransitionCandidate,
        owner_count: usize,
    },
    OwnerClaimedMultipleTimes {
        owner: MachOOutputOwner,
    },
    CurrentSiteClaimedMultipleTimes {
        current: MachOSectionSite,
    },
    UnclaimedPreviousOwner {
        owner: MachOOutputOwner,
    },
    MissingCoverageDomain {
        transition_count: usize,
    },
    EmptyCoverageRequirement {
        requirement: MachOCoverageRequirement,
    },
    EmptyCoverageRange {
        claim: MachOCoverageClaim,
    },
    OverlappingCoverageRequirements {
        first: MachOCoverageRequirement,
        second: MachOCoverageRequirement,
    },
    OverlappingCoverage {
        first: MachOCoverageClaim,
        second: MachOCoverageClaim,
    },
    MissingCoverage {
        requirement: MachOCoverageRequirement,
    },
    UnexpectedCoverage {
        claim: MachOCoverageClaim,
    },
}

impl fmt::Display for MachOOwnershipDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOutputOwner { owner } => write!(
                formatter,
                "invalid Mach-O output owner {} at {:#x}+{:#x}",
                DisplaySectionSite(&owner.previous),
                owner.allocation.output_offset,
                owner.allocation.output_size,
            ),
            Self::OverlappingOutputOwners { first, second } => write!(
                formatter,
                "overlapping Mach-O output owners {} and {}",
                DisplaySectionSite(&first.previous),
                DisplaySectionSite(&second.previous),
            ),
            Self::ChangedOutputAllocation { candidate } => write!(
                formatter,
                "Mach-O section transition changed output allocation for {}",
                DisplaySectionSite(&candidate.previous),
            ),
            Self::MissingPreviousOwner { candidate } => write!(
                formatter,
                "missing previous Mach-O output owner for {}",
                DisplaySectionSite(&candidate.previous),
            ),
            Self::AmbiguousPreviousOwner {
                candidate,
                owner_count,
            } => write!(
                formatter,
                "ambiguous previous Mach-O output owner for {} ({owner_count} candidates)",
                DisplaySectionSite(&candidate.previous),
            ),
            Self::OwnerClaimedMultipleTimes { owner } => write!(
                formatter,
                "Mach-O output owner {} was claimed multiple times",
                DisplaySectionSite(&owner.previous),
            ),
            Self::CurrentSiteClaimedMultipleTimes { current } => write!(
                formatter,
                "current Mach-O section {} was claimed by multiple owners",
                DisplaySectionSite(current),
            ),
            Self::UnclaimedPreviousOwner { owner } => write!(
                formatter,
                "previous Mach-O output owner {} was not claimed",
                DisplaySectionSite(&owner.previous),
            ),
            Self::MissingCoverageDomain { transition_count } => write!(
                formatter,
                "missing Mach-O coverage domain for {transition_count} ownership transitions",
            ),
            Self::EmptyCoverageRequirement { requirement } => write!(
                formatter,
                "empty {:?} Mach-O coverage requirement for {}",
                requirement.generation,
                DisplayObjectSite(&requirement.object),
            ),
            Self::EmptyCoverageRange { claim } => write!(
                formatter,
                "empty {:?} Mach-O coverage range for {}",
                claim.generation,
                DisplayObjectSite(&claim.object),
            ),
            Self::OverlappingCoverageRequirements { first, second } => write!(
                formatter,
                "overlapping {:?} Mach-O coverage requirements in {} for {} at {:#x}..{:#x} and {} at {:#x}..{:#x}",
                first.generation,
                first.object.input_file,
                DisplayObjectSite(&first.object),
                first.range.start,
                first.range.end,
                DisplayObjectSite(&second.object),
                second.range.start,
                second.range.end,
            ),
            Self::OverlappingCoverage { first, second } => write!(
                formatter,
                "overlapping {:?} Mach-O coverage in {} for {} at {:#x}..{:#x} and {} at {:#x}..{:#x}",
                first.generation,
                first.object.input_file,
                DisplayObjectSite(&first.object),
                first.range.start,
                first.range.end,
                DisplayObjectSite(&second.object),
                second.range.start,
                second.range.end,
            ),
            Self::MissingCoverage { requirement } => write!(
                formatter,
                "missing {:?} Mach-O {:?} coverage for {} at {:#x}..{:#x}",
                requirement.generation,
                requirement.kind,
                DisplayObjectSite(&requirement.object),
                requirement.range.start,
                requirement.range.end,
            ),
            Self::UnexpectedCoverage { claim } => write!(
                formatter,
                "unexpected {:?} Mach-O {:?} coverage for {} at {:#x}..{:#x}",
                claim.generation,
                claim.kind,
                DisplayObjectSite(&claim.object),
                claim.range.start,
                claim.range.end,
            ),
        }
    }
}

struct DisplayObjectSite<'a>(&'a MachOObjectSite);

impl fmt::Display for DisplayObjectSite<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}({})", self.0.input_file, self.0.input)
    }
}

struct DisplaySectionSite<'a>(&'a MachOSectionSite);

impl fmt::Display for DisplaySectionSite<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} section {}",
            DisplayObjectSite(&self.0.object),
            self.0.section_index,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MachOOwnershipTransitionPlan {
    inventory: MachOCohortInventory,
    transitions: Vec<MachOSectionOwnershipTransition>,
    coverage: MachOTransitionCoverage,
    diagnostics: Vec<MachOOwnershipDiagnostic>,
}

impl MachOOwnershipTransitionPlan {
    pub(super) fn inventory(&self) -> &MachOCohortInventory {
        &self.inventory
    }

    pub(super) fn transitions(&self) -> Option<&[MachOSectionOwnershipTransition]> {
        self.is_complete().then_some(&self.transitions)
    }

    pub(super) fn coverage(&self) -> Option<&MachOTransitionCoverage> {
        self.is_complete().then_some(&self.coverage)
    }

    pub(super) fn diagnostics(&self) -> &[MachOOwnershipDiagnostic] {
        &self.diagnostics
    }

    pub(super) fn is_complete(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

pub(super) fn inventory_macho_ownership_cohort(
    records: &[SectionRecord],
    reserved_ranges: &[ReservedRangeRecord],
    unwind_terminators: Vec<MachOUnwindTerminatorSite>,
    matched_sections: &[(&str, &MatchedPatchSection)],
    normalize_rust_archive_inputs: bool,
    coverage_requirements: Vec<MachOCoverageRequirement>,
    coverage_claims: Vec<MachOCoverageClaim>,
) -> Result<MachOCohortInventory> {
    let owners = records
        .iter()
        .map(|record| {
            let previous = MachOSectionSite {
                object: MachOObjectSite {
                    input_file: record.input_file.clone(),
                    input: record.input.clone(),
                },
                section_index: record.section_index,
            };
            Ok(MachOOutputOwner {
                allocation: MachOOutputAllocation {
                    output_offset: record.output_offset,
                    output_size: record.size,
                },
                key: MachOSectionOwnershipKey::from_site(&previous, normalize_rust_archive_inputs)?,
                previous,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let reserved_owners = reserved_ranges
        .iter()
        .map(|range| MachOReservedOutputOwner {
            output_section_id: range.output_section_id,
            alignment_exponent: range.alignment_exponent,
            allocation: MachOOutputAllocation {
                output_offset: range.output_offset,
                output_size: range.size,
            },
        })
        .collect();
    let candidates = matched_sections
        .iter()
        .map(|(input_file, matched)| {
            let previous = MachOSectionSite {
                object: MachOObjectSite {
                    input_file: (*input_file).into(),
                    input: matched.previous.input.clone().into(),
                },
                section_index: matched.previous.section_index,
            };
            let current = MachOSectionSite {
                object: MachOObjectSite {
                    input_file: (*input_file).into(),
                    input: matched.current.input.clone().into(),
                },
                section_index: matched.current.section_index,
            };
            Ok(MachOSectionTransitionCandidate {
                previous_key: MachOSectionOwnershipKey::from_site(
                    &previous,
                    normalize_rust_archive_inputs,
                )?,
                current_key: MachOSectionOwnershipKey::from_site(
                    &current,
                    normalize_rust_archive_inputs,
                )?,
                previous,
                current,
                previous_allocation: MachOOutputAllocation {
                    output_offset: matched.previous.output_offset,
                    output_size: matched.previous.output_size,
                },
                current_allocation: MachOOutputAllocation {
                    output_offset: matched.current.output_offset,
                    output_size: matched.current.output_size,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(MachOCohortInventory {
        owners,
        reserved_owners,
        unwind_terminators,
        candidates,
        coverage_requirements,
        coverage_claims,
    })
}

pub(super) fn plan_macho_ownership_transitions(
    inventory: &MachOCohortInventory,
) -> MachOOwnershipTransitionPlan {
    let mut diagnostics = output_owner_diagnostics(&inventory.owners);
    let mut transitions = Vec::new();
    let mut claimed_owners = BTreeSet::new();
    let mut claimed_current_sites = BTreeSet::new();

    for candidate in &inventory.candidates {
        if candidate.previous_allocation != candidate.current_allocation {
            diagnostics.push(MachOOwnershipDiagnostic::ChangedOutputAllocation {
                candidate: candidate.clone(),
            });
            continue;
        }
        let matching_owners = inventory
            .owners
            .iter()
            .enumerate()
            .filter(|(_, owner)| {
                owner.key == candidate.previous_key
                    && owner.allocation == candidate.previous_allocation
            })
            .collect::<Vec<_>>();
        let [(owner_index, owner)] = matching_owners.as_slice() else {
            if matching_owners.is_empty() {
                diagnostics.push(MachOOwnershipDiagnostic::MissingPreviousOwner {
                    candidate: candidate.clone(),
                });
            } else {
                diagnostics.push(MachOOwnershipDiagnostic::AmbiguousPreviousOwner {
                    candidate: candidate.clone(),
                    owner_count: matching_owners.len(),
                });
            }
            continue;
        };
        if !claimed_owners.insert(*owner_index) {
            diagnostics.push(MachOOwnershipDiagnostic::OwnerClaimedMultipleTimes {
                owner: (*owner).clone(),
            });
            continue;
        }
        if !claimed_current_sites.insert(candidate.current_key.clone()) {
            diagnostics.push(MachOOwnershipDiagnostic::CurrentSiteClaimedMultipleTimes {
                current: candidate.current.clone(),
            });
            continue;
        }
        transitions.push(MachOSectionOwnershipTransition {
            owner: (*owner).clone(),
            current: candidate.current.clone(),
        });
    }

    for (owner_index, owner) in inventory.owners.iter().enumerate() {
        if !claimed_owners.contains(&owner_index) {
            diagnostics.push(MachOOwnershipDiagnostic::UnclaimedPreviousOwner {
                owner: owner.clone(),
            });
        }
    }
    let (coverage, coverage_diagnostics) = validate_coverage(
        &inventory.coverage_requirements,
        &inventory.coverage_claims,
        transitions.len(),
    );
    diagnostics.extend(coverage_diagnostics);

    MachOOwnershipTransitionPlan {
        inventory: inventory.clone(),
        transitions,
        coverage,
        diagnostics,
    }
}

fn output_owner_diagnostics(owners: &[MachOOutputOwner]) -> Vec<MachOOwnershipDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut valid = owners
        .iter()
        .filter_map(|owner| {
            let Some(end) = owner.allocation.end() else {
                diagnostics.push(MachOOwnershipDiagnostic::InvalidOutputOwner {
                    owner: owner.clone(),
                });
                return None;
            };
            if owner.allocation.output_size == 0 {
                diagnostics.push(MachOOwnershipDiagnostic::InvalidOutputOwner {
                    owner: owner.clone(),
                });
                return None;
            }
            Some((owner, end))
        })
        .collect::<Vec<_>>();
    valid.sort_by_key(|(owner, _)| owner.allocation.output_offset);
    for pair in valid.windows(2) {
        let [(first, first_end), (second, _)] = pair else {
            continue;
        };
        if second.allocation.output_offset < *first_end {
            diagnostics.push(MachOOwnershipDiagnostic::OverlappingOutputOwners {
                first: (*first).clone(),
                second: (*second).clone(),
            });
        }
    }
    diagnostics
}

fn validate_coverage(
    requirements: &[MachOCoverageRequirement],
    claims: &[MachOCoverageClaim],
    transition_count: usize,
) -> (MachOTransitionCoverage, Vec<MachOOwnershipDiagnostic>) {
    let mut requirements = requirements.to_vec();
    requirements.sort_by(|left, right| {
        left.generation
            .cmp(&right.generation)
            .then_with(|| left.object.input_file.cmp(&right.object.input_file))
            .then_with(|| left.range.start.cmp(&right.range.start))
            .then_with(|| left.range.end.cmp(&right.range.end))
            .then_with(|| left.object.cmp(&right.object))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    let mut claims = claims.to_vec();
    claims.sort_by(|left, right| {
        left.generation
            .cmp(&right.generation)
            .then_with(|| left.object.input_file.cmp(&right.object.input_file))
            .then_with(|| left.range.start.cmp(&right.range.start))
            .then_with(|| left.range.end.cmp(&right.range.end))
            .then_with(|| left.object.cmp(&right.object))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    let mut diagnostics = Vec::new();
    if transition_count != 0 && requirements.is_empty() {
        diagnostics.push(MachOOwnershipDiagnostic::MissingCoverageDomain { transition_count });
    }
    for requirement in &requirements {
        if requirement.range.is_empty() {
            diagnostics.push(MachOOwnershipDiagnostic::EmptyCoverageRequirement {
                requirement: requirement.clone(),
            });
        }
    }
    for claim in &claims {
        if claim.range.is_empty() {
            diagnostics.push(MachOOwnershipDiagnostic::EmptyCoverageRange {
                claim: claim.clone(),
            });
        }
    }
    for pair in requirements.windows(2) {
        let [first, second] = pair else {
            continue;
        };
        if first.generation == second.generation
            && first.object.input_file == second.object.input_file
            && second.range.start < first.range.end
        {
            diagnostics.push(MachOOwnershipDiagnostic::OverlappingCoverageRequirements {
                first: first.clone(),
                second: second.clone(),
            });
        }
    }
    for pair in claims.windows(2) {
        let [first, second] = pair else {
            continue;
        };
        if first.generation == second.generation
            && first.object.input_file == second.object.input_file
            && second.range.start < first.range.end
        {
            diagnostics.push(MachOOwnershipDiagnostic::OverlappingCoverage {
                first: first.clone(),
                second: second.clone(),
            });
        }
    }

    // Requirements and claims come from separate discovery paths. Exact matching keeps either
    // side from silently broadening the byte domain or changing the kind of proof being asserted.
    for requirement in &requirements {
        if !claims
            .iter()
            .any(|claim| coverage_requirement_matches_claim(requirement, claim))
        {
            diagnostics.push(MachOOwnershipDiagnostic::MissingCoverage {
                requirement: requirement.clone(),
            });
        }
    }
    for claim in &claims {
        if !requirements
            .iter()
            .any(|requirement| coverage_requirement_matches_claim(requirement, claim))
        {
            diagnostics.push(MachOOwnershipDiagnostic::UnexpectedCoverage {
                claim: claim.clone(),
            });
        }
    }

    (
        MachOTransitionCoverage {
            requirements,
            claims,
        },
        diagnostics,
    )
}

fn coverage_requirement_matches_claim(
    requirement: &MachOCoverageRequirement,
    claim: &MachOCoverageClaim,
) -> bool {
    requirement.generation == claim.generation
        && requirement.object == claim.object
        && requirement.range == claim.range
        && requirement.kind == claim.kind
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incremental::PatchSection;

    fn object(input_file: &str, input: &str) -> MachOObjectSite {
        MachOObjectSite {
            input_file: input_file.into(),
            input: input.into(),
        }
    }

    fn section(input_file: &str, input: &str, section_index: u32) -> MachOSectionSite {
        MachOSectionSite {
            object: object(input_file, input),
            section_index,
        }
    }

    fn owner(
        input_file: &str,
        input: &str,
        section_index: u32,
        output_offset: u64,
        output_size: u64,
    ) -> MachOOutputOwner {
        let previous = section(input_file, input, section_index);
        MachOOutputOwner {
            allocation: MachOOutputAllocation {
                output_offset,
                output_size,
            },
            key: MachOSectionOwnershipKey::from_site(&previous, false).unwrap(),
            previous,
        }
    }

    fn candidate(
        previous: MachOSectionSite,
        current: MachOSectionSite,
        output_offset: u64,
        output_size: u64,
    ) -> MachOSectionTransitionCandidate {
        let previous_key = MachOSectionOwnershipKey::from_site(&previous, false).unwrap();
        let current_key = MachOSectionOwnershipKey::from_site(&current, false).unwrap();
        MachOSectionTransitionCandidate {
            previous,
            current,
            previous_key,
            current_key,
            previous_allocation: MachOOutputAllocation {
                output_offset,
                output_size,
            },
            current_allocation: MachOOutputAllocation {
                output_offset,
                output_size,
            },
        }
    }

    fn patch_section(input: &str, section_index: u32, output_offset: u64) -> PatchSection {
        PatchSection {
            input: input.to_owned(),
            section_index,
            section_name: Some("__TEXT,__text".to_owned()),
            input_size: 8,
            output_offset,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        }
    }

    fn coverage_requirement(
        generation: MachOCoverageGeneration,
        object: MachOObjectSite,
        range: Range<usize>,
        kind: MachOCoverageKind,
    ) -> MachOCoverageRequirement {
        MachOCoverageRequirement {
            generation,
            object,
            range,
            kind,
        }
    }

    fn coverage_claim(
        generation: MachOCoverageGeneration,
        object: MachOObjectSite,
        range: Range<usize>,
        kind: MachOCoverageKind,
    ) -> MachOCoverageClaim {
        MachOCoverageClaim {
            generation,
            object,
            range,
            kind,
        }
    }

    #[test]
    fn inventory_matches_normalized_archive_ownership_without_mutating_inputs() {
        let input_file = hex::encode("lib.rlib");
        let persisted_input = hex::encode("lib.rlib\x00crate.foo.persisted.rcgu.o\x0010:20");
        let previous_input = hex::encode("lib.rlib\x00crate.foo.previous.rcgu.o\x0030:40");
        let current_input = hex::encode("lib.rlib\x00crate.foo.current.rcgu.o\x0050:60");
        let record = SectionRecord {
            input_file: input_file.as_str().into(),
            input: persisted_input.as_str().into(),
            section_index: 3,
            output_offset: 0x1000,
            size: 8,
        };
        let matched = MatchedPatchSection {
            previous: patch_section(&previous_input, 3, 0x1000),
            current: patch_section(&current_input, 7, 0x1000),
        };
        let original_record = record.clone();
        let original_current_input = matched.current.input.clone();
        let current_object = object(&input_file, &current_input);
        let coverage_requirement = coverage_requirement(
            MachOCoverageGeneration::Current,
            current_object.clone(),
            0x50..0x58,
            MachOCoverageKind::SectionBytes,
        );
        let coverage_claim = coverage_claim(
            MachOCoverageGeneration::Current,
            current_object,
            0x50..0x58,
            MachOCoverageKind::SectionBytes,
        );

        let inventory = inventory_macho_ownership_cohort(
            std::slice::from_ref(&record),
            &[],
            Vec::new(),
            &[(input_file.as_str(), &matched)],
            true,
            vec![coverage_requirement],
            vec![coverage_claim],
        )
        .unwrap();
        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(plan.is_complete(), "{:?}", plan.diagnostics());
        let transitions = plan.transitions().expect("complete plan has transitions");
        assert_eq!(transitions.len(), 1);
        assert_eq!(
            transitions[0].owner.previous,
            section(&input_file, &persisted_input, 3),
        );
        assert_eq!(
            transitions[0].current,
            section(&input_file, &current_input, 7),
        );
        assert_eq!(record, original_record);
        assert_eq!(matched.current.input, original_current_input);
        assert_eq!(plan.inventory(), &inventory);
    }

    #[test]
    fn inventory_tracks_unwind_terminator_and_reserve_ownership() {
        let reserved = ReservedRangeRecord {
            output_section_id: 7,
            alignment_exponent: 3,
            output_offset: 0x2000,
            size: 0x100,
        };
        let terminator = MachOUnwindTerminatorSite {
            output_section_id: 7,
            allocation: MachOOutputAllocation {
                output_offset: 0x2000,
                output_size: 4,
            },
        };

        let inventory = inventory_macho_ownership_cohort(
            &[],
            std::slice::from_ref(&reserved),
            vec![terminator.clone()],
            &[],
            false,
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(plan.is_complete(), "{:?}", plan.diagnostics());
        assert_eq!(
            plan.inventory().reserved_owners(),
            &[MachOReservedOutputOwner {
                output_section_id: 7,
                alignment_exponent: 3,
                allocation: MachOOutputAllocation {
                    output_offset: 0x2000,
                    output_size: 0x100,
                },
            }],
        );
        assert_eq!(plan.inventory().unwind_terminators(), &[terminator]);
        let coverage = plan
            .coverage()
            .expect("a reserve-only inventory has no transition coverage domain");
        assert!(coverage.requirements().is_empty());
        assert!(coverage.claims().is_empty());
    }

    #[test]
    fn planner_rejects_transition_without_coverage_domain() {
        let owner = owner("lib.rlib", "member.o", 1, 0x1000, 8);
        let current = section("lib.rlib", "member.o", 1);
        let inventory = MachOCohortInventory {
            owners: vec![owner.clone()],
            reserved_owners: Vec::new(),
            unwind_terminators: Vec::new(),
            candidates: vec![candidate(owner.previous.clone(), current, 0x1000, 8)],
            coverage_requirements: Vec::new(),
            coverage_claims: Vec::new(),
        };

        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(!plan.is_complete());
        assert!(plan.transitions().is_none());
        assert!(plan.coverage().is_none());
        assert!(plan.diagnostics().iter().any(|diagnostic| matches!(
            diagnostic,
            MachOOwnershipDiagnostic::MissingCoverageDomain {
                transition_count: 1
            }
        )));
    }

    #[test]
    fn planner_rejects_missing_required_coverage() {
        let owner = owner("lib.rlib", "member.o", 1, 0x1000, 8);
        let current = section("lib.rlib", "member.o", 1);
        let input = object("lib.rlib", "member.o");
        let required = coverage_requirement(
            MachOCoverageGeneration::Current,
            input,
            10..20,
            MachOCoverageKind::SectionBytes,
        );
        let inventory = MachOCohortInventory {
            owners: vec![owner.clone()],
            reserved_owners: Vec::new(),
            unwind_terminators: Vec::new(),
            candidates: vec![candidate(owner.previous.clone(), current, 0x1000, 8)],
            coverage_requirements: vec![required.clone()],
            coverage_claims: Vec::new(),
        };

        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(!plan.is_complete());
        assert!(plan.transitions().is_none());
        assert!(plan.coverage().is_none());
        assert!(plan.diagnostics().iter().any(|diagnostic| matches!(
            diagnostic,
            MachOOwnershipDiagnostic::MissingCoverage { requirement }
                if requirement == &required
        )));
    }

    #[test]
    fn planner_rejects_ambiguous_and_unclaimed_previous_owners() {
        let duplicated = owner("lib.rlib", "member.o", 1, 0x1000, 8);
        let unclaimed = owner("lib.rlib", "unused.o", 1, 0x2000, 8);
        let inventory = MachOCohortInventory {
            owners: vec![duplicated.clone(), duplicated, unclaimed],
            reserved_owners: Vec::new(),
            unwind_terminators: Vec::new(),
            candidates: vec![candidate(
                section("lib.rlib", "member.o", 1),
                section("lib.rlib", "current.o", 2),
                0x1000,
                8,
            )],
            coverage_requirements: Vec::new(),
            coverage_claims: Vec::new(),
        };

        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(!plan.is_complete());
        assert!(plan.transitions().is_none());
        assert!(plan.diagnostics().iter().any(|diagnostic| matches!(
            diagnostic,
            MachOOwnershipDiagnostic::AmbiguousPreviousOwner { owner_count: 2, .. }
        )));
        assert_eq!(
            plan.diagnostics()
                .iter()
                .filter(|diagnostic| matches!(
                    diagnostic,
                    MachOOwnershipDiagnostic::UnclaimedPreviousOwner { .. }
                ))
                .count(),
            3,
        );
    }

    #[test]
    fn planner_rejects_multiple_owners_for_one_current_site() {
        let first = owner("lib.rlib", "first.o", 1, 0x1000, 8);
        let second = owner("lib.rlib", "second.o", 1, 0x2000, 8);
        let current = section("lib.rlib", "current.o", 4);
        let inventory = MachOCohortInventory {
            owners: vec![first.clone(), second.clone()],
            reserved_owners: Vec::new(),
            unwind_terminators: Vec::new(),
            candidates: vec![
                candidate(first.previous.clone(), current.clone(), 0x1000, 8),
                candidate(second.previous.clone(), current.clone(), 0x2000, 8),
            ],
            coverage_requirements: Vec::new(),
            coverage_claims: Vec::new(),
        };

        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(!plan.is_complete());
        assert!(plan.transitions().is_none());
        assert_eq!(plan.transitions.len(), 1);
        assert!(plan.diagnostics().iter().any(|diagnostic| matches!(
            diagnostic,
            MachOOwnershipDiagnostic::CurrentSiteClaimedMultipleTimes { current: site }
                if site == &current
        )));
    }

    #[test]
    fn planner_rejects_overlapping_coverage_claims() {
        let owner = owner("lib.rlib", "member.o", 1, 0x1000, 8);
        let current = section("lib.rlib", "member.o", 1);
        let first_input = object("lib.rlib", "member.o");
        let second_input = object("lib.rlib", "other.o");
        let coverage_requirements = vec![
            coverage_requirement(
                MachOCoverageGeneration::Current,
                first_input.clone(),
                10..20,
                MachOCoverageKind::SectionBytes,
            ),
            coverage_requirement(
                MachOCoverageGeneration::Current,
                second_input.clone(),
                18..24,
                MachOCoverageKind::RelocationField,
            ),
        ];
        let inventory = MachOCohortInventory {
            owners: vec![owner.clone()],
            reserved_owners: Vec::new(),
            unwind_terminators: Vec::new(),
            candidates: vec![candidate(owner.previous.clone(), current, 0x1000, 8)],
            coverage_requirements,
            coverage_claims: vec![
                MachOCoverageClaim {
                    generation: MachOCoverageGeneration::Current,
                    object: first_input,
                    range: 10..20,
                    kind: MachOCoverageKind::SectionBytes,
                },
                MachOCoverageClaim {
                    generation: MachOCoverageGeneration::Current,
                    object: second_input,
                    range: 18..24,
                    kind: MachOCoverageKind::RelocationField,
                },
            ],
        };

        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(!plan.is_complete());
        assert!(plan.coverage().is_none());
        assert_eq!(plan.coverage.claims().len(), 2);
        let diagnostic = plan
            .diagnostics()
            .iter()
            .find(|diagnostic| {
                matches!(
                    diagnostic,
                    MachOOwnershipDiagnostic::OverlappingCoverage { .. }
                )
            })
            .expect("overlapping coverage should be diagnosed");
        assert_eq!(
            diagnostic.to_string(),
            "overlapping Current Mach-O coverage in lib.rlib for lib.rlib(member.o) at 0xa..0x14 and lib.rlib(other.o) at 0x12..0x18",
        );
    }

    #[test]
    fn planner_keeps_previous_and_current_coverage_separate() {
        let owner = owner("lib.rlib", "member.o", 1, 0x1000, 8);
        let current = section("lib.rlib", "member.o", 1);
        let input = object("lib.rlib", "member.o");
        let coverage_requirements = vec![
            coverage_requirement(
                MachOCoverageGeneration::Previous,
                input.clone(),
                10..20,
                MachOCoverageKind::SymbolMetadata,
            ),
            coverage_requirement(
                MachOCoverageGeneration::Current,
                input.clone(),
                10..20,
                MachOCoverageKind::UnwindMetadata,
            ),
        ];
        let inventory = MachOCohortInventory {
            owners: vec![owner.clone()],
            reserved_owners: Vec::new(),
            unwind_terminators: Vec::new(),
            candidates: vec![candidate(owner.previous.clone(), current, 0x1000, 8)],
            coverage_requirements,
            coverage_claims: vec![
                MachOCoverageClaim {
                    generation: MachOCoverageGeneration::Previous,
                    object: input.clone(),
                    range: 10..20,
                    kind: MachOCoverageKind::SymbolMetadata,
                },
                MachOCoverageClaim {
                    generation: MachOCoverageGeneration::Current,
                    object: input,
                    range: 10..20,
                    kind: MachOCoverageKind::UnwindMetadata,
                },
            ],
        };

        let plan = plan_macho_ownership_transitions(&inventory);

        assert!(plan.is_complete(), "{:?}", plan.diagnostics());
        assert_eq!(
            plan.coverage()
                .expect("complete plan has validated coverage")
                .claims()
                .len(),
            2,
        );
        assert_eq!(plan.inventory().owners(), std::slice::from_ref(&owner));
        assert_eq!(plan.inventory().candidates().len(), 1);
    }

    #[test]
    fn symbol_and_relocation_sites_include_object_ownership() {
        let first_section = section("lib.rlib", "first.o", 1);
        let second_section = section("lib.rlib", "second.o", 1);
        let first_symbol = MachOSymbolSite {
            section: first_section.clone(),
            section_offset: 4,
        };
        let second_symbol = MachOSymbolSite {
            section: second_section.clone(),
            section_offset: 4,
        };
        let first_relocation = MachORelocationSite {
            section: first_section,
            relocation_offset: 8,
            kind: 2,
            addend: 0,
        };
        let second_relocation = MachORelocationSite {
            section: second_section,
            relocation_offset: 8,
            kind: 2,
            addend: 0,
        };

        assert_ne!(first_symbol, second_symbol);
        assert_ne!(first_relocation, second_relocation);
    }
}
