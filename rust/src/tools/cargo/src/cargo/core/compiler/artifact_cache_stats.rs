use std::sync::atomic::AtomicBool;
use std::time::Duration;

use portable_atomic::{AtomicU64, Ordering};

use crate::util::{CargoResult, GlobalContext};

const INELIGIBLE_REASON_COUNT: usize = 13;
const RESTORE_PHASE_COUNT: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IneligibleReason {
    TargetNotLibrary,
    ProcMacro,
    CompileMode,
    CustomBuildScript,
    Sbom,
    UnsupportedHost,
    UnmodeledLoaderEnvironment,
    UnmodeledLoaderInputs,
    DynamicExtern,
    CompilerWrapper,
    UnmodeledRustcAction,
    CompilerIdentityUnavailable,
    KeyGenerationFailure,
}

#[derive(Clone, Copy, Debug)]
pub enum MaterializationKind {
    Hardlink,
    Copy,
    CrossDeviceCopy,
}

#[derive(Clone, Copy, Debug)]
pub enum RestorePhase {
    Lock,
    ControlValidation,
    SourceValidation,
    EntryValidation,
    FinalValidation,
    FinalIdentityValidation,
    FinalLoaderValidation,
    FinalActionValidation,
    StateWrite,
}

impl RestorePhase {
    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::Lock => "lock_us",
            Self::ControlValidation => "control_validation_us",
            Self::SourceValidation => "source_validation_us",
            Self::EntryValidation => "entry_validation_us",
            Self::FinalValidation => "final_validation_us",
            Self::FinalIdentityValidation => "final_identity_validation_us",
            Self::FinalLoaderValidation => "final_loader_validation_us",
            Self::FinalActionValidation => "final_action_validation_us",
            Self::StateWrite => "state_write_us",
        }
    }

    fn all() -> [Self; RESTORE_PHASE_COUNT] {
        [
            Self::Lock,
            Self::ControlValidation,
            Self::SourceValidation,
            Self::EntryValidation,
            Self::FinalValidation,
            Self::FinalIdentityValidation,
            Self::FinalLoaderValidation,
            Self::FinalActionValidation,
            Self::StateWrite,
        ]
    }
}

#[derive(Default)]
pub struct MaterializationTotals {
    hardlinked_files: u64,
    hardlinked_bytes: u64,
    copied_files: u64,
    copied_bytes: u64,
    cross_device_copied_files: u64,
    cross_device_copied_bytes: u64,
}

impl MaterializationTotals {
    pub fn record(&mut self, kind: MaterializationKind, bytes: u64) {
        let (files_counter, bytes_counter) = match kind {
            MaterializationKind::Hardlink => {
                (&mut self.hardlinked_files, &mut self.hardlinked_bytes)
            }
            MaterializationKind::Copy => (&mut self.copied_files, &mut self.copied_bytes),
            MaterializationKind::CrossDeviceCopy => (
                &mut self.cross_device_copied_files,
                &mut self.cross_device_copied_bytes,
            ),
        };
        *files_counter += 1;
        *bytes_counter += bytes;
    }
}

impl IneligibleReason {
    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::TargetNotLibrary => "target_not_library",
            Self::ProcMacro => "proc_macro",
            Self::CompileMode => "compile_mode",
            Self::CustomBuildScript => "custom_build_script",
            Self::Sbom => "sbom",
            Self::UnsupportedHost => "unsupported_host",
            Self::UnmodeledLoaderEnvironment => "unmodeled_loader_environment",
            Self::UnmodeledLoaderInputs => "unmodeled_loader_inputs",
            Self::DynamicExtern => "dynamic_extern",
            Self::CompilerWrapper => "compiler_wrapper",
            Self::UnmodeledRustcAction => "unmodeled_rustc_action",
            Self::CompilerIdentityUnavailable => "compiler_identity_unavailable",
            Self::KeyGenerationFailure => "key_generation_failure",
        }
    }

    fn all() -> [Self; INELIGIBLE_REASON_COUNT] {
        [
            Self::TargetNotLibrary,
            Self::ProcMacro,
            Self::CompileMode,
            Self::CustomBuildScript,
            Self::Sbom,
            Self::UnsupportedHost,
            Self::UnmodeledLoaderEnvironment,
            Self::UnmodeledLoaderInputs,
            Self::DynamicExtern,
            Self::CompilerWrapper,
            Self::UnmodeledRustcAction,
            Self::CompilerIdentityUnavailable,
            Self::KeyGenerationFailure,
        ]
    }
}

pub struct ArtifactCacheStats {
    cache_configured: AtomicBool,
    cargo_fresh: AtomicU64,
    preflight_attempted: AtomicU64,
    preflight_already_fresh: AtomicU64,
    preflight_blocked_by_dependency: AtomicU64,
    preflight_finalized: AtomicU64,
    preflight_bypassed: AtomicU64,
    preflight_elapsed_us: AtomicU64,
    eligible: AtomicU64,
    ineligible: [AtomicU64; INELIGIBLE_REASON_COUNT],
    hits: AtomicU64,
    misses: AtomicU64,
    restore_failures: AtomicU64,
    restore_elapsed_us: AtomicU64,
    restore_hit_elapsed_us: AtomicU64,
    restore_miss_elapsed_us: AtomicU64,
    restore_phase_elapsed_us: [AtomicU64; RESTORE_PHASE_COUNT],
    restored_files: AtomicU64,
    restored_bytes: AtomicU64,
    hardlinked_files: AtomicU64,
    hardlinked_bytes: AtomicU64,
    copied_files: AtomicU64,
    copied_bytes: AtomicU64,
    cross_device_copied_files: AtomicU64,
    cross_device_copied_bytes: AtomicU64,
    materialization_elapsed_us: AtomicU64,
    identity_calls: AtomicU64,
    identity_computations: AtomicU64,
    identity_reuses: AtomicU64,
    identity_files: AtomicU64,
    identity_bytes: AtomicU64,
    identity_wall_us: AtomicU64,
    identity_cpu_us: AtomicU64,
    identity_computation_wall_us: AtomicU64,
    identity_computation_cpu_us: AtomicU64,
    identity_reuse_wall_us: AtomicU64,
    identity_reuse_cpu_us: AtomicU64,
    action_hash_calls: AtomicU64,
    action_hash_failures: AtomicU64,
    action_hash_wall_us: AtomicU64,
    publication_attempts: AtomicU64,
    publication_stored: AtomicU64,
    publication_skipped: AtomicU64,
    publication_failures: AtomicU64,
    publication_files: AtomicU64,
    publication_bytes: AtomicU64,
    publication_elapsed_us: AtomicU64,
    rustc_executions: AtomicU64,
    rustc_failures: AtomicU64,
    rustc_elapsed_us: AtomicU64,
    build_script_executions: AtomicU64,
    build_script_failures: AtomicU64,
    build_script_elapsed_us: AtomicU64,
    snapshot_restore_files: AtomicU64,
    snapshot_restore_bytes: AtomicU64,
    snapshot_restore_existing_files: AtomicU64,
    snapshot_restore_existing_bytes: AtomicU64,
    snapshot_restore_failures: AtomicU64,
    snapshot_restore_elapsed_us: AtomicU64,
    snapshot_manifest_files: AtomicU64,
    snapshot_manifest_bytes: AtomicU64,
    snapshot_manifest_failures: AtomicU64,
    snapshot_manifest_elapsed_us: AtomicU64,
    primary_link_rustc_executions: AtomicU64,
    primary_link_rustc_failures: AtomicU64,
    primary_link_rustc_elapsed_us: AtomicU64,
}

impl Default for ArtifactCacheStats {
    fn default() -> Self {
        Self {
            cache_configured: AtomicBool::new(false),
            cargo_fresh: AtomicU64::new(0),
            preflight_attempted: AtomicU64::new(0),
            preflight_already_fresh: AtomicU64::new(0),
            preflight_blocked_by_dependency: AtomicU64::new(0),
            preflight_finalized: AtomicU64::new(0),
            preflight_bypassed: AtomicU64::new(0),
            preflight_elapsed_us: AtomicU64::new(0),
            eligible: AtomicU64::new(0),
            ineligible: std::array::from_fn(|_| AtomicU64::new(0)),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            restore_failures: AtomicU64::new(0),
            restore_elapsed_us: AtomicU64::new(0),
            restore_hit_elapsed_us: AtomicU64::new(0),
            restore_miss_elapsed_us: AtomicU64::new(0),
            restore_phase_elapsed_us: std::array::from_fn(|_| AtomicU64::new(0)),
            restored_files: AtomicU64::new(0),
            restored_bytes: AtomicU64::new(0),
            hardlinked_files: AtomicU64::new(0),
            hardlinked_bytes: AtomicU64::new(0),
            copied_files: AtomicU64::new(0),
            copied_bytes: AtomicU64::new(0),
            cross_device_copied_files: AtomicU64::new(0),
            cross_device_copied_bytes: AtomicU64::new(0),
            materialization_elapsed_us: AtomicU64::new(0),
            identity_calls: AtomicU64::new(0),
            identity_computations: AtomicU64::new(0),
            identity_reuses: AtomicU64::new(0),
            identity_files: AtomicU64::new(0),
            identity_bytes: AtomicU64::new(0),
            identity_wall_us: AtomicU64::new(0),
            identity_cpu_us: AtomicU64::new(0),
            identity_computation_wall_us: AtomicU64::new(0),
            identity_computation_cpu_us: AtomicU64::new(0),
            identity_reuse_wall_us: AtomicU64::new(0),
            identity_reuse_cpu_us: AtomicU64::new(0),
            action_hash_calls: AtomicU64::new(0),
            action_hash_failures: AtomicU64::new(0),
            action_hash_wall_us: AtomicU64::new(0),
            publication_attempts: AtomicU64::new(0),
            publication_stored: AtomicU64::new(0),
            publication_skipped: AtomicU64::new(0),
            publication_failures: AtomicU64::new(0),
            publication_files: AtomicU64::new(0),
            publication_bytes: AtomicU64::new(0),
            publication_elapsed_us: AtomicU64::new(0),
            rustc_executions: AtomicU64::new(0),
            rustc_failures: AtomicU64::new(0),
            rustc_elapsed_us: AtomicU64::new(0),
            build_script_executions: AtomicU64::new(0),
            build_script_failures: AtomicU64::new(0),
            build_script_elapsed_us: AtomicU64::new(0),
            snapshot_restore_files: AtomicU64::new(0),
            snapshot_restore_bytes: AtomicU64::new(0),
            snapshot_restore_existing_files: AtomicU64::new(0),
            snapshot_restore_existing_bytes: AtomicU64::new(0),
            snapshot_restore_failures: AtomicU64::new(0),
            snapshot_restore_elapsed_us: AtomicU64::new(0),
            snapshot_manifest_files: AtomicU64::new(0),
            snapshot_manifest_bytes: AtomicU64::new(0),
            snapshot_manifest_failures: AtomicU64::new(0),
            snapshot_manifest_elapsed_us: AtomicU64::new(0),
            primary_link_rustc_executions: AtomicU64::new(0),
            primary_link_rustc_failures: AtomicU64::new(0),
            primary_link_rustc_elapsed_us: AtomicU64::new(0),
        }
    }
}

impl ArtifactCacheStats {
    fn add(counter: &AtomicU64, value: u64) {
        counter.fetch_add(value, Ordering::Relaxed);
    }

    fn micros(elapsed: Duration) -> u64 {
        elapsed.as_micros().min(u128::from(u64::MAX)) as u64
    }

    pub fn configured(&self) {
        self.cache_configured.store(true, Ordering::Relaxed);
    }

    pub fn cargo_fresh(&self) {
        Self::add(&self.cargo_fresh, 1);
    }

    pub fn preflight_attempted(&self) {
        Self::add(&self.preflight_attempted, 1);
    }

    pub fn preflight_already_fresh(&self) {
        Self::add(&self.preflight_already_fresh, 1);
    }

    pub fn preflight_blocked_by_dependency(&self) {
        Self::add(&self.preflight_blocked_by_dependency, 1);
    }

    pub fn preflight_finalized(&self) {
        Self::add(&self.preflight_finalized, 1);
    }

    pub fn preflight_bypassed(&self) {
        Self::add(&self.preflight_bypassed, 1);
    }

    pub fn preflight_finished(&self, elapsed: Duration) {
        Self::add(&self.preflight_elapsed_us, Self::micros(elapsed));
    }

    pub fn ineligible(&self, reason: IneligibleReason) {
        Self::add(&self.ineligible[reason.index()], 1);
    }

    pub fn eligible(&self) {
        Self::add(&self.eligible, 1);
    }

    pub fn restore_finished(&self, hit: bool, failed: bool, elapsed: Duration) {
        Self::add(if hit { &self.hits } else { &self.misses }, 1);
        let elapsed = Self::micros(elapsed);
        Self::add(&self.restore_elapsed_us, elapsed);
        Self::add(
            if hit {
                &self.restore_hit_elapsed_us
            } else {
                &self.restore_miss_elapsed_us
            },
            elapsed,
        );
        if failed {
            Self::add(&self.restore_failures, 1);
        }
    }

    pub fn restored(&self, files: u64, bytes: u64, materialization: &MaterializationTotals) {
        Self::add(&self.restored_files, files);
        Self::add(&self.restored_bytes, bytes);
        Self::add(&self.hardlinked_files, materialization.hardlinked_files);
        Self::add(&self.hardlinked_bytes, materialization.hardlinked_bytes);
        Self::add(&self.copied_files, materialization.copied_files);
        Self::add(&self.copied_bytes, materialization.copied_bytes);
        Self::add(
            &self.cross_device_copied_files,
            materialization.cross_device_copied_files,
        );
        Self::add(
            &self.cross_device_copied_bytes,
            materialization.cross_device_copied_bytes,
        );
    }

    pub fn restore_phase(&self, phase: RestorePhase, elapsed: Duration) {
        Self::add(
            &self.restore_phase_elapsed_us[phase.index()],
            Self::micros(elapsed),
        );
    }

    pub fn materialization_finished(&self, elapsed: Duration) {
        Self::add(&self.materialization_elapsed_us, Self::micros(elapsed));
    }

    pub fn compiler_identity(
        &self,
        computed: bool,
        files: u64,
        bytes: u64,
        wall: Duration,
        cpu: Duration,
    ) {
        Self::add(&self.identity_calls, 1);
        Self::add(
            if computed {
                &self.identity_computations
            } else {
                &self.identity_reuses
            },
            1,
        );
        if computed {
            Self::add(&self.identity_files, files);
            Self::add(&self.identity_bytes, bytes);
            Self::add(&self.identity_computation_wall_us, Self::micros(wall));
            Self::add(&self.identity_computation_cpu_us, Self::micros(cpu));
        } else {
            Self::add(&self.identity_reuse_wall_us, Self::micros(wall));
            Self::add(&self.identity_reuse_cpu_us, Self::micros(cpu));
        }
        Self::add(&self.identity_wall_us, Self::micros(wall));
        Self::add(&self.identity_cpu_us, Self::micros(cpu));
    }

    pub fn action_hash(&self, elapsed: Duration, failed: bool) {
        Self::add(&self.action_hash_calls, 1);
        if failed {
            Self::add(&self.action_hash_failures, 1);
        }
        Self::add(&self.action_hash_wall_us, Self::micros(elapsed));
    }

    pub fn publication_attempt(&self) {
        Self::add(&self.publication_attempts, 1);
    }

    pub fn publication_finished(&self, result: Result<bool, ()>, elapsed: Duration) {
        Self::add(&self.publication_elapsed_us, Self::micros(elapsed));
        Self::add(
            match result {
                Ok(true) => &self.publication_stored,
                Ok(false) => &self.publication_skipped,
                Err(()) => &self.publication_failures,
            },
            1,
        );
    }

    pub fn published(&self, files: u64, bytes: u64) {
        Self::add(&self.publication_files, files);
        Self::add(&self.publication_bytes, bytes);
    }

    pub fn rustc_finished(&self, elapsed: Duration, failed: bool, primary_link_rustc: bool) {
        Self::add(&self.rustc_executions, 1);
        let elapsed = Self::micros(elapsed);
        Self::add(&self.rustc_elapsed_us, elapsed);
        if failed {
            Self::add(&self.rustc_failures, 1);
        }
        if primary_link_rustc {
            Self::add(&self.primary_link_rustc_executions, 1);
            Self::add(&self.primary_link_rustc_elapsed_us, elapsed);
            if failed {
                Self::add(&self.primary_link_rustc_failures, 1);
            }
        }
    }

    pub fn build_script_finished(&self, elapsed: Duration, failed: bool) {
        Self::add(&self.build_script_executions, 1);
        Self::add(&self.build_script_elapsed_us, Self::micros(elapsed));
        if failed {
            Self::add(&self.build_script_failures, 1);
        }
    }

    pub fn snapshot_restore_finished(
        &self,
        result: Result<(u64, u64, u64, u64), ()>,
        elapsed: Duration,
    ) {
        Self::add(&self.snapshot_restore_elapsed_us, Self::micros(elapsed));
        match result {
            Ok((files, bytes, existing_files, existing_bytes)) => {
                Self::add(&self.snapshot_restore_files, files);
                Self::add(&self.snapshot_restore_bytes, bytes);
                Self::add(&self.snapshot_restore_existing_files, existing_files);
                Self::add(&self.snapshot_restore_existing_bytes, existing_bytes);
            }
            Err(()) => Self::add(&self.snapshot_restore_failures, 1),
        }
    }

    pub fn snapshot_manifest_finished(&self, result: Result<(u64, u64), ()>, elapsed: Duration) {
        Self::add(&self.snapshot_manifest_elapsed_us, Self::micros(elapsed));
        match result {
            Ok((files, bytes)) => {
                Self::add(&self.snapshot_manifest_files, files);
                Self::add(&self.snapshot_manifest_bytes, bytes);
            }
            Err(()) => Self::add(&self.snapshot_manifest_failures, 1),
        }
    }

    pub fn report(&self, gctx: &GlobalContext) -> CargoResult<()> {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let mut reasons = serde_json::Map::new();
        let mut ineligible = 0;
        for reason in IneligibleReason::all() {
            let count = load(&self.ineligible[reason.index()]);
            ineligible += count;
            reasons.insert(reason.name().to_string(), count.into());
        }
        let mut restore_phases = serde_json::Map::new();
        for phase in RestorePhase::all() {
            restore_phases.insert(
                phase.name().to_string(),
                load(&self.restore_phase_elapsed_us[phase.index()]).into(),
            );
        }
        let summary = serde_json::json!({
            "version": 1,
            "configured": self.cache_configured.load(Ordering::Relaxed),
            "units": {
                "cargo_fresh": load(&self.cargo_fresh),
                "eligible": load(&self.eligible),
                "ineligible": ineligible,
                "ineligible_by_reason": reasons,
            },
            "preflight": {
                "attempted": load(&self.preflight_attempted),
                "already_fresh": load(&self.preflight_already_fresh),
                "blocked_by_dependency": load(&self.preflight_blocked_by_dependency),
                "finalized": load(&self.preflight_finalized),
                "bypassed": load(&self.preflight_bypassed),
                "elapsed_us": load(&self.preflight_elapsed_us),
            },
            "lookup": {
                "hits": load(&self.hits),
                "misses": load(&self.misses),
                "restore_failures": load(&self.restore_failures),
                "elapsed_us": load(&self.restore_elapsed_us),
                "hit_elapsed_us": load(&self.restore_hit_elapsed_us),
                "miss_elapsed_us": load(&self.restore_miss_elapsed_us),
                "phase_elapsed_us": restore_phases,
            },
            "restore": {
                "files": load(&self.restored_files),
                "logical_bytes": load(&self.restored_bytes),
            },
            "materialization": {
                "hardlinked_files": load(&self.hardlinked_files),
                "hardlinked_logical_bytes": load(&self.hardlinked_bytes),
                "copied_files": load(&self.copied_files),
                "copied_logical_bytes": load(&self.copied_bytes),
                "cross_device_copied_files": load(&self.cross_device_copied_files),
                "cross_device_copied_logical_bytes": load(&self.cross_device_copied_bytes),
                "elapsed_us": load(&self.materialization_elapsed_us),
            },
            "hashing": {
                "compiler_identity": {
                    "calls": load(&self.identity_calls),
                    "computations": load(&self.identity_computations),
                    "reuses": load(&self.identity_reuses),
                    "files": load(&self.identity_files),
                    "bytes": load(&self.identity_bytes),
                    "wall_us": load(&self.identity_wall_us),
                    "cpu_us": load(&self.identity_cpu_us),
                    "computation_wall_us": load(&self.identity_computation_wall_us),
                    "computation_cpu_us": load(&self.identity_computation_cpu_us),
                    "reuse_wall_us": load(&self.identity_reuse_wall_us),
                    "reuse_cpu_us": load(&self.identity_reuse_cpu_us),
                },
                "action_inputs": {
                    "calls": load(&self.action_hash_calls),
                    "failures": load(&self.action_hash_failures),
                    "wall_us": load(&self.action_hash_wall_us),
                },
            },
            "publication": {
                "attempts": load(&self.publication_attempts),
                "stored": load(&self.publication_stored),
                "skipped": load(&self.publication_skipped),
                "failures": load(&self.publication_failures),
                "files": load(&self.publication_files),
                "logical_bytes": load(&self.publication_bytes),
                "elapsed_us": load(&self.publication_elapsed_us),
            },
            "rustc": {
                "executions": load(&self.rustc_executions),
                "failures": load(&self.rustc_failures),
                "elapsed_us": load(&self.rustc_elapsed_us),
            },
            "build_script": {
                "executions": load(&self.build_script_executions),
                "failures": load(&self.build_script_failures),
                "elapsed_us": load(&self.build_script_elapsed_us),
            },
            "snapshot": {
                "restore": {
                    "copied_files": load(&self.snapshot_restore_files),
                    "copied_logical_bytes": load(&self.snapshot_restore_bytes),
                    "existing_files": load(&self.snapshot_restore_existing_files),
                    "existing_logical_bytes": load(&self.snapshot_restore_existing_bytes),
                    "failures": load(&self.snapshot_restore_failures),
                    "elapsed_us": load(&self.snapshot_restore_elapsed_us),
                },
                "manifest": {
                    "files": load(&self.snapshot_manifest_files),
                    "logical_bytes": load(&self.snapshot_manifest_bytes),
                    "failures": load(&self.snapshot_manifest_failures),
                    "elapsed_us": load(&self.snapshot_manifest_elapsed_us),
                },
            },
            "primary_link_rustc": {
                "executions": load(&self.primary_link_rustc_executions),
                "failures": load(&self.primary_link_rustc_failures),
                "elapsed_us": load(&self.primary_link_rustc_elapsed_us),
            },
        });
        writeln!(gctx.shell().err(), "srs-artifact-cache-stats={summary}")?;
        Ok(())
    }
}

pub fn thread_cpu_time() -> Duration {
    #[cfg(unix)]
    {
        let mut value = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `value` points to initialized writable storage for `clock_gettime`.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut value) } == 0 {
            return Duration::new(value.tv_sec as u64, value.tv_nsec as u32);
        }
    }
    Duration::ZERO
}
