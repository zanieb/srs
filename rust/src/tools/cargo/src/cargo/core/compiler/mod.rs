//! # Interact with the compiler
//!
//! If you consider [`ops::cargo_compile::compile`] as a `rustc` driver but on
//! Cargo side, this module is kinda the `rustc_interface` for that merits.
//! It contains all the interaction between Cargo and the rustc compiler,
//! from preparing the context for the entire build process, to scheduling
//! and executing each unit of work (e.g. running `rustc`), to managing and
//! caching the output artifact of a build.
//!
//! However, it hasn't yet exposed a clear definition of each phase or session,
//! like what rustc has done. Also, no one knows if Cargo really needs that.
//! To be pragmatic, here we list a handful of items you may want to learn:
//!
//! * [`BuildContext`] is a static context containing all information you need
//!   before a build gets started.
//! * [`BuildRunner`] is the center of the world, coordinating a running build and
//!   collecting information from it.
//! * [`custom_build`] is the home of build script executions and output parsing.
//! * [`fingerprint`] not only defines but also executes a set of rules to
//!   determine if a re-compile is needed.
//! * [`job_queue`] is where the parallelism, job scheduling, and communication
//!   machinery happen between Cargo and the compiler.
//! * [`layout`] defines and manages output artifacts of a build in the filesystem.
//! * [`unit_dependencies`] is for building a dependency graph for compilation
//!   from a result of dependency resolution.
//! * [`Unit`] contains sufficient information to build something, usually
//!   turning into a compiler invocation in a later phase.
//!
//! [`ops::cargo_compile::compile`]: crate::ops::compile

pub mod artifact;
mod artifact_cache_snapshot;
mod artifact_cache_stats;
mod build_config;
pub(crate) mod build_context;
pub(crate) mod build_runner;
mod compilation;
mod compile_kind;
mod crate_type;
mod custom_build;
pub(crate) mod fingerprint;
pub mod future_incompat;
pub(crate) mod job_queue;
pub(crate) mod layout;
mod links;
mod locking;
mod lto;
mod output_depinfo;
mod output_sbom;
pub mod rustdoc;
pub mod standard_lib;
pub mod timings;
mod unit;
pub mod unit_dependencies;
pub mod unit_graph;
mod unused_deps;

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::fs::{self, File};
use std::io::{self, BufRead, BufWriter, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Error};
use cargo_platform::{Cfg, Platform};
use cargo_util_terminal::report::{AnnotationKind, Group, Level, Renderer, Snippet};
use itertools::Itertools;
use portable_atomic::{AtomicU64, Ordering};
use regex::Regex;
use tracing::{debug, instrument, trace};

use self::artifact_cache_stats::{
    IneligibleReason, MaterializationKind, MaterializationTotals, PublicationOutcome,
    PublicationSkipReason, RestorePhase, thread_cpu_time,
};
pub use self::build_config::UserIntent;
pub use self::build_config::{
    ArtifactCacheMaterialization, BuildConfig, CompileMode, MessageFormat, PrimaryUnitRustc,
};
pub use self::build_context::BuildContext;
pub use self::build_context::DepKindSet;
pub use self::build_context::FileFlavor;
pub use self::build_context::FileType;
pub use self::build_context::RustcTargetData;
pub use self::build_context::TargetInfo;
pub use self::build_runner::{BuildRunner, Metadata, UnitHash};
pub use self::compilation::{Compilation, Doctest, UnitOutput};
pub use self::compile_kind::{CompileKind, CompileKindFallback, CompileTarget};
pub use self::crate_type::CrateType;
pub use self::custom_build::LinkArgTarget;
pub use self::custom_build::{BuildOutput, BuildScriptOutputs, BuildScripts, LibraryPath};
pub(crate) use self::fingerprint::DirtyReason;
pub use self::fingerprint::RustdocFingerprint;
pub use self::job_queue::Freshness;
use self::job_queue::{Job, JobQueue, JobState, Work};
pub(crate) use self::layout::Layout;
pub use self::lto::Lto;
use self::output_depinfo::output_depinfo;
use self::output_sbom::build_sbom;
use self::unit_graph::UnitDep;

use crate::core::compiler::future_incompat::FutureIncompatReport;
use crate::core::compiler::locking::LockKey;
use crate::core::compiler::timings::SectionTiming;
pub use crate::core::compiler::unit::Unit;
pub use crate::core::compiler::unit::UnitIndex;
pub use crate::core::compiler::unit::UnitInterner;
use crate::core::manifest::TargetSourcePath;
use crate::core::profiles::{PanicStrategy, Profile, StripInner};
use crate::core::{Feature, PackageId, Target};
use crate::diagnostics::get_key_value;
use crate::util::OnceExt;
use crate::util::errors::{CargoResult, VerboseError};
use crate::util::interning::InternedString;
use crate::util::machine_message::{self, Message};
use crate::util::{Filesystem, TryLockResult, add_path_args, internal, path_args};

use cargo_util::{ProcessBuilder, ProcessError, paths};
use cargo_util_schemas::manifest::TomlDebugInfo;
use cargo_util_schemas::manifest::TomlTrimPaths;
use cargo_util_schemas::manifest::TomlTrimPathsValue;
use cargo_util_terminal::Verbosity;
use rustfix::diagnostics::Applicability;

const RUSTDOC_CRATE_VERSION_FLAG: &str = "--crate-version";
const SLD_PRIVATE_PERSISTENT_OUTPUT_ENV: &str = "SLD_EXPERIMENT_PRIVATE_PERSISTENT_OUTPUT";
const SLD_LEGACY_UNSIGNED_PERSISTENT_OUTPUT_ENV: &str = "SLD_EXPERIMENT_UNSIGNED_PERSISTENT_OUTPUT";
const SLD_NATIVE_INCREMENTAL_ENVS: [&str; 5] = [
    "SLD_INCREMENTAL",
    "SLD_INCREMENTAL_PADDING_PERCENT",
    "SLD_RUSTC_WORK_PRODUCT_PROVENANCE",
    "SLD_RUSTC_WORK_PRODUCT_PROVENANCE_FILE",
    "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS",
];

fn sld_native_incremental_requested(build_runner: &BuildRunner<'_, '_>) -> bool {
    build_runner.bcx.gctx.cli_unstable().sld_native_incremental
        && sld_native_incremental_supported(&build_runner.bcx.host_triple())
}

fn sld_native_incremental_supported(host_triple: &str) -> bool {
    host_triple == "aarch64-apple-darwin"
}

fn warn_if_sld_native_incremental_unsupported(
    build_runner: &BuildRunner<'_, '_>,
) -> CargoResult<()> {
    if build_runner.bcx.gctx.cli_unstable().sld_native_incremental
        && build_runner.compiled.is_empty()
    {
        if !sld_native_incremental_supported(&build_runner.bcx.host_triple()) {
            build_runner.bcx.gctx.shell().warn(
                "`-Z sld-native-incremental` is ignored because it only supports \
                 aarch64-apple-darwin hosts and targets",
            )?;
        } else if build_runner
            .bcx
            .target_data
            .requested_kinds()
            .iter()
            .any(|kind| build_runner.bcx.target_data.short_name(kind) != "aarch64-apple-darwin")
        {
            build_runner.bcx.gctx.shell().warn(
                "`-Z sld-native-incremental` is only enabled for aarch64-apple-darwin targets; \
                 unsupported targets are ignored",
            )?;
        }
    }
    Ok(())
}

fn sld_native_incremental_root_output(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> bool {
    sld_native_incremental_requested(build_runner)
        && build_runner.bcx.target_data.short_name(&unit.kind) == "aarch64-apple-darwin"
        && build_runner.bcx.roots.contains(unit)
        && unit.target.is_executable()
        && unit.mode == CompileMode::Build
}

// Rustc forwards provenance to native linkers, so publish it only for pure rlib builds.
fn sld_native_incremental_rlib_producer(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> bool {
    sld_native_incremental_requested(build_runner)
        && build_runner.bcx.target_data.short_name(&unit.kind) == "aarch64-apple-darwin"
        && unit.mode == CompileMode::Build
        && !unit.requires_upstream_objects()
}

fn sld_native_incremental_root_environment(
    gctx: &crate::GlobalContext,
) -> CargoResult<[Option<OsString>; 3]> {
    let env_config = gctx.env_config()?;
    Ok([
        "SLD_INCREMENTAL_PADDING_PERCENT",
        "SLD_RUSTC_WORK_PRODUCT_PROVENANCE",
        "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS",
    ]
    .map(|variable| {
        env_config
            .get(variable)
            .cloned()
            .or_else(|| gctx.get_env_os(variable).map(OsString::from))
    }))
}

fn remove_sld_native_incremental_env(command: &mut ProcessBuilder) {
    command.env_remove(SLD_PRIVATE_PERSISTENT_OUTPUT_ENV);
    command.env_remove(SLD_LEGACY_UNSIGNED_PERSISTENT_OUTPUT_ENV);
    for variable in SLD_NATIVE_INCREMENTAL_ENVS {
        command.env_remove(variable);
    }
}

fn configure_sld_native_incremental_rlib_producer(command: &mut ProcessBuilder) {
    remove_sld_native_incremental_env(command);
    command.env("SLD_RUSTC_WORK_PRODUCT_PROVENANCE", "1");
}

fn configure_sld_native_incremental_root(
    command: &mut ProcessBuilder,
    padding_percent: Option<&OsStr>,
    rustc_work_product_provenance: Option<&OsStr>,
    stabilize_rustc_transient_inputs: Option<&OsStr>,
) {
    remove_sld_native_incremental_env(command);
    command.env(SLD_PRIVATE_PERSISTENT_OUTPUT_ENV, "1");
    command.env("SLD_INCREMENTAL", "1");
    command.env(
        "SLD_RUSTC_WORK_PRODUCT_PROVENANCE",
        rustc_work_product_provenance.unwrap_or_else(|| OsStr::new("1")),
    );
    command.env(
        "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS",
        stabilize_rustc_transient_inputs.unwrap_or_else(|| OsStr::new("1")),
    );
    if let Some(value) = padding_percent {
        command.env("SLD_INCREMENTAL_PADDING_PERCENT", value);
    }
}

fn copy_sld_native_incremental_artifact(src: &Path, dst: &Path) -> CargoResult<()> {
    if fs::symlink_metadata(dst).is_ok() {
        paths::remove_file(dst)?;
    }
    let mut retries = 10;
    loop {
        match fs::copy(src, dst) {
            Ok(_) => return Ok(()),
            Err(error)
                if cfg!(target_os = "macos")
                    && error.raw_os_error() == Some(35 /* libc::EAGAIN */)
                    && retries > 0 =>
            {
                tracing::info!("copy failed {error:?}. retrying fs::copy");
                retries -= 1;
                if fs::symlink_metadata(dst).is_ok() {
                    paths::remove_file(dst)?;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to copy persistent SLD root output `{}` to `{}`",
                        src.display(),
                        dst.display()
                    )
                });
            }
        }
    }
}

#[cfg(unix)]
fn sld_native_incremental_artifact_has_multiple_links(path: &Path) -> CargoResult<bool> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "failed to inspect persistent SLD root output `{}`",
            path.display()
        )
    })?;
    Ok(metadata.nlink() > 1)
}

#[cfg(not(unix))]
fn sld_native_incremental_artifact_has_multiple_links(_path: &Path) -> CargoResult<bool> {
    Ok(false)
}

fn sld_native_incremental_artifact_is_regular_file(path: &Path) -> CargoResult<bool> {
    Ok(fs::symlink_metadata(path)
        .with_context(|| {
            format!(
                "failed to inspect persistent SLD root output `{}`",
                path.display()
            )
        })?
        .file_type()
        .is_file())
}

fn isolate_sld_native_incremental_artifact(path: &Path) -> CargoResult<()> {
    let parent = path.parent().ok_or_else(|| {
        internal(format!(
            "persistent SLD root output `{}` has no parent directory",
            path.display()
        ))
    })?;
    let temporary = tempfile::Builder::new()
        .prefix(".cargo-sld-detach")
        .tempfile_in(parent)?;
    copy_sld_native_incremental_artifact(path, temporary.path())?;
    fs::rename(temporary.path(), path).with_context(|| {
        format!(
            "failed to isolate persistent SLD root output `{}`",
            path.display()
        )
    })?;
    Ok(())
}

const ARTIFACT_CACHE_PUBLISH_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_PUBLISH_DELAY_MS";
const ARTIFACT_CACHE_PUBLISH_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_PUBLISH_READY_FILE";
const ARTIFACT_CACHE_PUBLISH_RELEASE_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_PUBLISH_RELEASE_FILE";
const ARTIFACT_CACHE_PUBLISH_LOCKED_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_PUBLISH_LOCKED_READY_FILE";
const ARTIFACT_CACHE_PUBLISH_LOCKED_RELEASE_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_PUBLISH_LOCKED_RELEASE_FILE";
const ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS";
const ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE";
const ARTIFACT_CACHE_INPUT_DIGEST_RELEASE_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_RELEASE_FILE";
const ARTIFACT_CACHE_RESTORE_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_DELAY_MS";
const ARTIFACT_CACHE_RESTORE_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_READY_FILE";
const ARTIFACT_CACHE_RESTORE_RELEASE_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_RELEASE_FILE";
const ARTIFACT_CACHE_RESTORE_MATERIALIZED_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_MATERIALIZED_READY_FILE";
const ARTIFACT_CACHE_RESTORE_MATERIALIZED_RELEASE_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_MATERIALIZED_RELEASE_FILE";
const ARTIFACT_CACHE_RESTORE_MATERIALIZED_STALE_IDENTITY_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_MATERIALIZED_STALE_IDENTITY";
const ARTIFACT_CACHE_RESTORE_ADMITTED_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_ADMITTED_DELAY_MS";
const ARTIFACT_CACHE_RESTORE_ADMITTED_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_ADMITTED_READY_FILE";
const ARTIFACT_CACHE_RESTORE_ADMITTED_RELEASE_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_ADMITTED_RELEASE_FILE";
const ARTIFACT_CACHE_KEY_FAILURE_FOR_TESTS: &str = "__CARGO_TEST_ARTIFACT_CACHE_KEY_FAILURE";
const ARTIFACT_CACHE_FORCE_REBUILD_FOR_TESTS: &str = "__CARGO_TEST_ARTIFACT_CACHE_FORCE_REBUILD";
const ARTIFACT_CACHE_RUNTIME_COMMAND_MUTATION_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RUNTIME_COMMAND_MUTATION";
const ARTIFACT_CACHE_EXECUTOR_INIT_LOG_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_EXECUTOR_INIT_LOG";
const ARTIFACT_CACHE_STORE_FAILURE_AFTER_STAGING_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_STORE_FAILURE_AFTER_STAGING";
const ARTIFACT_CACHE_TRANSIENT_REMOVE_FAILURE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_TRANSIENT_REMOVE_FAILURE";
const ARTIFACT_CACHE_CROSS_DEVICE_HARDLINK_FAILURE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_CROSS_DEVICE_HARDLINK_FAILURE";
const ARTIFACT_CACHE_SIZE_STATE: &str = ".cargo-artifact-cache-size";
const ARTIFACT_CACHE_SIZE_STATE_VERSION: &str = "v2";
const ARTIFACT_CACHE_ACTION_PUBLICATION_LOCK_PREFIX: &str = ".cargo-artifact-cache-publish-lock";
const ARTIFACT_CACHE_ACTION_LOCK_SHARD_HEX_LEN: usize = 6;
const ARTIFACT_CACHE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
static ARTIFACT_CACHE_ACCESS_LOCK: RwLock<()> = RwLock::new(());

struct ArtifactCacheRestoreLock {
    _process_lock: RwLockReadGuard<'static, ()>,
    _filesystem_lock: crate::util::FileLock,
}

struct ArtifactCachePublicationLock {
    _process_lock: RwLockWriteGuard<'static, ()>,
    _filesystem_lock: crate::util::FileLock,
}

#[derive(Debug, PartialEq, Eq)]
struct ArtifactCacheKey {
    entry_root: PathBuf,
    loader_inputs_digest: blake3::Hash,
    action_inputs_digest: blake3::Hash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtifactCacheActionInputWitness {
    paths: Vec<ArtifactCacheActionInputPathWitness>,
    supports_fast_path: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactCacheActionInputPathKind {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtifactCacheActionInputPathWitness {
    path: PathBuf,
    kind: ArtifactCacheActionInputPathKind,
    metadata: Option<ArtifactCacheActionInputMetadataWitness>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtifactCacheActionInputMetadataWitness {
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

#[derive(Debug)]
struct PreparedArtifactCacheKey {
    key: ArtifactCacheKey,
    action_inputs_witness: ArtifactCacheActionInputWitness,
}

struct ArtifactCacheActionInputSnapshot {
    digest: blake3::Hash,
    witness: ArtifactCacheActionInputWitness,
}

#[derive(Debug)]
pub(super) struct PreparedArtifactCacheAction {
    crate_name: String,
    config: build_config::ArtifactCacheConfig,
    key: ArtifactCacheKey,
    action_inputs_witness: ArtifactCacheActionInputWitness,
    identity_witness: crate::util::rustc::ArtifactCacheIdentityWitness,
    loader_input_paths: Vec<CompilerLoaderInput>,
    command_digest: blake3::Hash,
}

pub(super) enum PreflightArtifactCacheState {
    Miss(PreparedArtifactCacheAction),
    Bypassed,
}

#[derive(Debug)]
enum ArtifactCacheActionBuilder {
    Disabled,
    Rejected {
        reason: IneligibleReason,
    },
    Candidate {
        crate_name: String,
        config: build_config::ArtifactCacheConfig,
        identity_provider: crate::util::rustc::ArtifactCacheIdentityProvider,
        compiler_program: PathBuf,
        identity_program: PathBuf,
        dependency_search_paths: Vec<OsString>,
        portable_remaps: Vec<(OsString, OsString)>,
        build_dir: PathBuf,
        output_root: PathBuf,
        cwd: PathBuf,
        rustc_verbose_version: String,
        rustc_host: String,
    },
}

pub(super) const ARTIFACT_CACHE_FRESHNESS_STAMP: &str = "artifact-cache-complete.timestamp";

pub(super) fn artifact_cache_compile_mode_is_supported(mode: CompileMode) -> bool {
    matches!(
        mode,
        CompileMode::Build | CompileMode::Check { test: false }
    )
}
pub(super) const SLD_NATIVE_INCREMENTAL_FRESHNESS_STAMP: &str =
    "sld-native-incremental-complete.timestamp";
static ARTIFACT_CACHE_PUBLICATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A glorified callback for executing calls to rustc. Rather than calling rustc
/// directly, we'll use an `Executor`, giving clients an opportunity to intercept
/// the build calls.
pub trait Executor: Send + Sync + 'static {
    /// Called after a rustc process invocation is prepared up-front for a given
    /// unit of work (may still be modified for runtime-known dependencies, when
    /// the work is actually executed).
    fn init(&self, _build_runner: &BuildRunner<'_, '_>, _unit: &Unit) {}

    /// Whether this executor preserves rustc's ordinary command semantics so
    /// a verified artifact can safely replace a call to [`Self::exec`].
    ///
    /// Custom executors must opt in explicitly. An executor that changes the
    /// compiler, its arguments, its inputs, or its outputs is not compatible
    /// unless those changes are independently represented in the artifact
    /// cache action identity.
    fn artifact_cache_compatible(&self) -> bool {
        false
    }

    /// In case of an `Err`, Cargo will not continue with the build process for
    /// this package.
    fn exec(
        &self,
        cmd: &ProcessBuilder,
        id: PackageId,
        target: &Target,
        mode: CompileMode,
        on_stdout_line: &mut dyn FnMut(&str) -> CargoResult<()>,
        on_stderr_line: &mut dyn FnMut(&str) -> CargoResult<()>,
    ) -> CargoResult<()>;

    /// Queried when queuing each unit of work. If it returns true, then the
    /// unit will always be rebuilt, independent of whether it needs to be.
    fn force_rebuild(&self, _unit: &Unit) -> bool {
        #[expect(
            clippy::disallowed_methods,
            reason = "test-only hook is intentionally outside user configuration"
        )]
        std::env::var_os(ARTIFACT_CACHE_FORCE_REBUILD_FOR_TESTS).is_some()
    }
}

pub(super) fn artifact_cache_freshness_preflight(
    build_runner: &mut BuildRunner<'_, '_>,
    exec: &Arc<dyn Executor>,
) -> CargoResult<()> {
    if build_runner.bcx.build_config.artifact_cache.is_none()
        || !exec.artifact_cache_compatible()
        || build_runner.bcx.gctx.cli_unstable().fine_grain_locking
        || sld_native_incremental_requested(build_runner)
    {
        return Ok(());
    }
    let preflight_started = build_runner
        .artifact_cache_stats
        .as_ref()
        .map(|_| Instant::now());
    debug_assert!(build_runner.fingerprints.is_empty());
    let roots = build_runner.bcx.roots.clone();
    let mut discovered = HashSet::new();
    let mut candidates = Vec::new();
    for unit in &roots {
        artifact_cache_freshness_preflight_candidates(
            build_runner,
            unit,
            &mut discovered,
            &mut candidates,
        )?;
    }
    let mut visited = HashSet::new();
    let mut restored = HashSet::new();
    for unit in &candidates {
        artifact_cache_freshness_preflight_unit(
            build_runner,
            exec,
            unit,
            &mut visited,
            &mut restored,
        )?;
    }

    // Recalculate from the committed consumer-side state during ordinary
    // scheduling. The files on disk, not the preflight memoization, remain the
    // freshness authority.
    build_runner.fingerprints.clear();
    build_runner.mtime_cache.clear();
    build_runner.checksum_cache.clear();
    if let Some(started) = preflight_started
        && let Some(stats) = &build_runner.artifact_cache_stats
    {
        stats.preflight_finished(started.elapsed());
    }
    Ok(())
}

fn artifact_cache_freshness_preflight_candidates(
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
    visited: &mut HashSet<Unit>,
    candidates: &mut Vec<Unit>,
) -> CargoResult<()> {
    if !visited.insert(unit.clone()) {
        return Ok(());
    }
    for dep in build_runner.unit_deps(unit).iter() {
        artifact_cache_freshness_preflight_candidates(
            build_runner,
            &dep.unit,
            visited,
            candidates,
        )?;
    }
    if artifact_cache_freshness_preflight_candidate(build_runner, unit)? {
        candidates.push(unit.clone());
    }
    Ok(())
}

fn artifact_cache_freshness_preflight_unit(
    build_runner: &mut BuildRunner<'_, '_>,
    exec: &Arc<dyn Executor>,
    unit: &Unit,
    visited: &mut HashSet<Unit>,
    restored: &mut HashSet<Unit>,
) -> CargoResult<bool> {
    if !visited.insert(unit.clone()) {
        return Ok(restored.contains(unit));
    }

    let deps = Vec::from(build_runner.unit_deps(unit));
    let mut dependency_closure_ready = true;
    for dep in deps
        .into_iter()
        .filter(|dep| !dep.unit.target.is_bin() || dep.unit.artifact.is_true())
    {
        if !artifact_cache_freshness_preflight_unit(
            build_runner,
            exec,
            &dep.unit,
            visited,
            restored,
        )? {
            dependency_closure_ready = false;
        }
    }
    let force_rebuild = if !unit.mode.is_run_custom_build() && !unit.mode.is_doc_test() {
        *build_runner
            .preflight_force_rebuilds
            .entry(unit.clone())
            .or_insert_with(|| exec.force_rebuild(unit))
    } else {
        false
    };
    if force_rebuild {
        if let Some(stats) = &build_runner.artifact_cache_stats {
            stats.preflight_bypassed();
        }
        build_runner
            .preflight_artifact_cache_states
            .insert(unit.clone(), PreflightArtifactCacheState::Bypassed);
        return Ok(false);
    }
    if !dependency_closure_ready {
        if let Some(stats) = &build_runner.artifact_cache_stats {
            stats.preflight_blocked_by_dependency();
        }
        return Ok(false);
    }
    if !unit.mode.is_doc_test() && fingerprint::restored_target_is_fresh(build_runner, unit)? {
        if let Some(stats) = &build_runner.artifact_cache_stats {
            stats.preflight_already_fresh();
        }
        restored.insert(unit.clone());
        return Ok(true);
    }
    if !artifact_cache_freshness_preflight_candidate(build_runner, unit)? {
        return Ok(false);
    }
    fingerprint::clear_calculated_target(build_runner, unit);
    if let Some(stats) = &build_runner.artifact_cache_stats {
        stats.preflight_attempted();
    }

    if artifact_cache_freshness_preflight_restore(build_runner, exec, unit)? {
        restored.insert(unit.clone());
        Ok(true)
    } else {
        Ok(false)
    }
}

fn artifact_cache_freshness_preflight_candidate(
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
) -> CargoResult<bool> {
    let has_runtime_build_script_inputs = build_runner
        .build_scripts
        .get(unit)
        .is_some_and(|scripts| !scripts.to_link.is_empty() || !scripts.plugins.is_empty());
    Ok(!unit.skip_non_compile_time_dep
        && !unit.artifact.is_true()
        && matches!(unit.mode, CompileMode::Build)
        && unit.target.is_lib()
        && !unit.target.proc_macro()
        && !unit.pkg.has_custom_build()
        && !has_runtime_build_script_inputs
        && build_runner.sbom_output_files(unit)?.is_empty())
}

fn artifact_cache_freshness_preflight_restore(
    build_runner: &mut BuildRunner<'_, '_>,
    exec: &Arc<dyn Executor>,
    unit: &Unit,
) -> CargoResult<bool> {
    fingerprint::prepare_init(build_runner, unit)?;
    fingerprint::verify_source(build_runner, unit)?;
    fingerprint::invalidate_restored_target(build_runner, unit)?;

    let mut rustc = prepare_rustc(build_runner, unit)?;
    initialize_executor_once(build_runner, exec, unit);
    let outputs = build_runner.outputs(unit)?;
    let output_root = build_runner.files().output_dir(unit);
    let rustc_dep_info_loc = rustc_dep_info_loc(build_runner, unit, &output_root);
    let dep_info_loc = fingerprint::dep_info_loc(build_runner, unit);
    let fingerprint_dir = build_runner.files().fingerprint_dir(unit);
    let message_cache_path = build_runner.files().message_cache_path(unit);
    let build_dir = build_runner.bcx.ws.build_dir().into_path_unlocked();
    let pkg_root = unit.pkg.root().to_path_buf();
    let cwd = rustc
        .get_cwd()
        .unwrap_or_else(|| build_runner.bcx.gctx.cwd())
        .to_path_buf();
    let sbom_files = build_runner.sbom_output_files(unit)?;
    let builder = ArtifactCacheActionBuilder::new(
        build_runner,
        unit,
        &rustc,
        &sbom_files,
        &build_dir,
        &output_root,
        &cwd,
        exec.artifact_cache_compatible(),
    )?;
    builder.record_static_rejection(build_runner.artifact_cache_stats.as_deref());

    // Capture the consumer-side build boundary before verifying any cached
    // source or action input.
    let timestamp = paths::set_invocation_time(&fingerprint_dir)?;
    let action = builder.describe(
        &mut rustc,
        true,
        build_runner.artifact_cache_stats.as_deref(),
    )?;
    let Some(action) = action else {
        if let Some(stats) = &build_runner.artifact_cache_stats {
            stats.preflight_bypassed();
        }
        build_runner
            .preflight_artifact_cache_states
            .insert(unit.clone(), PreflightArtifactCacheState::Bypassed);
        return Ok(false);
    };
    let cache_hit = restore_artifact_cache_action(
        Some(&action),
        outputs.as_slice(),
        &rustc_dep_info_loc,
        &message_cache_path,
        &rustc,
        &cwd,
        &pkg_root,
        &output_root,
        unit.mode,
        build_runner.artifact_cache_stats.as_deref(),
        build_runner.artifact_cache_snapshot.as_deref(),
    );
    if !cache_hit {
        build_runner
            .preflight_artifact_cache_states
            .insert(unit.clone(), PreflightArtifactCacheState::Miss(action));
        return Ok(false);
    }

    let env_config = Arc::clone(build_runner.bcx.gctx.env_config()?);
    finish_rustc_target_state(
        &rustc_dep_info_loc,
        &dep_info_loc,
        &cwd,
        &pkg_root,
        &build_dir,
        &rustc,
        unit.is_local(),
        &env_config,
        timestamp,
        unit.mode,
        outputs.as_slice(),
        &fingerprint_dir,
        false,
        true,
    )?;
    if fingerprint::commit_restored_target(build_runner, unit)? {
        if let Some(stats) = &build_runner.artifact_cache_stats {
            stats.preflight_finalized();
        }
        debug!(
            "artifact cache preflight made {} Cargo-fresh",
            unit.target.crate_name()
        );
        Ok(true)
    } else {
        if let Some(stats) = &build_runner.artifact_cache_stats {
            stats.preflight_bypassed();
        }
        debug!(
            "artifact cache preflight could not make {} Cargo-fresh",
            unit.target.crate_name()
        );
        build_runner
            .preflight_artifact_cache_states
            .insert(unit.clone(), PreflightArtifactCacheState::Miss(action));
        Ok(false)
    }
}

/// A `DefaultExecutor` calls rustc without doing anything else. It is Cargo's
/// default behaviour.
#[derive(Copy, Clone)]
pub struct DefaultExecutor;

impl Executor for DefaultExecutor {
    fn artifact_cache_compatible(&self) -> bool {
        true
    }

    fn init(&self, _build_runner: &BuildRunner<'_, '_>, unit: &Unit) {
        #[expect(
            clippy::disallowed_methods,
            reason = "test-only hook is intentionally outside user configuration"
        )]
        if let Some(path) = env::var_os(ARTIFACT_CACHE_EXECUTOR_INIT_LOG_FOR_TESTS) {
            let mut log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("failed to open artifact cache executor init test log");
            writeln!(log, "{}", unit.target.crate_name())
                .expect("failed to write artifact cache executor init test log");
        }
    }

    #[instrument(name = "rustc", skip_all, fields(package = id.name().as_str(), process = cmd.to_string()))]
    fn exec(
        &self,
        cmd: &ProcessBuilder,
        id: PackageId,
        _target: &Target,
        _mode: CompileMode,
        on_stdout_line: &mut dyn FnMut(&str) -> CargoResult<()>,
        on_stderr_line: &mut dyn FnMut(&str) -> CargoResult<()>,
    ) -> CargoResult<()> {
        cmd.exec_with_streaming(on_stdout_line, on_stderr_line, false)
            .map(drop)
    }
}

/// Builds up and enqueue a list of pending jobs onto the `job` queue.
///
/// Starting from the `unit`, this function recursively calls itself to build
/// all jobs for dependencies of the `unit`. Each of these jobs represents
/// compiling a particular package.
///
/// Note that **no actual work is executed as part of this**, that's all done
/// next as part of [`JobQueue::execute`] function which will run everything
/// in order with proper parallelism.
#[tracing::instrument(skip(build_runner, jobs, exec))]
fn compile<'gctx>(
    build_runner: &mut BuildRunner<'_, 'gctx>,
    jobs: &mut JobQueue<'gctx>,
    unit: &Unit,
    exec: &Arc<dyn Executor>,
) -> CargoResult<()> {
    warn_if_sld_native_incremental_unsupported(build_runner)?;
    if !build_runner.compiled.insert(unit.clone()) {
        return Ok(());
    }

    let lock = if build_runner.bcx.gctx.cli_unstable().fine_grain_locking {
        Some(build_runner.lock_manager.lock_shared(build_runner, unit)?)
    } else {
        None
    };

    // If we are in `--compile-time-deps` and the given unit is not a compile time
    // dependency, skip compiling the unit and jumps to dependencies, which still
    // have chances to be compile time dependencies
    if !unit.skip_non_compile_time_dep {
        // Build up the work to be done to compile this unit, enqueuing it once
        // we've got everything constructed.
        fingerprint::prepare_init(build_runner, unit)?;

        let job = if unit.mode.is_run_custom_build() {
            custom_build::prepare(build_runner, unit)?
        } else if unit.mode.is_doc_test() {
            // We run these targets later, so this is just a no-op for now.
            Job::new_fresh()
        } else {
            let force = build_runner
                .preflight_force_rebuilds
                .remove(unit)
                .unwrap_or_else(|| exec.force_rebuild(unit));
            let mut job = fingerprint::prepare_target(build_runner, unit, force)?;
            job.before(if job.freshness().is_dirty() {
                let work = if unit.mode.is_doc() || unit.mode.is_doc_scrape() {
                    rustdoc(build_runner, unit)?
                } else {
                    rustc(build_runner, unit, exec, force)?
                };
                work.then(link_targets(build_runner, unit, false)?)
            } else {
                if let Some(stats) = &build_runner.artifact_cache_stats {
                    stats.cargo_fresh();
                }
                let output_options = OutputOptions::for_fresh(build_runner, unit);
                let manifest = ManifestErrorContext::new(build_runner, unit);
                let work = replay_output_cache(
                    unit.pkg.package_id(),
                    manifest,
                    &unit.target,
                    build_runner.files().message_cache_path(unit),
                    output_options,
                );
                // Need to link targets on both the dirty and fresh.
                work.then(link_targets(build_runner, unit, true)?)
            });

            // If -Zfine-grain-locking is enabled, we wrap the job with an upgrade to exclusive
            // lock before starting, then downgrade to a shared lock after the job is finished.
            if build_runner.bcx.gctx.cli_unstable().fine_grain_locking && job.freshness().is_dirty()
            {
                if let Some(lock) = lock {
                    // Here we unlock the current shared lock to avoid deadlocking with other cargo
                    // processes. Then we configure our compile job to take an exclusive lock
                    // before starting. Once we are done compiling (including both rmeta and rlib)
                    // we downgrade to a shared lock to allow other cargo's to read the build unit.
                    // We will hold this shared lock for the remainder of compilation to prevent
                    // other cargo from re-compiling while we are still using the unit.
                    build_runner.lock_manager.unlock(&lock)?;
                    job.before(prebuild_lock_exclusive(lock.clone()));
                    job.after(downgrade_lock_to_shared(lock));
                }
            }

            job
        };
        jobs.enqueue(build_runner, unit, job)?;
    }

    // Be sure to compile all dependencies of this target as well.
    let deps = Vec::from(build_runner.unit_deps(unit)); // Create vec due to mutable borrow.
    for dep in deps {
        compile(build_runner, jobs, &dep.unit, exec)?;
    }

    Ok(())
}

impl ArtifactCacheActionBuilder {
    fn record_static_rejection(&self, stats: Option<&artifact_cache_stats::ArtifactCacheStats>) {
        if let Self::Rejected { reason } = self
            && let Some(stats) = stats
        {
            stats.ineligible(*reason);
        }
    }

    fn new(
        build_runner: &BuildRunner<'_, '_>,
        unit: &Unit,
        rustc: &ProcessBuilder,
        sbom_files: &[PathBuf],
        build_dir: &Path,
        output_root: &Path,
        cwd: &Path,
        executor_artifact_cache_compatible: bool,
    ) -> CargoResult<Self> {
        let Some(config) = build_runner.bcx.build_config.artifact_cache.clone() else {
            return Ok(Self::Disabled);
        };
        let crate_name = unit.target.crate_name().to_string();
        let rejection = if !executor_artifact_cache_compatible {
            Some((
                IneligibleReason::CustomExecutor,
                "custom executor did not opt in to artifact cache substitution",
            ))
        } else if !unit.target.is_lib() {
            Some((
                IneligibleReason::TargetNotLibrary,
                "target is not a library",
            ))
        } else if unit.target.proc_macro() {
            Some((IneligibleReason::ProcMacro, "target is a proc macro"))
        } else if !artifact_cache_compile_mode_is_supported(unit.mode) {
            Some((
                IneligibleReason::CompileMode,
                "compile mode is not supported",
            ))
        } else if !sbom_files.is_empty() {
            Some((IneligibleReason::Sbom, "SBOM output is enabled"))
        } else if !artifact_cache_host_is_supported() {
            Some((
                IneligibleReason::UnsupportedHost,
                "host platform is unsupported",
            ))
        } else if !artifact_cache_loader_environment_is_modeled(build_runner.bcx.gctx, rustc) {
            Some((
                IneligibleReason::UnmodeledLoaderEnvironment,
                "compiler loader environment is not safely modeled",
            ))
        } else {
            None
        };
        if let Some((reason, description)) = rejection {
            debug!("artifact cache admission rejected for {crate_name}: {description}");
            return Ok(Self::Rejected { reason });
        }

        let identity_provider = build_runner.bcx.rustc().artifact_cache_identity_provider();
        let compiler_program = build_runner.bcx.rustc().path.clone();
        let identity_program = identity_provider
            .program()
            .unwrap_or(&compiler_program)
            .to_path_buf();
        let loader_home = rustc.get_env("HOME");
        let loader_input_paths = compiler_loader_input_paths(
            loader_home.as_deref(),
            rustc,
            &identity_program,
            build_dir,
            output_root,
            cwd,
        );
        if !artifact_cache_loader_input_paths_are_modeled(&loader_input_paths) {
            let description = "compiler loader inputs are not safely modeled";
            debug!("artifact cache admission rejected for {crate_name}: {description}");
            return Ok(Self::Rejected {
                reason: IneligibleReason::UnmodeledLoaderInputs,
            });
        }
        let dependency_search_paths = lib_search_paths(build_runner, unit)?
            .chunks_exact(2)
            .filter(|pair| pair[0] == OsStr::new("-L"))
            .map(|pair| pair[1].clone())
            .collect::<Vec<_>>();
        let source_id = unit.pkg.package_id().source_id();
        let package_replacement = if source_id.is_git() {
            "/__cargo_artifact_cache_git_checkouts"
        } else if source_id.is_registry() {
            "/__cargo_artifact_cache_registry_sources"
        } else if unit
            .pkg
            .root()
            .strip_prefix(build_runner.bcx.ws.root())
            .is_ok()
        {
            "/__cargo_artifact_cache_workspace"
        } else {
            "/__cargo_artifact_cache_package"
        };
        let portable_remaps = [
            (package_remap(build_runner, unit), package_replacement),
            (
                build_dir_remap(build_runner),
                "/__cargo_artifact_cache_build_dir",
            ),
            (
                sysroot_remap(build_runner, unit),
                "/__cargo_artifact_cache_compiler_sysroot/lib/rustlib/src/rust",
            ),
        ]
        .into_iter()
        .filter_map(|(argument, replacement)| artifact_cache_portable_remap(argument, replacement))
        .collect::<Vec<_>>();

        Ok(Self::Candidate {
            crate_name,
            config,
            identity_provider,
            compiler_program,
            identity_program,
            dependency_search_paths,
            portable_remaps,
            build_dir: build_dir.to_path_buf(),
            output_root: output_root.to_path_buf(),
            cwd: cwd.to_path_buf(),
            rustc_verbose_version: build_runner.bcx.rustc().verbose_version.clone(),
            rustc_host: build_runner.bcx.rustc().host.to_string(),
        })
    }

    fn describe(
        self,
        rustc: &mut ProcessBuilder,
        require_materialized_inputs: bool,
        stats: Option<&artifact_cache_stats::ArtifactCacheStats>,
    ) -> CargoResult<Option<PreparedArtifactCacheAction>> {
        let Self::Candidate {
            crate_name,
            config,
            identity_provider,
            compiler_program,
            identity_program,
            dependency_search_paths,
            portable_remaps,
            build_dir,
            output_root,
            cwd,
            rustc_verbose_version,
            rustc_host,
        } = self
        else {
            return Ok(None);
        };

        if !rlib_action_is_cacheable_with_search_paths(
            rustc,
            &output_root,
            &compiler_program,
            &rustc_host,
            &dependency_search_paths,
        ) {
            let reason = unmodeled_rustc_action_reason(rustc);
            if let Some(stats) = stats {
                stats.ineligible(reason);
            }
            debug!(
                "artifact cache admission rejected for {crate_name}: rustc action is not safely modeled ({reason:?})"
            );
            return Ok(None);
        }
        if require_materialized_inputs
            && !artifact_cache_action_inputs_are_materialized(rustc, &cwd)
        {
            debug!(
                "artifact cache preflight deferred for {crate_name}: action inputs are not materialized"
            );
            return Ok(None);
        }
        if !artifact_cache_rustc_loader_environment_is_modeled(rustc) {
            if let Some(stats) = stats {
                stats.ineligible(IneligibleReason::UnmodeledLoaderEnvironment);
            }
            debug!(
                "artifact cache admission rejected for {crate_name}: finalized compiler loader environment is not safely modeled"
            );
            return Ok(None);
        }
        remove_cargo_injected_loader_path(rustc, &output_root)?;
        let loader_home = rustc.get_env("HOME");
        let loader_input_paths = compiler_loader_input_paths(
            loader_home.as_deref(),
            rustc,
            &identity_program,
            &build_dir,
            &output_root,
            &cwd,
        );
        if !artifact_cache_loader_input_paths_are_modeled(&loader_input_paths) {
            if let Some(stats) = stats {
                stats.ineligible(IneligibleReason::UnmodeledLoaderInputs);
            }
            debug!(
                "artifact cache admission rejected for {crate_name}: finalized compiler loader inputs are not safely modeled"
            );
            return Ok(None);
        }

        let identity_started = stats.map(|_| (Instant::now(), thread_cpu_time()));
        let (identity_snapshot, identity_computed) = identity_provider.identity_snapshot();
        if let Some((started, cpu_started)) = identity_started
            && let Some(stats) = stats
        {
            let (files, bytes) = identity_snapshot
                .as_ref()
                .map_or((0, 0), |(_, _, files, bytes)| (*files, *bytes));
            stats.compiler_identity(
                identity_computed,
                files,
                bytes,
                started.elapsed(),
                thread_cpu_time().saturating_sub(cpu_started),
            );
        }
        let Some((compiler_identity, identity_witness, _, _)) = identity_snapshot else {
            if let Some(stats) = stats {
                stats.ineligible(IneligibleReason::CompilerIdentityUnavailable);
            }
            debug!(
                "artifact cache admission rejected for {crate_name}: portable compiler identity is unavailable"
            );
            return Ok(None);
        };

        let prepared_key = match rlib_cache_entry(
            &config.dir,
            rustc,
            &build_dir,
            &output_root,
            &rustc_verbose_version,
            &compiler_identity,
            &identity_program,
            &dependency_search_paths,
            &portable_remaps,
            &loader_input_paths,
            Some(&identity_witness),
            &cwd,
            stats,
        ) {
            Ok(key) => key,
            Err(error) => {
                if let Some(stats) = stats {
                    stats.ineligible(IneligibleReason::KeyGenerationFailure);
                }
                debug!("ignoring artifact cache key failure for {crate_name}: {error:#}");
                return Ok(None);
            }
        };
        if let Some(stats) = stats {
            stats.eligible();
        }
        let command_digest = artifact_cache_command_digest(rustc);
        Ok(Some(PreparedArtifactCacheAction {
            crate_name,
            config,
            key: prepared_key.key,
            action_inputs_witness: prepared_key.action_inputs_witness,
            identity_witness,
            loader_input_paths,
            command_digest,
        }))
    }
}

fn restore_artifact_cache_action(
    action: Option<&PreparedArtifactCacheAction>,
    outputs: &[build_runner::OutputFile],
    rustc_dep_info_loc: &Path,
    message_cache_path: &Path,
    rustc: &ProcessBuilder,
    cwd: &Path,
    pkg_root: &Path,
    output_root: &Path,
    mode: CompileMode,
    stats: Option<&artifact_cache_stats::ArtifactCacheStats>,
    snapshot: Option<&artifact_cache_snapshot::Recorder>,
) -> bool {
    let restore_started = (action.is_some() && stats.is_some()).then(Instant::now);
    let mut restore_failed = false;
    let cache_hit = match action {
        Some(action) => match restore_rlib_cache(
            &action.key.entry_root,
            outputs,
            rustc_dep_info_loc,
            message_cache_path,
            rustc,
            cwd,
            pkg_root,
            output_root,
            &action.identity_witness,
            &action.loader_input_paths,
            &action.key.loader_inputs_digest,
            &action.key.action_inputs_digest,
            &action.action_inputs_witness,
            if mode.is_check() {
                ArtifactCacheMaterialization::Copy
            } else {
                action.config.materialization
            },
            action.config.max_size,
            stats,
            snapshot,
        ) {
            Ok(cache_hit) => cache_hit,
            Err(error) => {
                restore_failed = true;
                debug!(
                    "ignoring artifact cache restore failure for {}: {error:#}",
                    action.crate_name
                );
                false
            }
        },
        None => false,
    };
    if let Some(started) = restore_started
        && let Some(stats) = stats
    {
        stats.restore_finished(cache_hit, restore_failed, started.elapsed());
    }
    cache_hit
}

#[expect(clippy::too_many_arguments)]
fn finish_rustc_target_state(
    rustc_dep_info_loc: &Path,
    dep_info_loc: &Path,
    cwd: &Path,
    pkg_root: &Path,
    build_dir: &Path,
    rustc: &ProcessBuilder,
    is_local: bool,
    env_config: &Arc<HashMap<String, OsString>>,
    timestamp: filetime::FileTime,
    mode: CompileMode,
    outputs: &[build_runner::OutputFile],
    fingerprint_dir: &Path,
    preserve_sld_root_output: bool,
    artifact_cache_freshness_stamp: bool,
) -> CargoResult<()> {
    if rustc_dep_info_loc.exists() {
        fingerprint::translate_dep_info(
            rustc_dep_info_loc,
            dep_info_loc,
            cwd,
            pkg_root,
            build_dir,
            rustc,
            // Do not track source files in the fingerprint for registry dependencies.
            is_local,
            env_config,
        )
        .with_context(|| {
            internal(format!(
                "could not parse/generate dep info at: {}",
                rustc_dep_info_loc.display()
            ))
        })?;
        // This mtime shift allows Cargo to detect if a source file was
        // modified in the middle of the build.
        paths::set_file_time_no_err(dep_info_loc, timestamp);
    }

    // This mtime shift for .rmeta is a workaround as rustc incremental build
    // since rust-lang/rust#114669 (1.90.0) skips unnecessary rmeta generation.
    if mode.is_check() {
        for output in outputs {
            paths::set_file_time_no_err(&output.path, timestamp);
        }
    }
    // SLD owns the retained private output's identity, including its mtime.
    // Record Cargo's successful completion without mutating that output.
    if preserve_sld_root_output {
        let stamp = fingerprint_dir.join(SLD_NATIVE_INCREMENTAL_FRESHNESS_STAMP);
        drop(paths::create(&stamp)?);
        paths::set_file_time_no_err(stamp, timestamp);
    }

    if artifact_cache_freshness_stamp {
        let stamp = fingerprint_dir.join(ARTIFACT_CACHE_FRESHNESS_STAMP);
        drop(paths::create(&stamp)?);
        paths::set_file_time_no_err(stamp, timestamp);
    }
    Ok(())
}

/// Generates the warning message used when fallible doc-scrape units fail,
/// either for rustdoc or rustc.
fn make_failed_scrape_diagnostic(
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
    top_line: impl Display,
) -> String {
    let manifest_path = unit.pkg.manifest_path();
    let relative_manifest_path = manifest_path
        .strip_prefix(build_runner.bcx.ws.root())
        .unwrap_or(&manifest_path);

    format!(
        "\
{top_line}
    Try running with `--verbose` to see the error message.
    If an example should not be scanned, then consider adding `doc-scrape-examples = false` to its `[[example]]` definition in {}",
        relative_manifest_path.display()
    )
}

fn rustc_dep_info_loc(
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
    output_root: &Path,
) -> PathBuf {
    let dep_info_name =
        if let Some(c_extra_filename) = build_runner.files().metadata(unit).c_extra_filename() {
            format!("{}-{}.d", unit.target.crate_name(), c_extra_filename)
        } else {
            format!("{}.d", unit.target.crate_name())
        };
    output_root.join(dep_info_name)
}

/// Creates a unit of work invoking `rustc` for building the `unit`.
fn rustc(
    build_runner: &mut BuildRunner<'_, '_>,
    unit: &Unit,
    exec: &Arc<dyn Executor>,
    force_rebuild: bool,
) -> CargoResult<Work> {
    let mut rustc = prepare_rustc(build_runner, unit)?;
    let artifact_cache_stats = build_runner.artifact_cache_stats.clone();
    let artifact_cache_snapshot = build_runner.artifact_cache_snapshot.clone();

    let name = unit.pkg.name();

    let outputs = build_runner.outputs(unit)?;
    let root = build_runner.files().output_dir(unit);

    // Prepare the native lib state (extra `-L` and `-l` flags).
    let build_script_outputs = Arc::clone(&build_runner.build_script_outputs);
    let current_id = unit.pkg.package_id();
    let manifest = ManifestErrorContext::new(build_runner, unit);
    let build_scripts = build_runner.build_scripts.get(unit).cloned();

    // If we are a binary and the package also contains a library, then we
    // don't pass the `-l` flags.
    let pass_l_flag = unit.target.is_lib() || !unit.pkg.targets().iter().any(|t| t.is_lib());

    let rustc_dep_info_loc = rustc_dep_info_loc(build_runner, unit, &root);
    let dep_info_loc = fingerprint::dep_info_loc(build_runner, unit);

    let mut output_options = OutputOptions::for_dirty(build_runner, unit);
    let package_id = unit.pkg.package_id();
    let target = Target::clone(&unit.target);
    let mode = unit.mode;
    let preserve_sld_root_output = sld_native_incremental_root_output(build_runner, unit);
    let publish_sld_rlib_link_content_digest =
        sld_native_incremental_rlib_producer(build_runner, unit);
    let scope_sld_native_incremental_env = sld_native_incremental_requested(build_runner);
    let sld_native_incremental_padding_percent = rustc.get_env("SLD_INCREMENTAL_PADDING_PERCENT");
    let sld_rustc_work_product_provenance = rustc.get_env("SLD_RUSTC_WORK_PRODUCT_PROVENANCE");
    let stabilize_sld_rustc_transient_inputs =
        rustc.get_env("SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS");

    initialize_executor_once(build_runner, exec, unit);
    let exec = exec.clone();

    let root_output = build_runner.files().host_dest().map(|v| v.to_path_buf());
    let build_dir = build_runner.bcx.ws.build_dir().into_path_unlocked();
    let pkg_root = unit.pkg.root().to_path_buf();
    let cwd = rustc
        .get_cwd()
        .unwrap_or_else(|| build_runner.bcx.gctx.cwd())
        .to_path_buf();
    let fingerprint_dir = build_runner.files().fingerprint_dir(unit);
    let message_cache_path = build_runner.files().message_cache_path(unit);
    let show_cached_diagnostics = unit.show_warnings(build_runner.bcx.gctx);
    let script_metadatas = build_runner.find_build_script_metadatas(unit);
    let is_local = unit.is_local();
    let is_primary_link_rustc = artifact_cache_stats.is_some()
        && build_runner.is_primary_package(unit)
        && (matches!(unit.mode, CompileMode::Test)
            || (matches!(unit.mode, CompileMode::Build)
                && (unit.target.is_executable()
                    || unit.target.is_dylib()
                    || unit.target.is_cdylib()
                    || unit.target.is_staticlib()
                    || unit.target.proc_macro())));
    let artifact = unit.artifact;
    let sbom_files = build_runner.sbom_output_files(unit)?;
    let sbom = build_sbom(build_runner, unit)?;
    let artifact_cache_freshness_stamp = build_runner.bcx.build_config.artifact_cache.is_some()
        && exec.artifact_cache_compatible()
        && unit.target.is_lib()
        && !unit.target.proc_macro()
        && artifact_cache_compile_mode_is_supported(unit.mode)
        && sbom_files.is_empty();
    let preflight_artifact_cache_state = if force_rebuild {
        build_runner.preflight_artifact_cache_states.remove(unit);
        Some(PreflightArtifactCacheState::Bypassed)
    } else {
        build_runner.preflight_artifact_cache_states.remove(unit)
    };
    let artifact_cache_builder = preflight_artifact_cache_state
        .is_none()
        .then(|| {
            ArtifactCacheActionBuilder::new(
                build_runner,
                unit,
                &rustc,
                &sbom_files,
                &build_dir,
                &root,
                &cwd,
                exec.artifact_cache_compatible(),
            )
        })
        .transpose()?;

    let hide_diagnostics_for_scrape_unit = build_runner.bcx.unit_can_fail_for_docscraping(unit)
        && !matches!(
            build_runner.bcx.gctx.shell().verbosity(),
            Verbosity::Verbose
        );
    let failed_scrape_diagnostic = hide_diagnostics_for_scrape_unit.then(|| {
        // If this unit is needed for doc-scraping, then we generate a diagnostic that
        // describes the set of reverse-dependencies that cause the unit to be needed.
        let target_desc = unit.target.description_named();
        let mut for_scrape_units = build_runner
            .bcx
            .scrape_units_have_dep_on(unit)
            .into_iter()
            .map(|unit| unit.target.description_named())
            .collect::<Vec<_>>();
        for_scrape_units.sort();
        let for_scrape_units = for_scrape_units.join(", ");
        make_failed_scrape_diagnostic(build_runner, unit, format_args!("failed to check {target_desc} in package `{name}` as a prerequisite for scraping examples from: {for_scrape_units}"))
    });
    if hide_diagnostics_for_scrape_unit {
        output_options.show_diagnostics = false;
    }
    let env_config = Arc::clone(build_runner.bcx.gctx.env_config()?);
    return Ok(Work::new(move |state| {
        if let Some(builder) = &artifact_cache_builder {
            builder.record_static_rejection(artifact_cache_stats.as_deref());
        }
        // Artifacts are in a different location than typical units,
        // hence we must assure the crate- and target-dependent
        // directory is present.
        if artifact.is_true() {
            paths::create_dir_all(&root)?;
        }

        // Only at runtime have we discovered what the extra -L and -l
        // arguments are for native libraries, so we process those here. We
        // also need to be sure to add any -L paths for our plugins to the
        // dynamic library load path as a plugin's dynamic library may be
        // located somewhere in there.
        // Finally, if custom environment variables have been produced by
        // previous build scripts, we include them in the rustc invocation.
        if let Some(build_scripts) = build_scripts {
            let script_outputs = build_script_outputs.lock().unwrap();
            add_native_deps(
                &mut rustc,
                &script_outputs,
                &build_scripts,
                pass_l_flag,
                &target,
                current_id,
                mode,
            )?;
            if let Some(ref root_output) = root_output {
                add_plugin_deps(&mut rustc, &script_outputs, &build_scripts, root_output)?;
            }
            add_custom_flags(&mut rustc, &script_outputs, script_metadatas)?;
        }

        if scope_sld_native_incremental_env {
            remove_sld_native_incremental_env(&mut rustc);
        }
        if publish_sld_rlib_link_content_digest {
            configure_sld_native_incremental_rlib_producer(&mut rustc);
        }
        if preserve_sld_root_output {
            configure_sld_native_incremental_root(
                &mut rustc,
                sld_native_incremental_padding_percent.as_deref(),
                sld_rustc_work_product_provenance.as_deref(),
                stabilize_sld_rustc_transient_inputs.as_deref(),
            );
        }

        // Record the invocation before reading cache-discovered inputs so edits
        // racing a restore leave the restored output dirty for the next build.
        let timestamp = paths::set_invocation_time(&fingerprint_dir)?;
        let (artifact_cache_action, cache_hit) = match preflight_artifact_cache_state {
            Some(PreflightArtifactCacheState::Miss(action)) => {
                #[expect(
                    clippy::disallowed_methods,
                    reason = "test-only hook is intentionally outside user configuration"
                )]
                if std::env::var_os(ARTIFACT_CACHE_RUNTIME_COMMAND_MUTATION_FOR_TESTS).is_some() {
                    rustc
                        .arg("--cfg")
                        .arg("artifact_cache_runtime_command_mutation_for_test");
                }
                let mut cache_rustc = rustc.clone();
                remove_cargo_injected_loader_path(&mut cache_rustc, &root)?;
                if artifact_cache_command_digest(&cache_rustc) == action.command_digest {
                    rustc = cache_rustc;
                    (Some(action), false)
                } else {
                    if let Some(stats) = &artifact_cache_stats {
                        stats.preflight_bypassed();
                    }
                    debug!(
                        "artifact cache preflight command changed for {}; compiling without cache publication",
                        action.crate_name
                    );
                    (None, false)
                }
            }
            Some(PreflightArtifactCacheState::Bypassed) => (None, false),
            None => {
                let action = artifact_cache_builder.unwrap().describe(
                    &mut rustc,
                    false,
                    artifact_cache_stats.as_deref(),
                )?;
                let hit = restore_artifact_cache_action(
                    action.as_ref(),
                    outputs.as_slice(),
                    &rustc_dep_info_loc,
                    &message_cache_path,
                    &rustc,
                    &cwd,
                    &pkg_root,
                    &root,
                    mode,
                    artifact_cache_stats.as_deref(),
                    artifact_cache_snapshot.as_deref(),
                );
                (action, hit)
            }
        };

        if cache_hit {
            debug!(
                "artifact cache hit for {}",
                artifact_cache_action.as_ref().unwrap().crate_name
            );
            let mut replay_options = OutputOptions {
                format: output_options.format,
                cache_cell: None,
                show_diagnostics: show_cached_diagnostics,
                warnings_seen: 0,
                errors_seen: 0,
            };
            replay_output_cache_file(
                state,
                package_id,
                &manifest,
                &target,
                &message_cache_path,
                &mut replay_options,
            )?;
        } else {
            for output in outputs.iter() {
                let preserve_sld_output =
                    preserve_sld_root_output && output.flavor == FileFlavor::Normal;
                // Only the normal private executable is SLD-owned. Sidecars still
                // need Cargo's usual pre-write cleanup.
                if !preserve_sld_output {
                    prepare_materialized_rlib_output_for_write(&output.path)?;
                }

                // If there is both an rmeta and rlib, rustc will prefer to use the
                // rlib, even if it is older. Therefore, we must delete the rlib to
                // force using the new rmeta.
                if output.path.extension() == Some(OsStr::new("rmeta")) {
                    let dst = root.join(&output.path).with_extension("rlib");
                    if dst.exists() {
                        paths::remove_file(&dst)?;
                    }
                }

                // Some linkers do not remove the executable, but truncate and modify it.
                // That results in the old hard-link being modified even after renamed.
                // We delete the old artifact here to prevent this behavior from confusing users.
                // See rust-lang/cargo#8348.
                if output.hardlink.is_some() && fs::symlink_metadata(&output.path).is_ok() {
                    if preserve_sld_output {
                        if !sld_native_incremental_artifact_is_regular_file(&output.path)? {
                            paths::remove_file(&output.path)?;
                        } else if sld_native_incremental_artifact_has_multiple_links(&output.path)?
                        {
                            isolate_sld_native_incremental_artifact(&output.path)?;
                        }
                    } else {
                        _ = paths::remove_file(&output.path).map_err(|e| {
                            tracing::debug!(
                                "failed to delete previous output file `{:?}`: {e:?}",
                                output.path
                            );
                        });
                    }
                }
            }

            state.running(&rustc);
            for file in sbom_files {
                tracing::debug!("writing sbom to {}", file.display());
                let outfile = BufWriter::new(paths::create(&file)?);
                serde_json::to_writer(outfile, &sbom)?;
            }

            let rustc_started = artifact_cache_stats.as_ref().map(|_| Instant::now());
            let result = exec
                .exec(
                    &rustc,
                    package_id,
                    &target,
                    mode,
                    &mut |line| on_stdout_line(state, line, package_id, &target),
                    &mut |line| {
                        on_stderr_line(
                            state,
                            line,
                            package_id,
                            &manifest,
                            &target,
                            &mut output_options,
                        )
                    },
                )
                .map_err(|e| {
                    if output_options.errors_seen == 0 {
                        // If we didn't expect an error, do not require --verbose to fail.
                        // This is intended to debug
                        // https://github.com/rust-lang/crater/issues/733, where we are seeing
                        // Cargo exit unsuccessfully while seeming to not show any errors.
                        e
                    } else {
                        verbose_if_simple_exit_code(e)
                    }
                })
                .with_context(|| {
                    // adapted from rustc_errors/src/lib.rs
                    let warnings = match output_options.warnings_seen {
                        0 => String::new(),
                        1 => "; 1 warning emitted".to_string(),
                        count => format!("; {} warnings emitted", count),
                    };
                    let errors = match output_options.errors_seen {
                        0 => String::new(),
                        1 => " due to 1 previous error".to_string(),
                        count => format!(" due to {count} previous errors"),
                    };
                    let name = descriptive_pkg_name(&name, &target, &mode);
                    format!("could not compile {name}{errors}{warnings}")
                });

            if let Some(started) = rustc_started
                && let Some(stats) = &artifact_cache_stats
            {
                stats.rustc_finished(started.elapsed(), result.is_err(), is_primary_link_rustc);
            }

            if let Err(e) = result {
                if let Some(diagnostic) = failed_scrape_diagnostic {
                    state.warning(diagnostic);
                }

                return Err(e);
            }

            // Exec should never return with success *and* generate an error.
            debug_assert_eq!(output_options.errors_seen, 0);
        }

        finish_rustc_target_state(
            &rustc_dep_info_loc,
            &dep_info_loc,
            &cwd,
            &pkg_root,
            &build_dir,
            &rustc,
            is_local,
            &env_config,
            timestamp,
            mode,
            outputs.as_slice(),
            &fingerprint_dir,
            preserve_sld_root_output,
            artifact_cache_freshness_stamp,
        )?;

        if !cache_hit && let Some(action) = artifact_cache_action.as_ref() {
            if let Some(stats) = &artifact_cache_stats {
                stats.publication_attempt();
            }
            let publication_started = artifact_cache_stats.as_ref().map(|_| Instant::now());
            let store_result = store_rlib_cache(
                &action.key.entry_root,
                outputs.as_slice(),
                &rustc_dep_info_loc,
                &message_cache_path,
                timestamp,
                &cwd,
                &pkg_root,
                &build_dir,
                &root,
                &action.identity_witness,
                &action.loader_input_paths,
                &action.key.loader_inputs_digest,
                &action.key.action_inputs_digest,
                &action.action_inputs_witness,
                &rustc,
                action.config.max_size,
                artifact_cache_stats.as_deref(),
                artifact_cache_snapshot.as_deref(),
            );
            if let Some(started) = publication_started
                && let Some(stats) = &artifact_cache_stats
            {
                stats.publication_finished(
                    store_result.as_ref().copied().map_err(|_| ()),
                    started.elapsed(),
                );
            }
            match store_result {
                Ok(PublicationOutcome::Stored) => {
                    debug!("stored artifact cache entry for {}", action.crate_name)
                }
                Ok(PublicationOutcome::Skipped(_)) => {}
                Err(error) => {
                    debug!(
                        "ignoring artifact cache store failure for {}: {error:#}",
                        action.crate_name
                    );
                }
            }
        }

        Ok(())
    }));

    // Add all relevant `-L` and `-l` flags from dependencies (now calculated and
    // present in `state`) to the command provided.
    fn add_native_deps(
        rustc: &mut ProcessBuilder,
        build_script_outputs: &BuildScriptOutputs,
        build_scripts: &BuildScripts,
        pass_l_flag: bool,
        target: &Target,
        current_id: PackageId,
        mode: CompileMode,
    ) -> CargoResult<()> {
        let mut library_paths = vec![];

        for key in build_scripts.to_link.iter() {
            let output = build_script_outputs.get(key.1).ok_or_else(|| {
                internal(format!(
                    "couldn't find build script output for {}/{}",
                    key.0, key.1
                ))
            })?;
            library_paths.extend(output.library_paths.iter());
        }

        // NOTE: This very intentionally does not use the derived ord from LibraryPath because we need to
        // retain relative ordering within the same type (i.e. not lexicographic). The use of a stable sort
        // is also important here because it ensures that paths of the same type retain the same relative
        // ordering (for an unstable sort to work here, the list would need to retain the idx of each element
        // and then sort by that idx when the type is equivalent.
        library_paths.sort_by_key(|p| match p {
            LibraryPath::CargoArtifact(_) => 0,
            LibraryPath::External(_) => 1,
        });

        for path in library_paths.iter() {
            rustc.arg("-L").arg(path.as_ref());
        }

        for key in build_scripts.to_link.iter() {
            let output = build_script_outputs.get(key.1).ok_or_else(|| {
                internal(format!(
                    "couldn't find build script output for {}/{}",
                    key.0, key.1
                ))
            })?;

            if key.0 == current_id {
                if pass_l_flag {
                    for name in output.library_links.iter() {
                        rustc.arg("-l").arg(name);
                    }
                }
            }

            for (lt, arg) in &output.linker_args {
                // There was an unintentional change where cdylibs were
                // allowed to be passed via transitive dependencies. This
                // clause should have been kept in the `if` block above. For
                // now, continue allowing it for cdylib only.
                // See https://github.com/rust-lang/cargo/issues/9562
                if lt.applies_to(target, mode)
                    && (key.0 == current_id || *lt == LinkArgTarget::Cdylib)
                {
                    rustc.arg("-C").arg(format!("link-arg={}", arg));
                }
            }
        }
        Ok(())
    }
}

fn initialize_executor_once(
    build_runner: &mut BuildRunner<'_, '_>,
    exec: &Arc<dyn Executor>,
    unit: &Unit,
) {
    if build_runner.executor_initialized_units.insert(unit.clone()) {
        exec.init(build_runner, unit);
    }
}

fn unmodeled_codegen_behavior_flag(arg: &str) -> bool {
    let (flag, value) = arg
        .split_once('=')
        .map_or((arg, None), |(flag, value)| (flag, Some(value)));
    let flag = flag.replace('_', "-");
    matches!(
        flag.as_str(),
        "profile-use" | "profile-sample-use" | "llvm-args" | "save-temps"
    ) || (flag == "target-cpu" && value == Some("native"))
}

fn modeled_sysroot_codegen_backend_flag(arg: &str) -> bool {
    ["codegen-backend=", "codegen_backend="]
        .iter()
        .find_map(|prefix| arg.strip_prefix(prefix))
        .is_some_and(|backend| !backend.contains('.'))
}

fn modeled_unstable_flag(arg: &str) -> bool {
    modeled_sysroot_codegen_backend_flag(arg)
        || arg == "checksum-hash-algorithm=blake3"
        || matches!(
            arg,
            "preserve-duplicate-constants=yes" | "preserve-duplicate-constants=no"
        )
}

fn unmodeled_codegen_backend_environment(rustc: &ProcessBuilder) -> bool {
    rustc
        .get_envs()
        .keys()
        .any(|key| key.starts_with("CG_GCCJIT_") && rustc.get_env(key).is_some())
        || {
            #[expect(
                clippy::disallowed_methods,
                reason = "the standalone action predicate must inspect inherited arbitrary backend keys"
            )]
            let mut inherited = std::env::vars_os();
            inherited.any(|(key, _)| {
                key.to_str().is_some_and(|key| {
                    key.starts_with("CG_GCCJIT_") && rustc.get_env(key).is_some()
                })
            })
        }
}

fn unmodeled_sld_provenance_environment(rustc: &ProcessBuilder) -> bool {
    let key = "SLD_RUSTC_WORK_PRODUCT_PROVENANCE";
    !rustc.get_envs().contains_key(key) && rustc.get_env(key).is_some()
}

fn custom_target_spec_flag(arg: &str) -> bool {
    arg.ends_with(".json")
}

fn windows_gnu_target(target: &str) -> bool {
    target.contains("-windows-gnu")
}

#[cfg(test)]
fn rlib_action_is_cacheable(
    rustc: &ProcessBuilder,
    output_root: &Path,
    compiler_program: &Path,
    host_triple: &str,
) -> bool {
    rlib_action_is_cacheable_with_search_paths(
        rustc,
        output_root,
        compiler_program,
        host_triple,
        &[],
    )
}

fn rlib_action_is_cacheable_with_search_paths(
    rustc: &ProcessBuilder,
    output_root: &Path,
    compiler_program: &Path,
    host_triple: &str,
    modeled_dependency_search_paths: &[OsString],
) -> bool {
    let args = rustc
        .get_args()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    let target_profile_root = output_root.parent().unwrap_or(output_root);
    let action_program = paths::resolve_executable(Path::new(rustc.get_program()))
        .unwrap_or_else(|_| PathBuf::from(rustc.get_program()));
    let compiler_program = paths::resolve_executable(compiler_program)
        .unwrap_or_else(|_| compiler_program.to_path_buf());
    let action_target = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--target="))
        .or_else(|| {
            args.windows(2)
                .find_map(|pair| (pair[0] == "--target").then_some(pair[1].as_ref()))
        })
        .unwrap_or(host_triple);
    let unmodeled_environment = [
        "RUSTC_BOOTSTRAP",
        "RUSTC_FORCE_RUSTC_VERSION",
        "RUSTC_OVERRIDE_VERSION_STRING",
        "RUSTC_LOG",
        "RUSTC_LOG_COLOR",
        "RUSTC_LOG_ENTRY_EXIT",
        "RUSTC_LOG_THREAD_IDS",
        "RUSTC_LOG_BACKTRACE",
        "RUSTC_LOG_LINES",
        "RUSTC_LOG_FORMAT_JSON",
        "RUSTC_LOG_OUTPUT_TARGET",
        "RUST_TARGET_PATH",
        "CG_CLIF_FORCE_GNU_AS",
        "CG_CLIF_DISABLE_INCR_CACHE",
        "CG_CLIF_ENABLE_VERIFIER",
        "CG_CLIF_JIT_ARGS",
    ];
    rustc.get_programs().all(|program| program.to_str().is_some())
        && rustc.get_args().all(|arg| arg.to_str().is_some())
        && rustc
            .get_envs()
            .values()
            .flatten()
            .all(|value| value.to_str().is_some())
        && !rustc.get_envs().contains_key("PATH")
        && rustc.get_programs().count() == 1
        && action_program == compiler_program
        && !unmodeled_environment
            .iter()
            .any(|key| rustc.get_env(key).is_some())
        && !unmodeled_codegen_backend_environment(rustc)
        && !unmodeled_sld_provenance_environment(rustc)
        // Windows GNU raw-dylib rlibs may embed output from a PATH-selected
        // or explicitly configured dlltool, which is outside this cache key.
        && !windows_gnu_target(action_target)
        && args
            .windows(2)
            .filter(|pair| pair[0] == "--crate-type")
            .map(|pair| pair[1].as_ref())
            .eq(["lib"])
        && args
            .iter()
            .filter_map(|arg| arg.strip_prefix("--emit="))
            .all(|emit| {
                matches!(
                    emit,
                    "dep-info,metadata" | "dep-info,link" | "dep-info,metadata,link"
                )
            })
        && args.iter().filter(|arg| arg.starts_with("--emit=")).count() == 1
        && !args.windows(2).any(|pair| pair[0] == "--emit")
        && args
            .windows(2)
            .filter(|pair| pair[0] == "--out-dir")
            .count()
            == 1
        && !args.iter().any(|arg| {
            arg.starts_with("--crate-type=")
                || arg.starts_with("--out-dir=")
                || arg.starts_with("--print=")
                || arg.starts_with("--pretty=")
                || arg.starts_with("--unpretty=")
                || arg
                    .strip_prefix("--target=")
                    .is_some_and(custom_target_spec_flag)
                || arg.starts_with("--sysroot=")
                || arg.starts_with("-Zunpretty=")
                || arg.starts_with("-o")
                || arg == "--print"
                || arg == "--pretty"
                || arg == "--unpretty"
                || arg == "-Csave-temps"
                || arg.starts_with("-Csave-temps=")
                || arg
                    .strip_prefix("-C")
                    .is_some_and(unmodeled_codegen_behavior_flag)
        })
        && !args.windows(2).any(|pair| {
            (pair[0] == "--target" && custom_target_spec_flag(&pair[1]))
                || pair[0] == "--sysroot"
                || (pair[0] == "-C"
                    && (pair[1].starts_with("save-temps")
                        || unmodeled_codegen_behavior_flag(&pair[1])))
        })
        && !args.iter().any(|arg| arg == "--test")
        && !args.iter().any(|arg| arg.starts_with('@'))
        && !args.iter().any(|arg| arg.starts_with("-l"))
        && !args.iter().any(|arg| arg != "-L" && arg.starts_with("-L"))
        && !args.windows(2).any(|pair| {
            pair[0] == "-L"
                && (pair[1].starts_with("dependency=") || pair[1].starts_with("crate="))
                && !modeled_dependency_search_paths
                    .iter()
                    .any(|path| path == OsStr::new(pair[1].as_ref()))
        })
        && !args.windows(2).any(|pair| {
            if pair[0] != "-L"
                || pair[1].starts_with("dependency=")
                || pair[1].starts_with("crate=")
            {
                return false;
            }
            let path = pair[1]
                .split_once('=')
                .map_or(pair[1].as_ref(), |(_, path)| path);
            !Path::new(path).starts_with(target_profile_root)
        })
        && !args.windows(2).any(|pair| {
            pair[0] == "--extern"
                && [".dylib", ".so", ".dll"]
                    .iter()
                    .any(|suffix| pair[1].ends_with(suffix))
        })
        && !args.iter().any(|arg| arg.starts_with("--extern="))
        && !args
            .windows(2)
            .any(|pair| pair[0] == "--extern" && !pair[1].contains('='))
        && !args.iter().any(|arg| {
            arg.starts_with("link-arg=")
                || arg.starts_with("link-args=")
                || arg.starts_with("-Clink-arg=")
                || arg.starts_with("-Clink-args=")
                || (arg != "-Z"
                    && arg
                        .strip_prefix("-Z")
                        .is_some_and(|arg| !modeled_unstable_flag(arg)))
        })
        && !args
            .windows(2)
            .any(|pair| pair[0] == "-Z" && !modeled_unstable_flag(&pair[1]))
}

fn unmodeled_rustc_action_reason(rustc: &ProcessBuilder) -> IneligibleReason {
    if rustc.get_programs().count() != 1 {
        return IneligibleReason::CompilerWrapper;
    }

    let args = rustc.get_args().collect::<Vec<_>>();
    let is_dynamic_extern = |argument: &OsStr| {
        let argument = argument.to_string_lossy();
        [".dylib", ".so", ".dll"]
            .iter()
            .any(|suffix| argument.ends_with(suffix))
    };
    let has_dynamic_extern = args
        .windows(2)
        .any(|pair| pair[0] == "--extern" && is_dynamic_extern(pair[1]))
        || args.iter().any(|argument| {
            argument
                .to_string_lossy()
                .strip_prefix("--extern=")
                .is_some_and(|argument| is_dynamic_extern(OsStr::new(argument)))
        });
    if has_dynamic_extern {
        IneligibleReason::DynamicExtern
    } else {
        IneligibleReason::UnmodeledRustcAction
    }
}

#[cfg(test)]
mod artifact_cache_admission_tests {
    use super::*;

    fn ordinary_library_command(emit: &str) -> ProcessBuilder {
        let mut rustc = ProcessBuilder::new("rustc");
        rustc
            .arg("--crate-type")
            .arg("lib")
            .arg(format!("--emit={emit}"))
            .arg("--out-dir")
            .arg("target/debug/deps");
        rustc
    }

    #[test]
    fn unmodeled_action_reasons_distinguish_wrappers_and_dynamic_externs() {
        let mut dynamic_extern = ordinary_library_command("dep-info,metadata");
        dynamic_extern
            .arg("--extern")
            .arg("derive=/target/libderive.dylib");
        assert_eq!(
            unmodeled_rustc_action_reason(&dynamic_extern),
            IneligibleReason::DynamicExtern
        );
        let mut compact_dynamic_extern = ordinary_library_command("dep-info,metadata");
        compact_dynamic_extern.arg("--extern=derive=/target/libderive.so");
        assert_eq!(
            unmodeled_rustc_action_reason(&compact_dynamic_extern),
            IneligibleReason::DynamicExtern
        );

        let wrapped =
            ordinary_library_command("dep-info,metadata").wrapped(Some(Path::new("clippy-driver")));
        assert_eq!(
            unmodeled_rustc_action_reason(&wrapped),
            IneligibleReason::CompilerWrapper
        );

        let mut other = ordinary_library_command("dep-info,metadata");
        other.arg("--test");
        assert_eq!(
            unmodeled_rustc_action_reason(&other),
            IneligibleReason::UnmodeledRustcAction
        );
    }

    fn ordinary_rlib_command() -> ProcessBuilder {
        ordinary_library_command("dep-info,metadata,link")
    }

    #[test]
    fn metadata_only_library_actions_are_cacheable() {
        assert!(rlib_action_is_cacheable(
            &ordinary_library_command("dep-info,metadata"),
            Path::new("target/debug/deps"),
            Path::new("rustc"),
            "aarch64-apple-darwin"
        ));
    }

    #[test]
    fn metadata_only_test_actions_remain_ineligible() {
        assert!(artifact_cache_compile_mode_is_supported(CompileMode::Build));
        assert!(artifact_cache_compile_mode_is_supported(
            CompileMode::Check { test: false }
        ));
        assert!(!artifact_cache_compile_mode_is_supported(
            CompileMode::Check { test: true }
        ));
    }

    #[test]
    fn profile_inputs_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";
        assert!(rlib_action_is_cacheable(
            &ordinary_rlib_command(),
            output_root,
            compiler,
            host
        ));
        for flag in [
            "profile-use",
            "profile_use",
            "profile-sample-use",
            "profile_sample_use",
        ] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-C{flag}=profile.profdata"));
            assert!(!rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split.arg("-C").arg(format!("{flag}=profile.profdata"));
            assert!(!rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }
    }

    #[test]
    fn profile_generation_is_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        let mut compact = ordinary_rlib_command();
        compact.arg("-Cprofile-generate=profile-output");
        assert!(rlib_action_is_cacheable(
            &compact,
            output_root,
            compiler,
            host
        ));

        let mut split = ordinary_rlib_command();
        split.arg("-C").arg("profile-generate=profile-output");
        assert!(rlib_action_is_cacheable(
            &split,
            output_root,
            compiler,
            host
        ));
    }

    #[test]
    fn arbitrary_llvm_arguments_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        for flag in ["llvm-args", "llvm_args"] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-C{flag}=-load=/path/to/plugin.dylib"));
            assert!(!rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split
                .arg("-C")
                .arg(format!("{flag}=-load=/path/to/plugin.dylib"));
            assert!(!rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }
    }

    #[test]
    fn host_cpu_detection_is_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        for flag in ["target-cpu", "target_cpu"] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-C{flag}=native"));
            assert!(!rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split.arg("-C").arg(format!("{flag}=native"));
            assert!(!rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }
    }

    #[test]
    fn temporary_outputs_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        for flag in ["save-temps", "save_temps"] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-C{flag}"));
            assert!(!rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split.arg("-C").arg(flag);
            assert!(!rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }
    }

    #[test]
    fn compact_output_overrides_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        let mut compact = ordinary_rlib_command();
        compact.arg("-ocustom-output");
        assert!(!rlib_action_is_cacheable(
            &compact,
            output_root,
            compiler,
            host
        ));
    }

    #[test]
    fn compiler_dispatch_and_backend_controls_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        for (key, value) in [
            ("PATH", "/configured/compiler/path"),
            ("CG_CLIF_FORCE_GNU_AS", "1"),
            ("CG_CLIF_DISABLE_INCR_CACHE", "1"),
            ("CG_CLIF_ENABLE_VERIFIER", "1"),
            ("CG_CLIF_JIT_ARGS", "--example"),
            ("CG_GCCJIT_DUMP_TO_FILE", "1"),
        ] {
            let mut rustc = ordinary_rlib_command();
            rustc.env(key, value);
            assert!(
                !rlib_action_is_cacheable(&rustc, output_root, compiler, host),
                "{key} should disable cache restoration"
            );
        }
    }

    #[test]
    fn path_backed_codegen_backends_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";
        for flag in ["codegen-backend", "codegen_backend"] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-Z{flag}=path/to/backend.dylib"));
            assert!(!rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split.arg("-Z").arg(format!("{flag}=path/to/backend.dylib"));
            assert!(!rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }

        let mut sysroot_backend = ordinary_rlib_command();
        sysroot_backend.arg("-Z").arg("codegen-backend=cranelift");
        assert!(rlib_action_is_cacheable(
            &sysroot_backend,
            output_root,
            compiler,
            host
        ));

        let mut compact_sysroot_backend = ordinary_rlib_command();
        compact_sysroot_backend.arg("-Zcodegen-backend=cranelift");
        assert!(rlib_action_is_cacheable(
            &compact_sysroot_backend,
            output_root,
            compiler,
            host
        ));
    }

    #[test]
    fn duplicate_constant_preservation_is_cacheable_only_with_modeled_values() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";
        for value in ["yes", "no"] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-Zpreserve-duplicate-constants={value}"));
            assert!(rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split
                .arg("-Z")
                .arg(format!("preserve-duplicate-constants={value}"));
            assert!(rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }

        for option in [
            "preserve-duplicate-constants",
            "preserve-duplicate-constants=maybe",
        ] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-Z{option}"));
            assert!(!rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split.arg("-Z").arg(option);
            assert!(!rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }
    }

    #[test]
    fn sld_provenance_with_duplicate_constant_preservation_is_cacheable() {
        let mut command = ordinary_rlib_command();
        command
            .arg("-Zpreserve-duplicate-constants=yes")
            .env("SLD_RUSTC_WORK_PRODUCT_PROVENANCE", "1");

        assert!(rlib_action_is_cacheable(
            &command,
            Path::new("target/debug/deps"),
            Path::new("rustc"),
            "aarch64-apple-darwin"
        ));
    }

    #[test]
    fn checksum_freshness_hash_algorithm_is_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        let mut split = ordinary_rlib_command();
        split.arg("-Z").arg("checksum-hash-algorithm=blake3");
        assert!(rlib_action_is_cacheable(
            &split,
            output_root,
            compiler,
            host
        ));

        let mut compact = ordinary_rlib_command();
        compact.arg("-Zchecksum-hash-algorithm=blake3");
        assert!(rlib_action_is_cacheable(
            &compact,
            output_root,
            compiler,
            host
        ));

        let mut unmodeled_algorithm = ordinary_rlib_command();
        unmodeled_algorithm
            .arg("-Z")
            .arg("checksum-hash-algorithm=unmodeled");
        assert!(!rlib_action_is_cacheable(
            &unmodeled_algorithm,
            output_root,
            compiler,
            host
        ));
    }

    #[test]
    fn unmodeled_unstable_options_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";
        for option in [
            "llvm-plugins=path/to/plugin.dylib",
            "sanitizer-dataflow-abilist=path/to/list.txt",
            "remark-dir=path/to/remarks",
            "metrics-dir=path/to/metrics",
            "self-profile=path/to/profile",
        ] {
            let mut compact = ordinary_rlib_command();
            compact.arg(format!("-Z{option}"));
            assert!(!rlib_action_is_cacheable(
                &compact,
                output_root,
                compiler,
                host
            ));

            let mut split = ordinary_rlib_command();
            split.arg("-Z").arg(option);
            assert!(!rlib_action_is_cacheable(
                &split,
                output_root,
                compiler,
                host
            ));
        }
    }

    #[test]
    fn custom_target_specs_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";
        let mut compact = ordinary_rlib_command();
        compact.arg("--target=custom-target.json");
        assert!(!rlib_action_is_cacheable(
            &compact,
            output_root,
            compiler,
            host
        ));

        let mut split = ordinary_rlib_command();
        split.arg("--target").arg("path/to/custom-target.json");
        assert!(!rlib_action_is_cacheable(
            &split,
            output_root,
            compiler,
            host
        ));
    }

    #[test]
    fn explicit_sysroots_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";
        let mut compact = ordinary_rlib_command();
        compact.arg("--sysroot=path/to/sysroot");
        assert!(!rlib_action_is_cacheable(
            &compact,
            output_root,
            compiler,
            host
        ));

        let mut split = ordinary_rlib_command();
        split.arg("--sysroot").arg("path/to/sysroot");
        assert!(!rlib_action_is_cacheable(
            &split,
            output_root,
            compiler,
            host
        ));
    }

    #[test]
    fn custom_target_search_paths_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let mut rustc = ordinary_rlib_command();
        rustc.env("RUST_TARGET_PATH", "path/to/targets");
        assert!(!rlib_action_is_cacheable(
            &rustc,
            output_root,
            compiler,
            "aarch64-apple-darwin"
        ));
    }

    #[test]
    fn unmodeled_dependency_search_paths_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";
        let mut rustc = ordinary_rlib_command();
        rustc.arg("-L").arg("dependency=target/debug/deps");
        assert!(!rlib_action_is_cacheable(
            &rustc,
            output_root,
            compiler,
            host
        ));
        assert!(rlib_action_is_cacheable_with_search_paths(
            &rustc,
            output_root,
            compiler,
            host,
            &[OsString::from("dependency=target/debug/deps")]
        ));
    }

    #[test]
    fn windows_gnu_targets_are_not_cacheable() {
        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");

        assert!(!rlib_action_is_cacheable(
            &ordinary_rlib_command(),
            output_root,
            compiler,
            "x86_64-pc-windows-gnu"
        ));

        let mut explicit = ordinary_rlib_command();
        explicit.arg("--target").arg("x86_64-pc-windows-gnu");
        assert!(!rlib_action_is_cacheable(
            &explicit,
            output_root,
            compiler,
            "aarch64-apple-darwin"
        ));

        let mut compact = ordinary_rlib_command();
        compact.arg("--target=x86_64-pc-windows-gnullvm");
        assert!(!rlib_action_is_cacheable(
            &compact,
            output_root,
            compiler,
            "aarch64-apple-darwin"
        ));
    }

    #[test]
    #[cfg(unix)]
    fn non_utf8_arguments_and_environment_are_not_cacheable() {
        use std::os::unix::ffi::OsStringExt;

        let output_root = Path::new("target/debug/deps");
        let compiler = Path::new("rustc");
        let host = "aarch64-apple-darwin";

        let mut arg = ordinary_rlib_command();
        arg.arg(OsString::from_vec(vec![0xff]));
        assert!(!rlib_action_is_cacheable(&arg, output_root, compiler, host));

        let mut env = ordinary_rlib_command();
        env.env(
            "ARTIFACT_CACHE_NON_UTF8_TEST",
            OsString::from_vec(vec![0xff]),
        );
        assert!(!rlib_action_is_cacheable(&env, output_root, compiler, host));
    }
}

fn rlib_cache_entry(
    cache_root: &Path,
    rustc: &ProcessBuilder,
    build_dir: &Path,
    output_root: &Path,
    rustc_verbose_version: &str,
    compiler_identity: &blake3::Hash,
    compiler_program: &Path,
    modeled_dependency_search_paths: &[OsString],
    portable_remaps: &[(OsString, OsString)],
    loader_input_paths: &[CompilerLoaderInput],
    identity_witness: Option<&crate::util::rustc::ArtifactCacheIdentityWitness>,
    rustc_cwd: &Path,
    stats: Option<&artifact_cache_stats::ArtifactCacheStats>,
) -> CargoResult<PreparedArtifactCacheKey> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if std::env::var_os(ARTIFACT_CACHE_KEY_FAILURE_FOR_TESTS).is_some() {
        return Err(internal("test-only artifact cache key failure".to_string()));
    }
    let target_profile_root = output_root.parent().unwrap_or(output_root);
    let compiler_sysroot = compiler_program.parent().and_then(Path::parent);
    let normalize = |value: &str| {
        let value = value
            .replace(
                &target_profile_root.to_string_lossy().to_string(),
                "/__cargo_artifact_cache_target_profile",
            )
            .replace(
                &build_dir.to_string_lossy().to_string(),
                "/__cargo_artifact_cache_build_dir",
            );
        compiler_sysroot.map_or(value.clone(), |sysroot| {
            value.replace(
                &sysroot.to_string_lossy().to_string(),
                "/__cargo_artifact_cache_compiler_sysroot",
            )
        })
    };
    let normalize_cargo_dylib_path = |value: &OsStr| -> CargoResult<OsString> {
        let mut search_path = env::split_paths(value).collect::<Vec<_>>();
        for cargo_path in &mut search_path {
            if let Some(input) = loader_input_paths.iter().find(|input| {
                input.source == OsStr::new(paths::dylib_path_envvar())
                    && input.raw_path == *cargo_path
            }) {
                match &input.location {
                    CompilerLoaderInputLocation::CompilerSysroot { relative, .. } => {
                        *cargo_path = PathBuf::from("/__cargo_artifact_cache_compiler_sysroot")
                            .join(relative);
                        continue;
                    }
                    CompilerLoaderInputLocation::TargetProfile { relative, .. } => {
                        *cargo_path =
                            PathBuf::from("/__cargo_artifact_cache_target_profile").join(relative);
                        continue;
                    }
                    CompilerLoaderInputLocation::BuildDir { relative, .. } => {
                        *cargo_path =
                            PathBuf::from("/__cargo_artifact_cache_build_dir").join(relative);
                        continue;
                    }
                    CompilerLoaderInputLocation::RustcCwd { relative, .. } => {
                        *cargo_path =
                            PathBuf::from("/__cargo_artifact_cache_rustc_cwd_loader_root")
                                .join(relative);
                        continue;
                    }
                    CompilerLoaderInputLocation::Absolute => {}
                }
            }
            *cargo_path = PathBuf::from(normalize(&cargo_path.to_string_lossy()));
        }
        paths::join_paths(&search_path, paths::dylib_path_envvar())
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-v8\0");
    hasher.update(b"rustc-verbose-version\0");
    hasher.update(rustc_verbose_version.as_bytes());
    hasher.update(b"\0");
    hasher.update(b"rustc-compiler-identity\0");
    hasher.update(compiler_identity.as_bytes());
    hasher.update(b"\0");
    let args = rustc.get_args().collect::<Vec<_>>();
    for (index, arg) in args.iter().enumerate() {
        let value = arg.to_string_lossy();
        let normalize_operand = index > 0
            && (args[index - 1] == OsStr::new("--out-dir")
                || args[index - 1] == OsStr::new("--extern")
                || (args[index - 1] == OsStr::new("-L")
                    && ((!value.starts_with("dependency=") && !value.starts_with("crate="))
                        || modeled_dependency_search_paths
                            .iter()
                            .any(|path| path == OsStr::new(value.as_ref()))))
                || (args[index - 1] == OsStr::new("-C") && value.starts_with("incremental=")));
        if let Some((_, remap)) = portable_remaps
            .iter()
            .find(|(original, _)| original == *arg)
        {
            hasher.update(remap.as_encoded_bytes());
        } else if normalize_operand || value.starts_with("-Cincremental=") {
            hasher.update(normalize(&value).as_bytes());
        } else {
            hasher.update(value.as_bytes());
        }
        hasher.update(b"\0");
    }
    let mut envs = rustc.get_envs().iter().collect::<Vec<_>>();
    envs.sort_by_key(|(key, _)| *key);
    for (key, value) in envs {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        if let Some(value) = value {
            if key == "OUT_DIR" || key.ends_with("_OUT_DIR") {
                hasher.update(normalize(&value.to_string_lossy()).as_bytes());
            } else if key == paths::dylib_path_envvar() {
                hasher.update(
                    normalize_cargo_dylib_path(value.as_os_str())?
                        .to_string_lossy()
                        .as_bytes(),
                );
            } else if key == "CARGO_MANIFEST_DIR" {
                hasher.update(b"/__cargo_artifact_cache_manifest_dir");
            } else if key == "CARGO_MANIFEST_PATH" {
                hasher.update(b"/__cargo_artifact_cache_manifest_path");
            } else if key == crate::CARGO_ENV {
                hasher.update(b"/__cargo_artifact_cache_cargo");
            } else {
                hasher.update(value.to_string_lossy().as_bytes());
            }
        }
        hasher.update(b"\0");
    }
    let loader_inputs_digest = compiler_loader_inputs_digest(loader_input_paths, identity_witness)?;
    hasher.update(b"compiler-loader-inputs-content\0");
    hasher.update(loader_inputs_digest.as_bytes());
    hasher.update(b"\0");
    let action_inputs = artifact_cache_action_inputs_snapshot(rustc, rustc_cwd, stats)?;
    hasher.update(b"action-inputs-content\0");
    hasher.update(action_inputs.digest.as_bytes());
    hasher.update(b"\0");
    for key in APPLE_DEPLOYMENT_TARGET_ENVIRONMENT {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        if let Some(value) = rustc.get_env(key) {
            hasher.update(value.as_encoded_bytes());
        }
        hasher.update(b"\0");
    }
    Ok(PreparedArtifactCacheKey {
        key: ArtifactCacheKey {
            entry_root: cache_root.join(hasher.finalize().to_hex().as_str()),
            loader_inputs_digest,
            action_inputs_digest: action_inputs.digest,
        },
        action_inputs_witness: action_inputs.witness,
    })
}

fn artifact_cache_portable_remap(
    argument: OsString,
    replacement_source: &str,
) -> Option<(OsString, OsString)> {
    let value = argument.to_str()?.strip_prefix("--remap-path-prefix=")?;
    let (_, destination) = value.rsplit_once('=')?;
    let normalized = OsString::from(format!(
        "--remap-path-prefix={replacement_source}={destination}"
    ));
    Some((argument, normalized))
}

#[cfg(test)]
mod artifact_cache_key_tests {
    use super::*;

    #[test]
    fn runner_locations_do_not_change_cache_key() {
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("cache");
        let compiler_identity = blake3::hash(b"portable compiler identity");
        let cache_entry = |runner_root: &Path,
                           alternate_layout: bool,
                           include_path_cfg: bool,
                           relative_loader: bool| {
            let workspace = runner_root.join("workspace");
            let build_dir = workspace.join(if alternate_layout {
                "target-b"
            } else {
                "target-a"
            });
            let toolchain = if alternate_layout {
                runner_root.join("toolchain")
            } else {
                build_dir.join("toolchain")
            };
            let package = workspace.join("package");
            let git_checkouts = if alternate_layout {
                runner_root.join("cargo").join("git").join("checkouts")
            } else {
                workspace.join(".cargo").join("git").join("checkouts")
            };
            let output_root = runner_root.join("target").join("debug").join("deps");
            let target_profile_loader = output_root.parent().unwrap().join("host-loader");
            let rustc_cwd = package.clone();
            let cwd_loader = rustc_cwd.join("relative-loader");
            for directory in [
                &target_profile_loader,
                &cwd_loader,
                &build_dir,
                &output_root,
                &rustc_cwd,
            ] {
                std::fs::create_dir_all(directory).unwrap();
            }
            std::fs::write(target_profile_loader.join("libhost.so"), b"host").unwrap();
            std::fs::write(cwd_loader.join("librelative.so"), b"relative").unwrap();

            let program = toolchain.join("bin").join("rustc");
            let loader_root = toolchain.join("lib");
            std::fs::create_dir_all(&loader_root).unwrap();
            std::fs::write(loader_root.join("librustc_driver.so"), b"driver").unwrap();
            let (raw_host_loader, resolved_host_loader) = if relative_loader {
                (PathBuf::from("relative-loader"), cwd_loader)
            } else {
                (target_profile_loader.clone(), target_profile_loader)
            };
            let loader_path = paths::join_paths(
                &[raw_host_loader.clone(), loader_root.clone()],
                paths::dylib_path_envvar(),
            )
            .unwrap();
            let mut rustc = ProcessBuilder::new(toolchain.join("rustup-proxy"));
            let remap_arguments = [
                OsString::from(format!(
                    "--remap-path-prefix={}=/workspace",
                    workspace.display()
                )),
                OsString::from(format!(
                    "--remap-path-prefix={}=/package",
                    package.display()
                )),
                OsString::from(format!(
                    "--remap-path-prefix={}=/cargo/build-dir",
                    build_dir.display()
                )),
                OsString::from(format!(
                    "--remap-path-prefix={}/lib/rustlib/src/rust=/rustc/test",
                    toolchain.display()
                )),
                OsString::from(format!("--remap-path-prefix={}=", git_checkouts.display())),
            ];
            rustc.args(&remap_arguments);
            if include_path_cfg {
                rustc.arg(format!("--cfg=runner_path=\"{}\"", runner_root.display()));
            }
            rustc.env(paths::dylib_path_envvar(), loader_path);
            rustc.env(crate::CARGO_ENV, toolchain.join("bin").join("cargo"));
            let portable_remaps = [
                artifact_cache_portable_remap(
                    remap_arguments[0].clone(),
                    "/__cargo_artifact_cache_workspace",
                )
                .unwrap(),
                artifact_cache_portable_remap(
                    remap_arguments[1].clone(),
                    "/__cargo_artifact_cache_package",
                )
                .unwrap(),
                artifact_cache_portable_remap(
                    remap_arguments[2].clone(),
                    "/__cargo_artifact_cache_build_dir",
                )
                .unwrap(),
                artifact_cache_portable_remap(
                    remap_arguments[3].clone(),
                    "/__cargo_artifact_cache_compiler_sysroot/lib/rustlib/src/rust",
                )
                .unwrap(),
                artifact_cache_portable_remap(
                    remap_arguments[4].clone(),
                    "/__cargo_artifact_cache_git_checkouts",
                )
                .unwrap(),
            ];
            rlib_cache_entry(
                &cache_root,
                &rustc,
                &build_dir,
                &output_root,
                "rustc 1.0.0\nhost: test-host",
                &compiler_identity,
                &program,
                &[],
                &portable_remaps,
                &[
                    compiler_loader_input(
                        paths::dylib_path_envvar(),
                        raw_host_loader,
                        resolved_host_loader,
                        Some(&toolchain),
                        output_root.parent().unwrap(),
                        &build_dir,
                        &rustc_cwd,
                    ),
                    compiler_loader_input(
                        paths::dylib_path_envvar(),
                        loader_root.clone(),
                        loader_root,
                        Some(&toolchain),
                        output_root.parent().unwrap(),
                        &build_dir,
                        &rustc_cwd,
                    ),
                ],
                None,
                &rustc_cwd,
                None,
            )
            .unwrap()
            .key
        };

        let first_runner = temp.path().join("first=runner");
        let second_runner = temp.path().join("second=runner");
        assert_eq!(
            cache_entry(&first_runner, false, false, false),
            cache_entry(&second_runner, true, false, false)
        );
        assert_eq!(
            cache_entry(&first_runner, false, false, true),
            cache_entry(&second_runner, true, false, true)
        );
        assert_ne!(
            cache_entry(&first_runner, false, true, false),
            cache_entry(&second_runner, true, true, false)
        );
    }

    #[test]
    fn loader_digest_frames_fields() {
        let mut split = blake3::Hasher::new();
        update_framed_loader_digest(&mut split, b"a");
        update_framed_loader_digest(&mut split, b"bc");

        let mut joined = blake3::Hasher::new();
        update_framed_loader_digest(&mut joined, b"ab");
        update_framed_loader_digest(&mut joined, b"c");

        assert_ne!(split.finalize(), joined.finalize());
    }

    #[test]
    fn loader_path_escape_is_not_normalized() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("target").join("debug");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let escaped = root.join("..").join("..").join("outside");

        let input = compiler_loader_input(
            paths::dylib_path_envvar(),
            escaped.clone(),
            escaped,
            None,
            &root,
            &temp.path().join("build"),
            &temp.path().join("source"),
        );

        assert!(matches!(
            input.location,
            CompilerLoaderInputLocation::Absolute
        ));
    }

    #[cfg(unix)]
    #[test]
    fn loader_symlink_escape_is_not_normalized() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("target").join("debug");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let escaped = root.join("loader");
        symlink(&outside, &escaped).unwrap();

        let input = compiler_loader_input(
            paths::dylib_path_envvar(),
            escaped.clone(),
            escaped,
            None,
            &root,
            &temp.path().join("build"),
            &temp.path().join("source"),
        );

        assert!(matches!(
            input.location,
            CompilerLoaderInputLocation::Absolute
        ));
    }

    #[test]
    fn unmodeled_sysroot_loader_contents_change_cache_key() {
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("cache");
        let toolchain = temp.path().join("toolchain");
        let rustc_program = toolchain.join("bin").join("rustc");
        let rustlib = toolchain.join("lib").join("rustlib");
        let custom_loader = toolchain.join("custom-loader");
        let build_dir = temp.path().join("build");
        let output_root = temp.path().join("target").join("debug").join("deps");
        let rustc_cwd = temp.path().join("source");
        for directory in [
            rustc_program.parent().unwrap(),
            &rustlib,
            &custom_loader,
            &build_dir,
            &output_root,
            &rustc_cwd,
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(&rustc_program, b"rustc").unwrap();
        std::fs::write(
            toolchain.join("lib").join("librustc_driver.dylib"),
            b"\xca\xfe\xba\xbfdriver",
        )
        .unwrap();
        let custom_library = custom_loader.join("rustc_driver_dependency");
        let dynamic_elf = |payload: &[u8]| {
            let mut contents = vec![0; 20];
            contents[..4].copy_from_slice(b"\x7fELF");
            contents[5] = 1;
            contents[16..18].copy_from_slice(&3u16.to_le_bytes());
            contents.extend_from_slice(payload);
            contents
        };
        std::fs::write(&custom_library, dynamic_elf(b"first")).unwrap();
        let (compiler_identity, identity_witness) =
            crate::util::rustc::artifact_cache_identity_for_program_for_test(&rustc_program)
                .unwrap();
        #[cfg(unix)]
        {
            let toolchain_alias = temp.path().join("toolchain-alias");
            std::os::unix::fs::symlink(&toolchain, &toolchain_alias).unwrap();
            let alias_lib = toolchain_alias.join("lib");
            let aliased_loader_input = compiler_loader_input(
                paths::dylib_path_envvar(),
                alias_lib.clone(),
                alias_lib,
                Some(&toolchain),
                output_root.parent().unwrap(),
                &build_dir,
                &rustc_cwd,
            );
            let canonical_lib = toolchain.join("lib");
            let canonical_loader_input = compiler_loader_input(
                paths::dylib_path_envvar(),
                canonical_lib.clone(),
                canonical_lib,
                Some(&toolchain),
                output_root.parent().unwrap(),
                &build_dir,
                &rustc_cwd,
            );
            let canonical_digest = compiler_loader_inputs_digest(
                std::slice::from_ref(&canonical_loader_input),
                Some(&identity_witness),
            )
            .unwrap();
            let aliased_digest = compiler_loader_inputs_digest(
                std::slice::from_ref(&aliased_loader_input),
                Some(&identity_witness),
            )
            .unwrap();
            if cfg!(target_os = "macos") {
                assert_eq!(canonical_digest, aliased_digest);
            } else {
                assert_ne!(canonical_digest, aliased_digest);
            }
            assert_eq!(
                compiler_identity_statically_covers_loader_input(
                    &identity_witness,
                    &aliased_loader_input,
                    &toolchain,
                    Path::new("lib"),
                ),
                cfg!(target_os = "macos")
            );
            let replacement_toolchain = temp.path().join("replacement-toolchain");
            std::fs::create_dir_all(&replacement_toolchain).unwrap();
            std::os::unix::fs::symlink(toolchain.join("bin"), replacement_toolchain.join("lib"))
                .unwrap();
            std::fs::remove_file(&toolchain_alias).unwrap();
            std::os::unix::fs::symlink(&replacement_toolchain, &toolchain_alias).unwrap();
            assert!(!compiler_identity_statically_covers_loader_input(
                &identity_witness,
                &aliased_loader_input,
                &toolchain,
                Path::new("lib"),
            ));
        }
        let rustc = ProcessBuilder::new(toolchain.join("rustup-proxy"));
        let loader_input = compiler_loader_input(
            paths::dylib_path_envvar(),
            custom_loader.clone(),
            custom_loader,
            Some(&toolchain),
            output_root.parent().unwrap(),
            &build_dir,
            &rustc_cwd,
        );
        let cache_entry = || {
            rlib_cache_entry(
                &cache_root,
                &rustc,
                &build_dir,
                &output_root,
                "rustc 1.0.0\nhost: test-host",
                &compiler_identity,
                &rustc_program,
                &[],
                &[],
                std::slice::from_ref(&loader_input),
                Some(&identity_witness),
                &rustc_cwd,
                None,
            )
            .unwrap()
            .key
            .entry_root
        };

        let first = cache_entry();
        std::fs::write(custom_library, dynamic_elf(b"other")).unwrap();
        let second = cache_entry();

        assert_ne!(first, second);
    }

    #[test]
    fn loader_binary_magic_distinguishes_macho_objects_and_fat_libraries() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("extensionless-library");
        std::fs::write(&library, b"\xca\xfe\xba\xbfcontents").unwrap();
        let object = temp.path().join("extensionless-object");
        let mut object_header = [0; 20];
        object_header[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        object_header[12..16].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(&object, object_header).unwrap();

        assert!(is_potential_dynamic_library(&library).unwrap());
        assert!(!is_potential_dynamic_library(&object).unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn action_input_witness_falls_back_after_same_mtime_content_change() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("dependency.rlib");
        std::fs::write(&input, b"before").unwrap();
        let mtime =
            filetime::FileTime::from_last_modification_time(&std::fs::metadata(&input).unwrap());
        let mut rustc = ProcessBuilder::new("rustc");
        rustc
            .arg("--extern")
            .arg(format!("dependency={}", input.display()));
        let snapshot = artifact_cache_action_inputs_snapshot(&rustc, temp.path(), None).unwrap();
        let mut witness = snapshot.witness;
        assert!(witness.is_current());

        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&input, b"after!").unwrap();
        filetime::set_file_mtime(&input, mtime).unwrap();

        assert!(!witness.is_current());
        assert!(
            !artifact_cache_action_inputs_are_current(
                &rustc,
                temp.path(),
                &snapshot.digest,
                &mut witness,
                None,
            )
            .unwrap()
        );
    }

    #[test]
    #[cfg(unix)]
    fn action_input_witness_refreshes_after_unchanged_rewrite() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("dependency.rlib");
        std::fs::write(&input, b"contents").unwrap();
        let mtime =
            filetime::FileTime::from_last_modification_time(&std::fs::metadata(&input).unwrap());
        let mut rustc = ProcessBuilder::new("rustc");
        rustc
            .arg("--extern")
            .arg(format!("dependency={}", input.display()));
        let snapshot = artifact_cache_action_inputs_snapshot(&rustc, temp.path(), None).unwrap();
        let mut witness = snapshot.witness;

        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&input, b"contents").unwrap();
        filetime::set_file_mtime(&input, mtime).unwrap();

        assert!(!witness.is_current());
        assert!(
            artifact_cache_action_inputs_are_current(
                &rustc,
                temp.path(),
                &snapshot.digest,
                &mut witness,
                None,
            )
            .unwrap()
        );
        assert!(witness.is_current());
    }

    #[test]
    fn action_input_tree_witness_detects_added_files() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();
        std::fs::write(out_dir.join("generated.rs"), b"before").unwrap();
        let mut rustc = ProcessBuilder::new("rustc");
        rustc.env("OUT_DIR", &out_dir);
        let snapshot = artifact_cache_action_inputs_snapshot(&rustc, temp.path(), None).unwrap();
        let mut witness = snapshot.witness;
        assert!(witness.is_current());

        std::fs::write(out_dir.join("additional.rs"), b"after").unwrap();

        assert!(!witness.is_current());
        assert!(
            !artifact_cache_action_inputs_are_current(
                &rustc,
                temp.path(),
                &snapshot.digest,
                &mut witness,
                None,
            )
            .unwrap()
        );
    }

    #[test]
    fn action_input_witness_detects_newly_created_tree() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("out");
        let mut rustc = ProcessBuilder::new("rustc");
        rustc.env("OUT_DIR", &out_dir);
        let snapshot = artifact_cache_action_inputs_snapshot(&rustc, temp.path(), None).unwrap();
        let mut witness = snapshot.witness;
        assert!(witness.is_current());

        std::fs::create_dir(&out_dir).unwrap();
        std::fs::write(out_dir.join("generated.rs"), b"contents").unwrap();

        assert!(!witness.is_current());
        assert!(
            !artifact_cache_action_inputs_are_current(
                &rustc,
                temp.path(),
                &snapshot.digest,
                &mut witness,
                None,
            )
            .unwrap()
        );
    }

    #[test]
    #[cfg(unix)]
    fn action_input_witness_rejects_symlinked_inputs() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("dependency.rlib");
        let alias = temp.path().join("dependency-alias.rlib");
        std::fs::write(&input, b"contents").unwrap();
        symlink(&input, &alias).unwrap();
        let mut rustc = ProcessBuilder::new("rustc");
        rustc
            .arg("--extern")
            .arg(format!("dependency={}", alias.display()));

        assert!(artifact_cache_action_inputs_snapshot(&rustc, temp.path(), None).is_err());
    }
}

const APPLE_DEPLOYMENT_TARGET_ENVIRONMENT: [&str; 5] = [
    "MACOSX_DEPLOYMENT_TARGET",
    "IPHONEOS_DEPLOYMENT_TARGET",
    "WATCHOS_DEPLOYMENT_TARGET",
    "TVOS_DEPLOYMENT_TARGET",
    "XROS_DEPLOYMENT_TARGET",
];

fn resolve_rustc_input_path(path: &Path, rustc_cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        rustc_cwd.join(path)
    }
}

fn artifact_cache_action_inputs_are_materialized(rustc: &ProcessBuilder, rustc_cwd: &Path) -> bool {
    let args = rustc.get_args().collect::<Vec<_>>();
    let externs_exist = args.windows(2).all(|pair| {
        if pair[0] != OsStr::new("--extern") {
            return true;
        }
        let value = pair[1].to_string_lossy();
        let Some((_, path)) = value.split_once('=') else {
            return false;
        };
        resolve_rustc_input_path(Path::new(path), rustc_cwd).is_file()
    });
    let generated_inputs_exist = rustc.get_envs().iter().all(|(key, value)| {
        if key != "OUT_DIR" && !key.ends_with("_OUT_DIR") {
            return true;
        }
        value
            .as_ref()
            .is_none_or(|value| resolve_rustc_input_path(Path::new(value), rustc_cwd).is_dir())
    });
    externs_exist && generated_inputs_exist
}

fn artifact_cache_command_digest(rustc: &ProcessBuilder) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-command-v1\0");
    hasher.update(rustc.get_program().as_encoded_bytes());
    hasher.update(b"\0cwd\0");
    if let Some(cwd) = rustc.get_cwd() {
        hasher.update(cwd.as_os_str().as_encoded_bytes());
    }
    hasher.update(b"\0args\0");
    for arg in rustc.get_args() {
        hasher.update(arg.as_encoded_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"env\0");
    let mut envs = rustc.get_envs().iter().collect::<Vec<_>>();
    envs.sort_by_key(|(key, _)| *key);
    for (key, value) in envs {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        if let Some(value) = value {
            hasher.update(value.as_encoded_bytes());
        }
        hasher.update(b"\0");
    }
    hasher.finalize()
}

impl ArtifactCacheActionInputPathWitness {
    fn from_metadata(
        path: &Path,
        kind: ArtifactCacheActionInputPathKind,
        metadata: &fs::Metadata,
    ) -> CargoResult<Self> {
        if !(matches!(kind, ArtifactCacheActionInputPathKind::File)
            && metadata.file_type().is_file()
            || matches!(kind, ArtifactCacheActionInputPathKind::Directory)
                && metadata.file_type().is_dir())
        {
            anyhow::bail!(
                "artifact cache action input has an unsupported file type {}",
                path.display()
            );
        }
        Ok(Self {
            path: path.to_path_buf(),
            kind,
            metadata: Some(ArtifactCacheActionInputMetadataWitness {
                len: metadata.len(),
                modified: metadata.modified()?,
                #[cfg(unix)]
                device: std::os::unix::fs::MetadataExt::dev(metadata),
                #[cfg(unix)]
                inode: std::os::unix::fs::MetadataExt::ino(metadata),
                #[cfg(unix)]
                changed_seconds: std::os::unix::fs::MetadataExt::ctime(metadata),
                #[cfg(unix)]
                changed_nanoseconds: std::os::unix::fs::MetadataExt::ctime_nsec(metadata),
            }),
        })
    }

    fn capture(path: &Path, kind: ArtifactCacheActionInputPathKind) -> CargoResult<Self> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => Self::from_metadata(path, kind, &metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self {
                path: path.to_path_buf(),
                kind,
                metadata: None,
            }),
            Err(error) => Err(error.into()),
        }
    }

    fn is_current(&self) -> bool {
        Self::capture(&self.path, self.kind).is_ok_and(|current| current == *self)
    }
}

impl ArtifactCacheActionInputWitness {
    fn new(mut paths: Vec<ArtifactCacheActionInputPathWitness>, supports_fast_path: bool) -> Self {
        paths.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| (left.kind as u8).cmp(&(right.kind as u8)))
        });
        paths.dedup();
        Self {
            paths,
            supports_fast_path,
        }
    }

    fn is_current(&self) -> bool {
        self.paths
            .iter()
            .all(ArtifactCacheActionInputPathWitness::is_current)
    }
}

fn hash_artifact_cache_action_input_file(
    path: &Path,
    witnesses: &mut Vec<ArtifactCacheActionInputPathWitness>,
) -> CargoResult<(Option<Vec<u8>>, bool)> {
    let before =
        ArtifactCacheActionInputPathWitness::capture(path, ArtifactCacheActionInputPathKind::File)?;
    let Some(_) = before.metadata else {
        witnesses.push(before);
        return Ok((None, false));
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let supports_fast_path = artifact_cache_action_input_witness_is_reliable(&file);
    let handle_before = ArtifactCacheActionInputPathWitness::from_metadata(
        path,
        ArtifactCacheActionInputPathKind::File,
        &file.metadata()?,
    )?;
    if before != handle_before {
        anyhow::bail!(
            "artifact cache action input changed while opening {}",
            path.display()
        );
    }
    let mut bytes = Vec::with_capacity(
        before
            .metadata
            .as_ref()
            .and_then(|metadata| usize::try_from(metadata.len).ok())
            .unwrap_or(0),
    );
    file.read_to_end(&mut bytes)?;
    let handle_after = ArtifactCacheActionInputPathWitness::from_metadata(
        path,
        ArtifactCacheActionInputPathKind::File,
        &file.metadata()?,
    )?;
    let after =
        ArtifactCacheActionInputPathWitness::capture(path, ArtifactCacheActionInputPathKind::File)?;
    if before != handle_after || before != after {
        anyhow::bail!(
            "artifact cache action input changed while hashing {}",
            path.display()
        );
    }
    witnesses.push(after);
    Ok((Some(bytes), supports_fast_path))
}

#[cfg(target_vendor = "apple")]
fn artifact_cache_action_input_witness_is_reliable(file: &File) -> bool {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd as _;

    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `filesystem` points to writable storage for one `statfs` and is
    // only assumed initialized after `fstatfs` reports success.
    if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: `fstatfs` initialized the structure.
    let filesystem = unsafe { filesystem.assume_init() };
    // SAFETY: Darwin guarantees that `f_fstypename` is NUL-terminated.
    let name = unsafe { CStr::from_ptr(filesystem.f_fstypename.as_ptr()) };
    name.to_bytes() == b"apfs"
}

#[cfg(not(target_vendor = "apple"))]
fn artifact_cache_action_input_witness_is_reliable(_file: &File) -> bool {
    false
}

fn artifact_cache_action_inputs_snapshot(
    rustc: &ProcessBuilder,
    rustc_cwd: &Path,
    stats: Option<&artifact_cache_stats::ArtifactCacheStats>,
) -> CargoResult<ArtifactCacheActionInputSnapshot> {
    let started = stats.map(|_| Instant::now());
    let result = (|| {
        let args = rustc.get_args().collect::<Vec<_>>();
        let mut hasher = blake3::Hasher::new();
        let mut witnesses = Vec::new();
        let mut supports_fast_path = true;
        hasher.update(b"cargo-artifact-cache-action-inputs-v1\0");
        for pair in args.windows(2) {
            if pair[0] != OsStr::new("--extern") {
                continue;
            }
            let value = pair[1].to_string_lossy();
            let Some((_, path)) = value.split_once('=') else {
                continue;
            };
            let path = resolve_rustc_input_path(Path::new(path), rustc_cwd);
            let (bytes, reliable_witness) =
                hash_artifact_cache_action_input_file(&path, &mut witnesses)?;
            supports_fast_path &= reliable_witness;
            if let Some(bytes) = bytes {
                hasher.update(b"extern-content\0");
                hasher.update(&bytes);
                hasher.update(b"\0");
            }
        }
        for (key, value) in rustc.get_envs() {
            if key != "OUT_DIR" && !key.ends_with("_OUT_DIR") {
                continue;
            }
            let Some(value) = value else {
                continue;
            };
            let path = resolve_rustc_input_path(Path::new(value), rustc_cwd);
            let directory = ArtifactCacheActionInputPathWitness::capture(
                &path,
                ArtifactCacheActionInputPathKind::Directory,
            )?;
            if directory.metadata.is_some() {
                supports_fast_path = false;
                hasher.update(b"generated-input-tree\0");
                hasher.update(key.as_bytes());
                hasher.update(b"\0");
                hash_path_tree(
                    &mut hasher,
                    &path,
                    &path,
                    None,
                    false,
                    Some(directory),
                    &mut witnesses,
                )?;
            } else {
                witnesses.push(directory);
            }
        }
        for pair in args.windows(2) {
            if pair[0] != OsStr::new("-L") {
                continue;
            }
            let value = pair[1].to_string_lossy();
            if value.starts_with("dependency=") || value.starts_with("crate=") {
                continue;
            }
            let path = value
                .split_once('=')
                .map_or(value.as_ref(), |(_, path)| path);
            let path = resolve_rustc_input_path(Path::new(path), rustc_cwd);
            let directory = ArtifactCacheActionInputPathWitness::capture(
                &path,
                ArtifactCacheActionInputPathKind::Directory,
            )?;
            if directory.metadata.is_some() {
                supports_fast_path = false;
                hasher.update(b"link-search-input-tree\0");
                hash_path_tree(
                    &mut hasher,
                    &path,
                    &path,
                    None,
                    true,
                    Some(directory),
                    &mut witnesses,
                )?;
            } else {
                witnesses.push(directory);
            }
        }
        Ok(ArtifactCacheActionInputSnapshot {
            digest: hasher.finalize(),
            witness: ArtifactCacheActionInputWitness::new(witnesses, supports_fast_path),
        })
    })();
    if let Some(started) = started
        && let Some(stats) = stats
    {
        stats.action_hash(started.elapsed(), result.is_err());
    }
    result
}

fn artifact_cache_action_inputs_are_current(
    rustc: &ProcessBuilder,
    rustc_cwd: &Path,
    expected_digest: &blake3::Hash,
    witness: &mut ArtifactCacheActionInputWitness,
    stats: Option<&artifact_cache_stats::ArtifactCacheStats>,
) -> CargoResult<bool> {
    let started = stats.map(|_| Instant::now());
    let witness_is_current = witness.supports_fast_path && witness.is_current();
    if witness_is_current {
        if let Some(started) = started
            && let Some(stats) = stats
        {
            stats.action_witness_fast_path(started.elapsed());
        }
        return Ok(true);
    }
    if let Some(started) = started
        && let Some(stats) = stats
    {
        stats.action_witness_fallback(started.elapsed());
    }
    let current = artifact_cache_action_inputs_snapshot(rustc, rustc_cwd, stats)?;
    if current.digest != *expected_digest {
        return Ok(false);
    }
    *witness = current.witness;
    Ok(true)
}

fn artifact_cache_host_is_supported() -> bool {
    cfg!(target_os = "linux") || cfg!(target_os = "macos")
}

#[derive(Clone, Debug)]
struct CompilerLoaderInput {
    source: OsString,
    raw_path: PathBuf,
    path: PathBuf,
    location: CompilerLoaderInputLocation,
}

#[derive(Clone, Debug)]
enum CompilerLoaderInputLocation {
    Absolute,
    CompilerSysroot { root: PathBuf, relative: PathBuf },
    TargetProfile { root: PathBuf, relative: PathBuf },
    BuildDir { root: PathBuf, relative: PathBuf },
    RustcCwd { root: PathBuf, relative: PathBuf },
}

fn canonical_relative(path: &Path, root: &Path) -> Option<PathBuf> {
    let path = std::fs::canonicalize(path).ok()?;
    let root = std::fs::canonicalize(root).ok()?;
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}

fn compiler_loader_input(
    source: &str,
    raw_path: PathBuf,
    path: PathBuf,
    compiler_sysroot: Option<&Path>,
    target_profile_root: &Path,
    build_dir: &Path,
    rustc_cwd: &Path,
) -> CompilerLoaderInput {
    let location = (!raw_path.is_absolute())
        .then(|| {
            canonical_relative(&path, rustc_cwd).map(|relative| {
                CompilerLoaderInputLocation::RustcCwd {
                    root: rustc_cwd.to_path_buf(),
                    relative,
                }
            })
        })
        .flatten()
        .or_else(|| {
            compiler_sysroot.and_then(|root| {
                canonical_relative(&path, root).map(|relative| {
                    CompilerLoaderInputLocation::CompilerSysroot {
                        root: root.to_path_buf(),
                        relative,
                    }
                })
            })
        })
        .or_else(|| {
            canonical_relative(&path, target_profile_root).map(|relative| {
                CompilerLoaderInputLocation::TargetProfile {
                    root: target_profile_root.to_path_buf(),
                    relative,
                }
            })
        })
        .or_else(|| {
            canonical_relative(&path, build_dir).map(|relative| {
                CompilerLoaderInputLocation::BuildDir {
                    root: build_dir.to_path_buf(),
                    relative,
                }
            })
        })
        .unwrap_or(CompilerLoaderInputLocation::Absolute);
    CompilerLoaderInput {
        source: OsString::from(source),
        raw_path,
        path,
        location,
    }
}

fn artifact_cache_loader_environment_is_modeled(
    gctx: &crate::util::GlobalContext,
    rustc: &ProcessBuilder,
) -> bool {
    #[expect(
        clippy::disallowed_methods,
        reason = "loader admission must preserve non-UTF-8 inherited values filtered from GlobalContext::env"
    )]
    let inherited_is_modeled = std::env::vars_os().all(|(key, value)| {
        key.to_str()
            .is_some_and(|key| artifact_cache_loader_variable_is_modeled(key, &value))
    });
    inherited_is_modeled
        && gctx
            .env()
            .all(|(key, value)| artifact_cache_loader_variable_is_modeled(key, OsStr::new(value)))
        && artifact_cache_rustc_loader_environment_is_modeled(rustc)
}

fn artifact_cache_loader_variable_is_modeled(key: &str, value: &OsStr) -> bool {
    let bytes = value.as_encoded_bytes();
    let has_loader_expansion = (cfg!(target_os = "linux") && bytes.contains(&b'$'))
        || (cfg!(target_os = "macos") && (bytes.contains(&b'@') || bytes.contains(&b'$')));
    if value.is_empty()
        || (key == paths::dylib_path_envvar() && !has_loader_expansion)
        || (cfg!(target_os = "macos")
            && (key == "DYLD_LIBRARY_PATH" || key == "LD_LIBRARY_PATH")
            && !has_loader_expansion)
    {
        return true;
    }
    !(cfg!(unix) && key.starts_with("LD_"))
        && !(cfg!(target_os = "linux") && key == "GLIBC_TUNABLES")
        && !(cfg!(target_os = "macos") && key.starts_with("DYLD_"))
}

fn artifact_cache_rustc_loader_environment_is_modeled(rustc: &ProcessBuilder) -> bool {
    rustc.get_envs().iter().all(|(key, value)| {
        value
            .as_deref()
            .is_none_or(|value| artifact_cache_loader_variable_is_modeled(key, value))
    })
}

fn compiler_loader_input_paths(
    home: Option<&OsStr>,
    rustc: &ProcessBuilder,
    compiler_program: &Path,
    build_dir: &Path,
    output_root: &Path,
    rustc_cwd: &Path,
) -> Vec<CompilerLoaderInput> {
    fn resolve_path(path: PathBuf, rustc_cwd: &Path) -> PathBuf {
        if path.as_os_str().is_empty() {
            rustc_cwd.to_path_buf()
        } else if path.is_absolute() {
            path
        } else {
            rustc_cwd.join(path)
        }
    }

    fn extend_paths(
        inputs: &mut Vec<CompilerLoaderInput>,
        key: &str,
        value: &OsStr,
        compiler_sysroot: Option<&Path>,
        target_profile_root: &Path,
        build_dir: &Path,
        rustc_cwd: &Path,
    ) {
        inputs.extend(env::split_paths(value).map(|raw_path| {
            let path = resolve_path(raw_path.clone(), rustc_cwd);
            compiler_loader_input(
                key,
                raw_path,
                path,
                compiler_sysroot,
                target_profile_root,
                build_dir,
                rustc_cwd,
            )
        }));
    }

    let mut inputs = Vec::new();
    let compiler_sysroot = compiler_program.parent().and_then(Path::parent);
    let target_profile_root = output_root.parent().unwrap_or(output_root);
    let primary = paths::dylib_path_envvar();
    if let Some(value) = rustc.get_env(primary).filter(|value| !value.is_empty()) {
        extend_paths(
            &mut inputs,
            primary,
            &value,
            compiler_sysroot,
            target_profile_root,
            build_dir,
            rustc_cwd,
        );
    } else if cfg!(target_os = "macos") {
        if let Some(home) = home {
            let home_lib = PathBuf::from(home).join("lib");
            inputs.push(compiler_loader_input(
                primary,
                home_lib.clone(),
                resolve_path(home_lib, rustc_cwd),
                compiler_sysroot,
                target_profile_root,
                build_dir,
                rustc_cwd,
            ));
        }
        inputs.push(compiler_loader_input(
            primary,
            PathBuf::from("/usr/local/lib"),
            PathBuf::from("/usr/local/lib"),
            compiler_sysroot,
            target_profile_root,
            build_dir,
            rustc_cwd,
        ));
        inputs.push(compiler_loader_input(
            primary,
            PathBuf::from("/usr/lib"),
            PathBuf::from("/usr/lib"),
            compiler_sysroot,
            target_profile_root,
            build_dir,
            rustc_cwd,
        ));
    }
    if cfg!(target_os = "macos") {
        for key in ["DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"] {
            if let Some(value) = rustc.get_env(key).filter(|value| !value.is_empty()) {
                extend_paths(
                    &mut inputs,
                    key,
                    &value,
                    compiler_sysroot,
                    target_profile_root,
                    build_dir,
                    rustc_cwd,
                );
            }
        }
        inputs.push(CompilerLoaderInput {
            source: OsString::from(paths::dylib_path_envvar()),
            raw_path: PathBuf::new(),
            path: rustc_cwd.to_path_buf(),
            location: CompilerLoaderInputLocation::RustcCwd {
                root: rustc_cwd.to_path_buf(),
                relative: PathBuf::new(),
            },
        });
    }
    inputs
}

fn remove_cargo_injected_loader_path(
    rustc: &mut ProcessBuilder,
    output_root: &Path,
) -> CargoResult<bool> {
    let variable = paths::dylib_path_envvar();
    let Some(value) = rustc.get_env(variable) else {
        return Ok(false);
    };
    let mut search_paths = env::split_paths(&value).collect::<Vec<_>>();
    let original_len = search_paths.len();
    search_paths.retain(|path| path != output_root);
    if search_paths.len() == original_len {
        return Ok(false);
    }

    // `rlib_action_is_cacheable_with_search_paths` rejects proc macros and
    // every other target-local dynamic-library input before this is called.
    // An ordinary `--crate-type lib` action neither loads target artifacts nor
    // invokes the linker, so retaining Cargo's target-local loader path only
    // lets unrelated proc-macro dylibs perturb the key while the build runs.
    rustc.env(variable, paths::join_paths(&search_paths, variable)?);
    Ok(true)
}

fn compiler_loader_inputs_digest(
    loader_input_paths: &[CompilerLoaderInput],
    identity_witness: Option<&crate::util::rustc::ArtifactCacheIdentityWitness>,
) -> CargoResult<blake3::Hash> {
    // Bind Linux nested-library admission to key creation and publication as
    // well as the early eligibility check, since loader trees can change.
    if !artifact_cache_loader_input_paths_are_modeled(loader_input_paths) {
        anyhow::bail!("Linux compiler loader roots contain nested dynamic libraries");
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-compiler-loader-inputs-v2\0");
    for input in loader_input_paths {
        update_framed_loader_digest(&mut hasher, input.source.as_encoded_bytes());
        let mut hash_contents = true;
        match &input.location {
            CompilerLoaderInputLocation::CompilerSysroot { root, relative } => {
                validate_portable_loader_location(input, root, relative)?;
                update_framed_loader_digest(
                    &mut hasher,
                    b"/__cargo_artifact_cache_compiler_sysroot",
                );
                update_framed_loader_digest(&mut hasher, relative.as_os_str().as_encoded_bytes());
                hash_contents = identity_witness.is_none_or(|witness| {
                    let statically_covered = compiler_identity_statically_covers_loader_input(
                        witness, input, root, relative,
                    );
                    !statically_covered
                        && !compiler_identity_covers_loader_input(witness, &input.path)
                });
            }
            CompilerLoaderInputLocation::TargetProfile { root, relative } => {
                validate_portable_loader_location(input, root, relative)?;
                update_framed_loader_digest(&mut hasher, b"/__cargo_artifact_cache_target_profile");
                update_framed_loader_digest(&mut hasher, relative.as_os_str().as_encoded_bytes());
            }
            CompilerLoaderInputLocation::BuildDir { root, relative } => {
                validate_portable_loader_location(input, root, relative)?;
                update_framed_loader_digest(&mut hasher, b"/__cargo_artifact_cache_build_dir");
                update_framed_loader_digest(&mut hasher, relative.as_os_str().as_encoded_bytes());
            }
            CompilerLoaderInputLocation::RustcCwd { root, relative } => {
                validate_portable_loader_location(input, root, relative)?;
                update_framed_loader_digest(
                    &mut hasher,
                    b"/__cargo_artifact_cache_rustc_cwd_loader_root",
                );
                update_framed_loader_digest(&mut hasher, relative.as_os_str().as_encoded_bytes());
            }
            CompilerLoaderInputLocation::Absolute => {
                update_framed_loader_digest(&mut hasher, input.path.as_os_str().as_encoded_bytes());
            }
        }
        if hash_contents {
            hash_dynamic_library_inputs(
                &mut hasher,
                &input.path,
                !matches!(&input.location, CompilerLoaderInputLocation::Absolute),
            )?;
        }
    }
    Ok(hasher.finalize())
}

fn compiler_identity_statically_covers_loader_input(
    witness: &crate::util::rustc::ArtifactCacheIdentityWitness,
    input: &CompilerLoaderInput,
    root: &Path,
    relative: &Path,
) -> bool {
    // The macOS loader searches one directory level. Compiler identity hashes
    // every file directly under `lib`, while its directory witness detects
    // additions and removals. The final restore and every publication boundary
    // validate that witness, so rescanning that root for every unit cannot
    // discover new information. `bin` remains dynamic because compiler identity
    // selects runtime libraries by name while loader admission also recognizes
    // extensionless binaries by magic.
    // Linux loader modeling is recursive and retains the conservative scan.
    if !cfg!(target_os = "macos") || relative != Path::new("lib") {
        return false;
    }
    let Ok(actual) = fs::canonicalize(&input.path) else {
        return false;
    };
    let Ok(expected) = fs::canonicalize(root.join(relative)) else {
        return false;
    };
    actual == expected && witness.contains_canonical_directory(&actual)
}

fn compiler_identity_covers_loader_input(
    witness: &crate::util::rustc::ArtifactCacheIdentityWitness,
    path: &Path,
) -> bool {
    fn visit(
        witness: &crate::util::rustc::ArtifactCacheIdentityWitness,
        path: &Path,
        recurse: bool,
    ) -> CargoResult<bool> {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                return Ok(false);
            }
            if recurse && file_type.is_dir() && !visit(witness, &path, true)? {
                return Ok(false);
            }
            if file_type.is_file()
                && is_potential_dynamic_library(&path)?
                && !witness.contains_file(&path)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    visit(witness, path, cfg!(target_os = "linux")).unwrap_or(false)
}

fn validate_portable_loader_location(
    input: &CompilerLoaderInput,
    root: &Path,
    expected_relative: &Path,
) -> CargoResult<()> {
    let current_relative = canonical_relative(&input.path, root).ok_or_else(|| {
        internal(format!(
            "compiler loader input escaped its modeled root: {}",
            input.path.display()
        ))
    })?;
    if current_relative != expected_relative {
        anyhow::bail!(
            "compiler loader input changed location: {}",
            input.path.display()
        );
    }
    Ok(())
}

fn update_framed_loader_digest(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn dynamic_library_file_digest(path: &Path) -> CargoResult<blake3::Hash> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut file)?;
    Ok(hasher.finalize())
}

fn is_potential_dynamic_library(path: &Path) -> CargoResult<bool> {
    if path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.ends_with(".dylib") || name.ends_with(".dll") || name.contains(".so")
    }) {
        return Ok(true);
    }
    let mut file = File::open(path)?;
    let mut header = [0; 20];
    let read = std::io::Read::read(&mut file, &mut header)?;
    let header = &header[..read];
    if header.starts_with(b"\x7fELF") {
        let Some(kind) = header.get(16..18) else {
            return Ok(true);
        };
        let kind = match header.get(5) {
            Some(1) => u16::from_le_bytes(kind.try_into().unwrap()),
            Some(2) => u16::from_be_bytes(kind.try_into().unwrap()),
            _ => return Ok(true),
        };
        return Ok(kind == 3);
    }
    let macho_file_type = match header.get(..4) {
        Some([0xfe, 0xed, 0xfa, 0xce] | [0xfe, 0xed, 0xfa, 0xcf]) => header
            .get(12..16)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap())),
        Some([0xce, 0xfa, 0xed, 0xfe] | [0xcf, 0xfa, 0xed, 0xfe]) => header
            .get(12..16)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap())),
        Some(
            [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca],
        ) => return Ok(true),
        _ => None,
    };
    Ok(macho_file_type.is_some_and(|kind| kind == 6 || kind == 8) || header.starts_with(b"MZ"))
}

fn artifact_cache_loader_input_paths_are_modeled(
    loader_input_paths: &[CompilerLoaderInput],
) -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }

    fn has_nested_dynamic_library(
        path: &Path,
        nested: bool,
        visited: &mut HashSet<PathBuf>,
    ) -> CargoResult<bool> {
        let canonical = match fs::canonicalize(path) {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if !visited.insert(canonical) {
            return Ok(false);
        }
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let path = entry.path();
            if path.is_dir() && has_nested_dynamic_library(&path, true, visited)? {
                return Ok(true);
            }
            if nested && path.is_file() && is_potential_dynamic_library(&path)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    loader_input_paths.iter().all(|input| {
        !has_nested_dynamic_library(&input.path, false, &mut HashSet::new()).unwrap_or(true)
    })
}

fn hash_dynamic_library_inputs(
    hasher: &mut blake3::Hasher,
    path: &Path,
    normalize_directory_locations: bool,
) -> CargoResult<()> {
    fn hash_directory(
        hasher: &mut blake3::Hasher,
        root: &Path,
        path: &Path,
        recurse: bool,
        normalize_directory_locations: bool,
        visited: &mut HashSet<PathBuf>,
    ) -> CargoResult<()> {
        let canonical = match fs::canonicalize(path) {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        update_framed_loader_digest(hasher, b"compiler-loader-search-directory");
        update_framed_loader_digest(
            hasher,
            path.strip_prefix(root)
                .unwrap_or(path)
                .as_os_str()
                .as_encoded_bytes(),
        );
        if normalize_directory_locations {
            update_framed_loader_digest(hasher, b"/__cargo_artifact_cache_compiler_loader_root");
            update_framed_loader_digest(
                hasher,
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .as_os_str()
                    .as_encoded_bytes(),
            );
        } else {
            update_framed_loader_digest(hasher, canonical.as_os_str().as_encoded_bytes());
        }
        if recurse && !visited.insert(canonical) {
            return Ok(());
        }
        let mut entries = match fs::read_dir(path) {
            Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            if entry.file_type()?.is_symlink() {
                update_framed_loader_digest(hasher, b"compiler-loader-search-symlink");
                update_framed_loader_digest(
                    hasher,
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .as_os_str()
                        .as_encoded_bytes(),
                );
                update_framed_loader_digest(
                    hasher,
                    fs::read_link(&path)?.as_os_str().as_encoded_bytes(),
                );
            }
            if recurse && path.is_dir() {
                hash_directory(
                    hasher,
                    root,
                    &path,
                    recurse,
                    normalize_directory_locations,
                    visited,
                )?;
                continue;
            }
            if !path.is_file() || !is_potential_dynamic_library(&path)? {
                continue;
            }
            update_framed_loader_digest(hasher, b"compiler-loader-search-input");
            update_framed_loader_digest(
                hasher,
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .as_os_str()
                    .as_encoded_bytes(),
            );
            hasher.update(dynamic_library_file_digest(&path)?.as_bytes());
        }
        Ok(())
    }

    hash_directory(
        hasher,
        path,
        path,
        cfg!(target_os = "linux"),
        normalize_directory_locations,
        &mut HashSet::new(),
    )
}

fn hash_path_tree(
    hasher: &mut blake3::Hasher,
    root: &Path,
    path: &Path,
    excluded_path: Option<&Path>,
    link_search_input: bool,
    directory_before: Option<ArtifactCacheActionInputPathWitness>,
    witnesses: &mut Vec<ArtifactCacheActionInputPathWitness>,
) -> CargoResult<()> {
    let directory_before = match directory_before {
        Some(directory_before) => directory_before,
        None => ArtifactCacheActionInputPathWitness::capture(
            path,
            ArtifactCacheActionInputPathKind::Directory,
        )?,
    };
    if directory_before.metadata.is_none() {
        anyhow::bail!(
            "artifact cache action input tree is not a directory {}",
            path.display()
        );
    }
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if excluded_path.is_some_and(|excluded| path.starts_with(excluded))
            || path.file_name() == Some(OsStr::new(".git"))
            || (link_search_input && path.extension() == Some(OsStr::new("dSYM")))
        {
            continue;
        }
        if file_type.is_symlink() {
            anyhow::bail!(
                "artifact cache action input tree contains symlink {}",
                path.display()
            );
        } else if file_type.is_dir() {
            hash_path_tree(
                hasher,
                root,
                &path,
                excluded_path,
                link_search_input,
                None,
                witnesses,
            )?;
        } else if file_type.is_file() {
            hasher.update(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .as_os_str()
                    .as_encoded_bytes(),
            );
            hasher.update(b"\0");
            let (bytes, _) = hash_artifact_cache_action_input_file(&path, witnesses)?;
            let Some(bytes) = bytes else {
                anyhow::bail!(
                    "artifact cache action input disappeared while hashing {}",
                    path.display()
                );
            };
            if link_search_input && path.extension() == Some(OsStr::new("a")) {
                hasher.update(&normalize_ar_timestamps(&bytes));
            } else {
                hasher.update(&bytes);
            }
            hasher.update(b"\0");
        } else {
            anyhow::bail!(
                "artifact cache action input tree contains unsupported node {}",
                path.display()
            );
        }
    }
    let directory_after = ArtifactCacheActionInputPathWitness::capture(
        path,
        ArtifactCacheActionInputPathKind::Directory,
    )?;
    if directory_before != directory_after {
        anyhow::bail!(
            "artifact cache action input directory changed while hashing {}",
            path.display()
        );
    }
    witnesses.push(directory_after);
    Ok(())
}

fn normalize_ar_timestamps(bytes: &[u8]) -> Cow<'_, [u8]> {
    const GLOBAL_HEADER: &[u8] = b"!<arch>\n";
    const MEMBER_HEADER_LEN: usize = 60;

    if !bytes.starts_with(GLOBAL_HEADER) {
        return Cow::Borrowed(bytes);
    }

    let mut normalized = bytes.to_vec();
    let mut offset = GLOBAL_HEADER.len();
    while offset < normalized.len() {
        let Some(header) = normalized.get_mut(offset..offset + MEMBER_HEADER_LEN) else {
            return Cow::Borrowed(bytes);
        };
        if &header[58..] != b"`\n" {
            return Cow::Borrowed(bytes);
        }
        let Some(size) = std::str::from_utf8(&header[48..58])
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
        else {
            return Cow::Borrowed(bytes);
        };
        header[16..28].fill(b'0');
        offset += MEMBER_HEADER_LEN + size + (size % 2);
    }
    if offset != normalized.len() {
        return Cow::Borrowed(bytes);
    }
    Cow::Owned(normalized)
}

fn restore_rlib_cache(
    entry_root: &Path,
    outputs: &[build_runner::OutputFile],
    rustc_dep_info_loc: &Path,
    message_cache_path: &Path,
    rustc: &ProcessBuilder,
    rustc_cwd: &Path,
    pkg_root: &Path,
    output_root: &Path,
    identity_witness: &crate::util::rustc::ArtifactCacheIdentityWitness,
    loader_input_paths: &[CompilerLoaderInput],
    loader_inputs_digest: &blake3::Hash,
    action_inputs_digest: &blake3::Hash,
    action_inputs_witness: &ArtifactCacheActionInputWitness,
    materialization: ArtifactCacheMaterialization,
    max_size: Option<u64>,
    stats: Option<&artifact_cache_stats::ArtifactCacheStats>,
    snapshot: Option<&artifact_cache_snapshot::Recorder>,
) -> CargoResult<bool> {
    let mut action_inputs_witness = action_inputs_witness.clone();
    if !path_is_directory_no_follow(entry_root) {
        return Ok(false);
    }
    let cache_root = entry_root.parent().unwrap_or(entry_root);
    let phase_started = stats.map(|_| Instant::now());
    let lock_result = try_read_lock_rlib_cache_within_limit(cache_root, max_size);
    if let Some(started) = phase_started
        && let Some(stats) = stats
    {
        stats.restore_phase(RestorePhase::Lock, started.elapsed());
    }
    let Some(lock) = lock_result? else {
        return Ok(false);
    };
    if !path_is_directory_no_follow(entry_root) {
        return Ok(false);
    }
    delay_rlib_cache_restore_for_tests()?;
    let mut entries = fs::read_dir(entry_root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    let mut corrupt_entries = Vec::new();
    for entry in entries {
        let entry = entry.path();
        if !path_is_directory_no_follow(&entry)
            || entry
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            || !path_is_regular_file(&entry.join("complete"))
        {
            continue;
        }
        let phase_started = stats.map(|_| Instant::now());
        let control_result = verify_rlib_cache_control_files(&entry);
        if let Some(started) = phase_started
            && let Some(stats) = stats
        {
            stats.restore_phase(RestorePhase::ControlValidation, started.elapsed());
        }
        match control_result {
            Ok(true) => {}
            Ok(false) => {
                debug!("rejecting corrupt artifact cache entry {}", entry.display());
                corrupt_entries.push(entry);
                continue;
            }
            Err(error) => {
                debug!(
                    "rejecting unreadable artifact cache entry {}: {error:#}",
                    entry.display()
                );
                corrupt_entries.push(entry);
                continue;
            }
        }
        let phase_started = stats.map(|_| Instant::now());
        let inputs_result = verify_rlib_cache_inputs(&entry, rustc, rustc_cwd);
        if let Some(started) = phase_started
            && let Some(stats) = stats
        {
            stats.restore_phase(RestorePhase::SourceValidation, started.elapsed());
        }
        match inputs_result {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                debug!(
                    "rejecting unreadable artifact cache inputs {}: {error:#}",
                    entry.display()
                );
                continue;
            }
        }
        let phase_started = stats.map(|_| Instant::now());
        let entry_result = verify_rlib_cache_entry(&entry, outputs);
        if let Some(started) = phase_started
            && let Some(stats) = stats
        {
            stats.restore_phase(RestorePhase::EntryValidation, started.elapsed());
        }
        match entry_result {
            Ok(true) => {}
            Ok(false) => {
                debug!("rejecting corrupt artifact cache entry {}", entry.display());
                corrupt_entries.push(entry);
                continue;
            }
            Err(error) => {
                debug!(
                    "rejecting unreadable artifact cache entry {}: {error:#}",
                    entry.display()
                );
                corrupt_entries.push(entry);
                continue;
            }
        }
        let stored_files = entry.join("files");
        let mut restored_files = 0;
        let mut restored_bytes = 0;
        let mut materialization_totals = MaterializationTotals::default();
        for output in outputs {
            let stored = stored_files.join(output.path.file_name().unwrap());
            let bytes = if stats.is_some() {
                fs::metadata(&stored)?.len()
            } else {
                0
            };
            let started = stats.map(|_| Instant::now());
            let materialization_result =
                materialize_rlib_cache_file(&stored, &output.path, materialization);
            if let Some(started) = started
                && let Some(stats) = stats
            {
                stats.materialization_finished(started.elapsed());
            }
            let materialization_kind = materialization_result?;
            materialization_totals.record(materialization_kind, bytes);
            restored_files += 1;
            restored_bytes += bytes;
        }
        delay_rlib_cache_restore_materialized_for_tests()?;
        let phase_started = stats.map(|_| Instant::now());
        let identity_started = stats.map(|_| Instant::now());
        let identity_is_current =
            restore_materialized_identity_witness_is_current(identity_witness);
        if let Some(started) = identity_started
            && let Some(stats) = stats
        {
            stats.restore_phase(RestorePhase::FinalIdentityValidation, started.elapsed());
        }
        let inputs_are_current: CargoResult<bool> = if !identity_is_current {
            Ok(false)
        } else {
            let loader_started = stats.map(|_| Instant::now());
            let loader_result =
                compiler_loader_inputs_digest(loader_input_paths, Some(identity_witness));
            if let Some(started) = loader_started
                && let Some(stats) = stats
            {
                stats.restore_phase(RestorePhase::FinalLoaderValidation, started.elapsed());
            }
            if loader_result? != *loader_inputs_digest {
                Ok(false)
            } else {
                let action_started = stats.map(|_| Instant::now());
                let action_result = artifact_cache_action_inputs_are_current(
                    rustc,
                    rustc_cwd,
                    action_inputs_digest,
                    &mut action_inputs_witness,
                    stats,
                );
                if let Some(started) = action_started
                    && let Some(stats) = stats
                {
                    stats.restore_phase(RestorePhase::FinalActionValidation, started.elapsed());
                }
                action_result
            }
        };
        if let Some(started) = phase_started
            && let Some(stats) = stats
        {
            stats.restore_phase(RestorePhase::FinalValidation, started.elapsed());
        }
        if !inputs_are_current? {
            debug!("not restoring artifact cache entry with inputs modified during restore");
            for output in outputs {
                if fs::symlink_metadata(&output.path).is_ok() {
                    paths::remove_file(&output.path)?;
                }
            }
            return Ok(false);
        }
        let phase_started = stats.map(|_| Instant::now());
        let state_write_result: CargoResult<()> = (|| {
            paths::copy(&entry.join("compiler-messages"), message_cache_path)?;
            let stored_dep_info = entry.join("rustc-dep-info");
            if path_is_regular_file(&stored_dep_info) {
                let origin_pkg_root = paths::read(&entry.join("origin-pkg-root"))?;
                let origin_target_profile_root =
                    paths::read(&entry.join("origin-target-profile-root"))?;
                let target_profile_root = output_root.parent().unwrap_or(output_root);
                let translated = paths::read(&stored_dep_info)?
                    .replace(origin_pkg_root.trim_end(), &pkg_root.to_string_lossy())
                    .replace(
                        origin_target_profile_root.trim_end(),
                        &target_profile_root.to_string_lossy(),
                    );
                paths::write(rustc_dep_info_loc, translated.as_bytes())?;
            }
            Ok(())
        })();
        if let Some(started) = phase_started
            && let Some(stats) = stats
        {
            stats.restore_phase(RestorePhase::StateWrite, started.elapsed());
        }
        state_write_result?;
        if let Some(stats) = stats {
            stats.restored(restored_files, restored_bytes, &materialization_totals);
        }
        if let Some(snapshot) = snapshot {
            snapshot.record(&entry, outputs);
        }
        drop(lock);
        cleanup_corrupt_rlib_cache_entries(cache_root, &corrupt_entries, outputs, max_size);
        return Ok(true);
    }
    drop(lock);
    cleanup_corrupt_rlib_cache_entries(cache_root, &corrupt_entries, outputs, max_size);
    Ok(false)
}

fn materialize_rlib_cache_file(
    stored: &Path,
    output: &Path,
    materialization: ArtifactCacheMaterialization,
) -> CargoResult<MaterializationKind> {
    match materialization {
        ArtifactCacheMaterialization::Hardlink => {
            #[cfg(unix)]
            {
                if fs::symlink_metadata(output).is_ok() {
                    paths::remove_file(output)?;
                }
                #[expect(
                    clippy::disallowed_methods,
                    reason = "test-only hook is intentionally outside user configuration"
                )]
                let hardlink_result =
                    if std::env::var_os(ARTIFACT_CACHE_CROSS_DEVICE_HARDLINK_FAILURE_FOR_TESTS)
                        .is_some()
                    {
                        Err(io::Error::from(io::ErrorKind::CrossesDevices))
                    } else {
                        fs::hard_link(stored, output)
                    };
                match hardlink_result {
                    Ok(()) => {
                        debug!(
                            "hardlinked cached artifact {} -> {}",
                            stored.display(),
                            output.display()
                        );
                        return Ok(MaterializationKind::Hardlink);
                    }
                    Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                        paths::copy(stored, output)?;
                        debug!(
                            "copied cached artifact across filesystems {} -> {}",
                            stored.display(),
                            output.display()
                        );
                        return Ok(MaterializationKind::CrossDeviceCopy);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            #[cfg(not(unix))]
            {
                paths::copy(stored, output)?;
                debug!(
                    "copied cached artifact on platform without protected hardlink rebuilds {} -> {}",
                    stored.display(),
                    output.display()
                );
                return Ok(MaterializationKind::Copy);
            }
        }
        ArtifactCacheMaterialization::Copy => {
            if fs::symlink_metadata(output).is_ok() {
                paths::remove_file(output)?;
            }
            paths::copy(stored, output)?;
            debug!(
                "copied cached artifact {} -> {}",
                stored.display(),
                output.display()
            );
            return Ok(MaterializationKind::Copy);
        }
    }
}

fn prepare_materialized_rlib_output_for_write(output: &Path) -> CargoResult<()> {
    let Ok(metadata) = fs::symlink_metadata(output) else {
        return Ok(());
    };
    if !metadata.file_type().is_file() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() > 1 {
            paths::remove_file(output)?;
            debug!(
                "detached hardlinked cached artifact before rebuild {}",
                output.display()
            );
        }
    }
    Ok(())
}

fn skip_artifact_cache_publication(reason: PublicationSkipReason) -> PublicationOutcome {
    PublicationOutcome::Skipped(reason)
}

fn store_rlib_cache(
    entry_root: &Path,
    outputs: &[build_runner::OutputFile],
    rustc_dep_info_loc: &Path,
    message_cache_path: &Path,
    invocation_time: filetime::FileTime,
    rustc_cwd: &Path,
    pkg_root: &Path,
    build_dir: &Path,
    output_root: &Path,
    identity_witness: &crate::util::rustc::ArtifactCacheIdentityWitness,
    loader_input_paths: &[CompilerLoaderInput],
    loader_inputs_digest: &blake3::Hash,
    action_inputs_digest: &blake3::Hash,
    action_inputs_witness: &ArtifactCacheActionInputWitness,
    rustc: &ProcessBuilder,
    max_size: Option<u64>,
    stats: Option<&artifact_cache_stats::ArtifactCacheStats>,
    snapshot: Option<&artifact_cache_snapshot::Recorder>,
) -> CargoResult<PublicationOutcome> {
    let mut action_inputs_witness = action_inputs_witness.clone();
    match rlib_cache_input_support(rustc_dep_info_loc, rustc_cwd, build_dir, output_root)? {
        RlibCacheInputSupport::Supported => {}
        RlibCacheInputSupport::MissingDepInfo => {
            debug!("not storing artifact cache entry without rustc dep-info");
            return Ok(skip_artifact_cache_publication(
                PublicationSkipReason::SourceInputsUnavailable,
            ));
        }
        RlibCacheInputSupport::GeneratedBuildInput => {
            debug!("not storing artifact cache entry with generated build-directory inputs");
            return Ok(skip_artifact_cache_publication(
                PublicationSkipReason::GeneratedBuildInput,
            ));
        }
    }
    delay_rlib_cache_input_digest_for_tests()?;
    if !identity_witness.is_current() {
        debug!(
            "not storing artifact cache entry with compiler identity modified during compilation"
        );
        return Ok(skip_artifact_cache_publication(
            PublicationSkipReason::CompilerIdentityChanged,
        ));
    }
    if compiler_loader_inputs_digest(loader_input_paths, Some(identity_witness))?
        != *loader_inputs_digest
    {
        debug!(
            "not storing artifact cache entry with compiler loader inputs modified during compilation"
        );
        return Ok(skip_artifact_cache_publication(
            PublicationSkipReason::LoaderInputsChanged,
        ));
    }
    if !artifact_cache_action_inputs_are_current(
        rustc,
        rustc_cwd,
        action_inputs_digest,
        &mut action_inputs_witness,
        stats,
    )? {
        debug!("not storing artifact cache entry with action inputs modified during compilation");
        return Ok(skip_artifact_cache_publication(
            PublicationSkipReason::ActionInputsChanged,
        ));
    }
    let Some(inputs_digest) = rlib_cache_inputs_digest_matching_rustc_dep_info(
        rustc_dep_info_loc,
        rustc_cwd,
        invocation_time,
    )?
    else {
        debug!("not storing artifact cache entry with unreadable compiler-discovered inputs");
        return Ok(skip_artifact_cache_publication(
            PublicationSkipReason::SourceInputsUnavailable,
        ));
    };
    let cache_root = entry_root.parent().unwrap_or(entry_root);
    paths::create_dir_all(cache_root)?;
    let Some(_action_lock) = try_write_lock_rlib_cache_action(entry_root)? else {
        return Ok(skip_artifact_cache_publication(
            PublicationSkipReason::ActionLockUnavailable,
        ));
    };
    if !create_rlib_cache_directory_no_follow(entry_root)? {
        debug!(
            "not storing artifact cache entry under unsupported action root {}",
            entry_root.display()
        );
        return Ok(skip_artifact_cache_publication(
            PublicationSkipReason::UnsupportedActionRoot,
        ));
    }
    cleanup_abandoned_rlib_cache_action_transients(entry_root);
    let entry = entry_root.join(&inputs_digest);
    let staging = staging_rlib_cache_entry(&entry)?;
    let result = (|| -> CargoResult<PublicationOutcome> {
        let files = staging.join("files");
        paths::create_dir_all(&files)?;
        #[expect(
            clippy::disallowed_methods,
            reason = "test-only hook is intentionally outside user configuration"
        )]
        if std::env::var_os(ARTIFACT_CACHE_STORE_FAILURE_AFTER_STAGING_FOR_TESTS).is_some() {
            return Err(internal(
                "test-only artifact cache failure after staging".to_string(),
            ));
        }
        let mut manifest = String::new();
        let mut entry_files = 0;
        for output in outputs {
            if output.path.exists() {
                let name = output.path.file_name().ok_or_else(|| {
                    internal(format!(
                        "artifact cache output has no filename: {}",
                        output.path.display()
                    ))
                })?;
                let stored = files.join(name);
                paths::copy(&output.path, &stored)?;
                append_rlib_cache_manifest(&mut manifest, &staging, &stored)?;
                entry_files += 1;
            }
        }
        if rustc_dep_info_loc.exists() {
            let stored = staging.join("rustc-dep-info");
            paths::copy(rustc_dep_info_loc, &stored)?;
            append_rlib_cache_manifest(&mut manifest, &staging, &stored)?;
            entry_files += 1;
        }
        let stored_messages = staging.join("compiler-messages");
        if message_cache_path.exists() {
            paths::copy(message_cache_path, &stored_messages)?;
        } else {
            paths::write(&stored_messages, b"")?;
        }
        append_rlib_cache_manifest(&mut manifest, &staging, &stored_messages)?;
        entry_files += 1;
        paths::write(
            staging.join("inputs.blake3"),
            format!("{inputs_digest}\n").as_bytes(),
        )?;
        append_rlib_cache_manifest(&mut manifest, &staging, &staging.join("inputs.blake3"))?;
        entry_files += 1;
        paths::write(
            staging.join("origin-pkg-root"),
            pkg_root.to_string_lossy().as_bytes(),
        )?;
        append_rlib_cache_manifest(&mut manifest, &staging, &staging.join("origin-pkg-root"))?;
        entry_files += 1;
        paths::write(
            staging.join("origin-target-profile-root"),
            output_root
                .parent()
                .unwrap_or(output_root)
                .to_string_lossy()
                .as_bytes(),
        )?;
        append_rlib_cache_manifest(
            &mut manifest,
            &staging,
            &staging.join("origin-target-profile-root"),
        )?;
        entry_files += 1;
        let manifest_path = staging.join("manifest.blake3");
        paths::write(&manifest_path, manifest.as_bytes())?;
        entry_files += 1;
        let manifest_digest = rlib_cache_digest(&manifest_path)?;
        paths::write(
            staging.join("complete"),
            format!("{manifest_digest}\n").as_bytes(),
        )?;
        entry_files += 1;
        let entry_size = artifact_cache_entry_size(&staging)?;
        if let Some(max_size) = max_size
            && entry_size > max_size
        {
            paths::remove_dir_all(&staging)?;
            debug!(
                "not storing artifact cache entry larger than configured maximum: {} > {}",
                entry_size, max_size
            );
            return Ok(skip_artifact_cache_publication(
                PublicationSkipReason::EntryTooLarge,
            ));
        }
        delay_rlib_cache_publish_for_tests()?;
        if !identity_witness.is_current()
            || compiler_loader_inputs_digest(loader_input_paths, Some(identity_witness))?
                != *loader_inputs_digest
            || !artifact_cache_action_inputs_are_current(
                rustc,
                rustc_cwd,
                action_inputs_digest,
                &mut action_inputs_witness,
                stats,
            )?
            || rlib_cache_inputs_digest_matching_rustc_dep_info(
                rustc_dep_info_loc,
                rustc_cwd,
                invocation_time,
            )?
            .as_deref()
                != Some(&inputs_digest)
        {
            debug!("not storing artifact cache entry with inputs modified during staging");
            paths::remove_dir_all(&staging)?;
            return Ok(skip_artifact_cache_publication(
                PublicationSkipReason::InputsChangedDuringStaging,
            ));
        }
        let Some(_lock) = lock_rlib_cache_exclusive(cache_root, "publishing artifact cache entry")?
        else {
            paths::remove_dir_all(&staging)?;
            return Ok(skip_artifact_cache_publication(
                PublicationSkipReason::PublicationLockUnavailable,
            ));
        };
        delay_rlib_cache_publish_locked_for_tests()?;
        let mut cache_size = match recorded_rlib_cache_size(cache_root) {
            Some(size) if rlib_cache_size_within_limit(size, max_size) => size,
            Some(_) | None => reconcile_rlib_cache_size(cache_root, max_size, None)?,
        };
        if !identity_witness.is_current()
            || compiler_loader_inputs_digest(loader_input_paths, Some(identity_witness))?
                != *loader_inputs_digest
            || !artifact_cache_action_inputs_are_current(
                rustc,
                rustc_cwd,
                action_inputs_digest,
                &mut action_inputs_witness,
                stats,
            )?
            || rlib_cache_inputs_digest_matching_rustc_dep_info(
                rustc_dep_info_loc,
                rustc_cwd,
                invocation_time,
            )?
            .as_deref()
                != Some(&inputs_digest)
        {
            debug!("not storing artifact cache entry with inputs modified before publication");
            paths::remove_dir_all(&staging)?;
            return Ok(skip_artifact_cache_publication(
                PublicationSkipReason::InputsChangedBeforePublication,
            ));
        }
        if entry.exists() {
            if entry.join("complete").exists()
                && verify_rlib_cache_entry(&entry, outputs).unwrap_or(false)
                && artifact_cache_entry_size(&entry)
                    .is_ok_and(|size| rlib_cache_size_within_limit(size, max_size))
            {
                if let Some(snapshot) = snapshot {
                    snapshot.record(&entry, outputs);
                }
                paths::remove_dir_all(&staging)?;
                return Ok(skip_artifact_cache_publication(
                    PublicationSkipReason::AlreadyStored,
                ));
            }
            mark_rlib_cache_size_dirty(cache_root)?;
            quarantine_rlib_cache_entry(&entry)?;
            cache_size = reconcile_rlib_cache_size(cache_root, max_size, None)?;
        }
        mark_rlib_cache_size_dirty(cache_root)?;
        match fs::rename(&staging, &entry) {
            Ok(()) => {}
            Err(_error) if entry.join("complete").exists() => {
                paths::remove_dir_all(&staging)?;
                reconcile_rlib_cache_size(cache_root, max_size, None)?;
                return Ok(skip_artifact_cache_publication(
                    PublicationSkipReason::ConcurrentPublication,
                ));
            }
            Err(error) => return Err(error.into()),
        }
        cache_size = cache_size.saturating_add(entry_size);
        if !rlib_cache_size_within_limit(cache_size, max_size) {
            reconcile_rlib_cache_size(cache_root, max_size, Some(&entry))?;
        } else {
            write_rlib_cache_size(cache_root, cache_size)?;
        }
        if let Some(stats) = stats {
            stats.published(entry_files, entry_size);
        }
        if let Some(snapshot) = snapshot {
            snapshot.record(&entry, outputs);
        }
        Ok(PublicationOutcome::Stored)
    })();
    if result.is_err() && staging.exists() {
        if let Err(error) = paths::remove_dir_all(&staging) {
            debug!(
                "failed to remove abandoned artifact cache publication {}: {error:#}",
                staging.display()
            );
        }
    }
    result
}

fn verify_rlib_cache_entry(
    entry: &Path,
    outputs: &[build_runner::OutputFile],
) -> CargoResult<bool> {
    let Some(expected) = verified_rlib_cache_manifest(entry)? else {
        return Ok(false);
    };
    let mut required = vec![
        entry.join("origin-pkg-root"),
        entry.join("origin-target-profile-root"),
        entry.join("inputs.blake3"),
        entry.join("compiler-messages"),
    ];
    let dep_info = entry.join("rustc-dep-info");
    if expected.contains_key("rustc-dep-info") {
        required.push(dep_info);
    }
    for output in outputs {
        let Some(name) = output.path.file_name() else {
            return Ok(false);
        };
        required.push(entry.join("files").join(name));
    }
    verify_rlib_cache_manifest_files(entry, &expected, required)
}

fn verify_rlib_cache_control_files(entry: &Path) -> CargoResult<bool> {
    let Some(expected) = verified_rlib_cache_manifest(entry)? else {
        return Ok(false);
    };
    let mut required = vec![entry.join("inputs.blake3")];
    if expected.contains_key("rustc-dep-info") {
        required.push(entry.join("rustc-dep-info"));
    }
    verify_rlib_cache_manifest_files(entry, &expected, required)
}

fn verified_rlib_cache_manifest(entry: &Path) -> CargoResult<Option<HashMap<String, String>>> {
    let manifest_path = entry.join("manifest.blake3");
    if !path_is_regular_file(&manifest_path) || !path_is_regular_file(&entry.join("complete")) {
        return Ok(None);
    }
    let complete_digest = paths::read(&entry.join("complete"))?;
    if rlib_cache_digest(&manifest_path)? != complete_digest.trim() {
        return Ok(None);
    }
    let manifest = paths::read(&manifest_path)?;
    let mut expected = HashMap::new();
    for line in manifest.lines() {
        let Some((path, digest)) = line.split_once('\t') else {
            return Ok(None);
        };
        if path.is_empty()
            || digest.is_empty()
            || digest.contains('\t')
            || expected
                .insert(path.to_string(), digest.to_string())
                .is_some()
        {
            return Ok(None);
        }
    }
    Ok(Some(expected))
}

fn verify_rlib_cache_manifest_files(
    entry: &Path,
    expected: &HashMap<String, String>,
    required: impl IntoIterator<Item = PathBuf>,
) -> CargoResult<bool> {
    for path in required {
        let relative = path.strip_prefix(entry).unwrap_or(&path).to_string_lossy();
        let Some(expected_digest) = expected.get(relative.as_ref()) else {
            return Ok(false);
        };
        if !path_is_regular_file(&path) || rlib_cache_digest(&path)? != *expected_digest {
            return Ok(false);
        }
    }
    Ok(true)
}

fn cleanup_corrupt_rlib_cache_entries(
    cache_root: &Path,
    entries: &[PathBuf],
    outputs: &[build_runner::OutputFile],
    max_size: Option<u64>,
) {
    if entries.is_empty() {
        return;
    }
    let Ok(Some(_lock)) = try_write_lock_rlib_cache(cache_root) else {
        return;
    };
    let mut changed = false;
    for entry in entries {
        if verify_rlib_cache_entry(entry, outputs).unwrap_or(false) {
            continue;
        }
        if mark_rlib_cache_size_dirty(cache_root).is_ok()
            && quarantine_rlib_cache_entry(entry).is_ok()
        {
            changed = true;
        }
    }
    if changed && let Err(error) = reconcile_rlib_cache_size(cache_root, max_size, None) {
        debug!("failed to reconcile artifact cache after removing corrupt entries: {error:#}");
    }
}

fn verify_rlib_cache_inputs(
    entry: &Path,
    rustc: &ProcessBuilder,
    rustc_cwd: &Path,
) -> CargoResult<bool> {
    let dep_info = entry.join("rustc-dep-info");
    let Some(digest) = rlib_cache_inputs_digest_with_env(&dep_info, rustc_cwd, |key, _| {
        rustc
            .get_env(key)
            .map(|value| value.to_string_lossy().into_owned())
    })?
    else {
        return Ok(false);
    };
    Ok(paths::read(&entry.join("inputs.blake3"))?.trim() == digest)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RlibCacheInputSupport {
    Supported,
    MissingDepInfo,
    GeneratedBuildInput,
}

fn rlib_cache_input_support(
    rustc_dep_info_loc: &Path,
    rustc_cwd: &Path,
    build_dir: &Path,
    output_root: &Path,
) -> CargoResult<RlibCacheInputSupport> {
    if !rustc_dep_info_loc.exists() {
        return Ok(RlibCacheInputSupport::MissingDepInfo);
    }
    let depinfo = fingerprint::parse_rustc_dep_info(rustc_dep_info_loc)?;
    let has_generated_build_input = depinfo.files.keys().any(|path| {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            rustc_cwd.join(path)
        };
        path.starts_with(build_dir) || path.starts_with(output_root)
    });
    Ok(if has_generated_build_input {
        RlibCacheInputSupport::GeneratedBuildInput
    } else {
        RlibCacheInputSupport::Supported
    })
}

#[cfg(test)]
mod artifact_cache_input_support_tests {
    use super::*;

    #[test]
    fn missing_and_generated_dep_info_are_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let rustc_cwd = temp.path();
        let build_dir = rustc_cwd.join("build");
        let output_root = build_dir.join("debug/deps");
        let dep_info = rustc_cwd.join("crate.d");

        assert_eq!(
            rlib_cache_input_support(&dep_info, rustc_cwd, &build_dir, &output_root).unwrap(),
            RlibCacheInputSupport::MissingDepInfo
        );

        fs::write(&dep_info, b"output: build/generated.rs\n").unwrap();
        assert_eq!(
            rlib_cache_input_support(&dep_info, rustc_cwd, &build_dir, &output_root).unwrap(),
            RlibCacheInputSupport::GeneratedBuildInput
        );

        fs::write(&dep_info, b"output: src/lib.rs\n").unwrap();
        assert_eq!(
            rlib_cache_input_support(&dep_info, rustc_cwd, &build_dir, &output_root).unwrap(),
            RlibCacheInputSupport::Supported
        );
    }
}

fn rlib_cache_inputs_digest_matching_rustc_dep_info(
    rustc_dep_info_loc: &Path,
    rustc_cwd: &Path,
    invocation_time: filetime::FileTime,
) -> CargoResult<Option<String>> {
    if !rustc_dep_info_loc.exists() {
        return Ok(None);
    }
    let depinfo = fingerprint::parse_rustc_dep_info(rustc_dep_info_loc)?;
    let mut files = depinfo.files.into_iter().collect::<Vec<_>>();
    files.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-inputs-v1\0");
    for (file, checksum_info) in files {
        let path = if file.is_absolute() {
            file.clone()
        } else {
            rustc_cwd.join(&file)
        };
        let Ok(contents) = fs::read(&path) else {
            return Ok(None);
        };
        if let Some((file_len, checksum)) = checksum_info {
            // Validate and key the same bytes so a same-mtime edit cannot
            // publish the compiled artifact under the edited input's digest.
            if contents.len() as u64 != file_len
                || fingerprint::Checksum::compute(checksum.algo(), contents.as_slice())? != checksum
            {
                return Ok(None);
            }
        }
        let Ok(mtime) = paths::mtime(&path) else {
            return Ok(None);
        };
        if mtime >= invocation_time {
            return Ok(None);
        }
        hasher.update(file.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        hasher.update(&contents);
        hasher.update(b"\0");
    }
    let mut envs = depinfo.env.iter().collect::<Vec<_>>();
    envs.sort_by_key(|(key, _)| key.as_str());
    for (key, value) in envs {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        if let Some(value) = value {
            hasher.update(b"set\0");
            hasher.update(value.as_bytes());
        } else {
            hasher.update(b"unset\0");
        }
        hasher.update(b"\0");
    }
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

fn rlib_cache_inputs_digest_with_env(
    rustc_dep_info_loc: &Path,
    rustc_cwd: &Path,
    mut env_value: impl FnMut(&str, &Option<String>) -> Option<String>,
) -> CargoResult<Option<String>> {
    if !rustc_dep_info_loc.exists() {
        return Ok(None);
    }
    let depinfo = fingerprint::parse_rustc_dep_info(rustc_dep_info_loc)?;
    let mut files = depinfo.files.keys().collect::<Vec<_>>();
    files.sort();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-inputs-v1\0");
    for file in files {
        let path = if file.is_absolute() {
            file.to_path_buf()
        } else {
            rustc_cwd.join(file)
        };
        if !path.is_file() {
            return Ok(None);
        }
        hasher.update(file.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        let Ok(contents) = fs::read(path) else {
            return Ok(None);
        };
        hasher.update(&contents);
        hasher.update(b"\0");
    }
    let mut envs = depinfo.env.iter().collect::<Vec<_>>();
    envs.sort_by_key(|(key, _)| key.as_str());
    for (key, value) in envs {
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        if let Some(value) = env_value(key, value) {
            hasher.update(b"set\0");
            hasher.update(value.as_bytes());
        } else {
            hasher.update(b"unset\0");
        }
        hasher.update(b"\0");
    }
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

fn delay_rlib_cache_input_digest_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let delay = std::env::var_os(ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS_FOR_TESTS)
        .and_then(|value| value.to_string_lossy().parse::<u64>().ok());
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let release = std::env::var_os(ARTIFACT_CACHE_INPUT_DIGEST_RELEASE_FILE_FOR_TESTS);
    if delay.is_none() && release.is_none() {
        return Ok(());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    wait_for_rlib_cache_test_release(release)?;
    if let Some(delay) = delay {
        std::thread::sleep(Duration::from_millis(delay));
    }
    Ok(())
}

fn delay_rlib_cache_restore_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let delay = std::env::var_os(ARTIFACT_CACHE_RESTORE_DELAY_MS_FOR_TESTS);
    let delay = delay.and_then(|value| value.to_string_lossy().parse::<u64>().ok());
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let release = std::env::var_os(ARTIFACT_CACHE_RESTORE_RELEASE_FILE_FOR_TESTS);
    if delay.is_none() && release.is_none() {
        return Ok(());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_RESTORE_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    wait_for_rlib_cache_test_release(release)?;
    if let Some(delay) = delay {
        std::thread::sleep(Duration::from_millis(delay));
    }
    Ok(())
}

fn delay_rlib_cache_publish_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let delay = std::env::var_os(ARTIFACT_CACHE_PUBLISH_DELAY_MS_FOR_TESTS)
        .and_then(|value| value.to_string_lossy().parse::<u64>().ok());
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let release = std::env::var_os(ARTIFACT_CACHE_PUBLISH_RELEASE_FILE_FOR_TESTS);
    if delay.is_none() && release.is_none() {
        return Ok(());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_PUBLISH_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    wait_for_rlib_cache_test_release(release)?;
    if let Some(delay) = delay {
        std::thread::sleep(Duration::from_millis(delay));
    }
    Ok(())
}

fn delay_rlib_cache_publish_locked_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let release = std::env::var_os(ARTIFACT_CACHE_PUBLISH_LOCKED_RELEASE_FILE_FOR_TESTS);
    if release.is_none() {
        return Ok(());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_PUBLISH_LOCKED_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    wait_for_rlib_cache_test_release(release)
}

fn delay_rlib_cache_restore_materialized_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let release = std::env::var_os(ARTIFACT_CACHE_RESTORE_MATERIALIZED_RELEASE_FILE_FOR_TESTS);
    if release.is_none() {
        return Ok(());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_RESTORE_MATERIALIZED_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    wait_for_rlib_cache_test_release(release)
}

fn restore_materialized_identity_witness_is_current(
    identity_witness: &crate::util::rustc::ArtifactCacheIdentityWitness,
) -> bool {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if std::env::var_os(ARTIFACT_CACHE_RESTORE_MATERIALIZED_STALE_IDENTITY_FOR_TESTS).is_some() {
        return false;
    }
    identity_witness.is_current()
}

fn wait_for_rlib_cache_test_release(release: Option<OsString>) -> CargoResult<()> {
    let Some(release) = release else {
        return Ok(());
    };
    let release = Path::new(&release);
    for _ in 0..3000 {
        if release.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(internal(format!(
        "timed out waiting for artifact cache test release file {}",
        release.display()
    )))
}

fn delay_rlib_cache_restore_admitted_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let delay = std::env::var_os(ARTIFACT_CACHE_RESTORE_ADMITTED_DELAY_MS_FOR_TESTS)
        .and_then(|value| value.to_string_lossy().parse::<u64>().ok());
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let release = std::env::var_os(ARTIFACT_CACHE_RESTORE_ADMITTED_RELEASE_FILE_FOR_TESTS);
    if delay.is_none() && release.is_none() {
        return Ok(());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_RESTORE_ADMITTED_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    wait_for_rlib_cache_test_release(release)?;
    if let Some(delay) = delay {
        std::thread::sleep(Duration::from_millis(delay));
    }
    Ok(())
}

fn append_rlib_cache_manifest(manifest: &mut String, entry: &Path, path: &Path) -> CargoResult<()> {
    let relative = path.strip_prefix(entry).unwrap_or(path).to_string_lossy();
    let digest = rlib_cache_digest(path)?;
    manifest.push_str(&relative);
    manifest.push('\t');
    manifest.push_str(&digest);
    manifest.push('\n');
    Ok(())
}

fn rlib_cache_digest(path: &Path) -> CargoResult<String> {
    Ok(blake3::hash(&fs::read(path)?).to_hex().to_string())
}

fn path_is_directory_no_follow(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn path_is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn staging_rlib_cache_entry(entry: &Path) -> CargoResult<PathBuf> {
    let key = entry.file_name().unwrap_or_default().to_string_lossy();
    let sequence = ARTIFACT_CACHE_PUBLICATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = entry.with_file_name(format!(
        ".{key}.staging-v8-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir(&staging)?;
    Ok(staging)
}

fn create_rlib_cache_directory_no_follow(path: &Path) -> CargoResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_dir()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Ok(path_is_directory_no_follow(path))
            }
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    }
}

fn read_lock_artifact_cache_process_until(
    deadline: Instant,
) -> Option<RwLockReadGuard<'static, ()>> {
    let mut retry_delay = Duration::from_millis(1);
    loop {
        match ARTIFACT_CACHE_ACCESS_LOCK.try_read() {
            Ok(lock) => return Some(lock),
            Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(retry_delay);
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_millis(32));
            }
            Err(TryLockError::WouldBlock) => return None,
        }
    }
}

fn write_lock_artifact_cache_process_until(
    deadline: Instant,
) -> Option<RwLockWriteGuard<'static, ()>> {
    let mut retry_delay = Duration::from_millis(1);
    loop {
        match ARTIFACT_CACHE_ACCESS_LOCK.try_write() {
            Ok(lock) => return Some(lock),
            Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(retry_delay);
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_millis(32));
            }
            Err(TryLockError::WouldBlock) => return None,
        }
    }
}

fn lock_rlib_cache_for_restore(cache_root: &Path) -> CargoResult<Option<ArtifactCacheRestoreLock>> {
    paths::create_dir_all(cache_root)?;
    if fs::symlink_metadata(cache_root.join(".cargo-artifact-cache-lock"))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        debug!("not restoring artifact cache entries because the lock path is a symlink");
        return Ok(None);
    }
    let deadline = Instant::now() + ARTIFACT_CACHE_LOCK_TIMEOUT;
    let Some(process_lock) = read_lock_artifact_cache_process_until(deadline) else {
        debug!(
            "not restoring artifact cache entry after waiting {:?} for in-process cache access",
            ARTIFACT_CACHE_LOCK_TIMEOUT
        );
        return Ok(None);
    };
    let filesystem = Filesystem::new(cache_root.to_path_buf());
    let mut retry_delay = Duration::from_millis(1);
    loop {
        match filesystem.try_open_ro_shared_create_strict(".cargo-artifact-cache-lock")? {
            TryLockResult::Acquired(filesystem_lock) => {
                return Ok(Some(ArtifactCacheRestoreLock {
                    _process_lock: process_lock,
                    _filesystem_lock: filesystem_lock,
                }));
            }
            TryLockResult::WouldBlock if Instant::now() < deadline => {
                std::thread::sleep(retry_delay);
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_millis(32));
            }
            TryLockResult::WouldBlock => {
                debug!(
                    "not restoring artifact cache entry after waiting {:?} for the cache lock",
                    ARTIFACT_CACHE_LOCK_TIMEOUT
                );
                return Ok(None);
            }
            TryLockResult::LockingUnsupported => {
                debug!("not restoring artifact cache entries because locking is unsupported");
                return Ok(None);
            }
        }
    }
}

fn try_write_lock_rlib_cache(cache_root: &Path) -> CargoResult<Option<crate::util::FileLock>> {
    paths::create_dir_all(cache_root)?;
    if fs::symlink_metadata(cache_root.join(".cargo-artifact-cache-lock"))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        debug!("not publishing artifact cache entries because the lock path is a symlink");
        return Ok(None);
    }
    Ok(
        match Filesystem::new(cache_root.to_path_buf())
            .try_open_rw_exclusive_create_strict(".cargo-artifact-cache-lock")?
        {
            TryLockResult::Acquired(lock) => Some(lock),
            TryLockResult::WouldBlock => {
                debug!("not publishing artifact cache entries because the cache lock is contended");
                None
            }
            TryLockResult::LockingUnsupported => {
                debug!("not publishing artifact cache entries because locking is unsupported");
                None
            }
        },
    )
}

fn lock_rlib_cache_exclusive(
    cache_root: &Path,
    operation: &str,
) -> CargoResult<Option<ArtifactCachePublicationLock>> {
    paths::create_dir_all(cache_root)?;
    if fs::symlink_metadata(cache_root.join(".cargo-artifact-cache-lock"))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        debug!("not publishing artifact cache entries because the lock path is a symlink");
        return Ok(None);
    }
    let deadline = Instant::now() + ARTIFACT_CACHE_LOCK_TIMEOUT;
    let Some(process_lock) = write_lock_artifact_cache_process_until(deadline) else {
        debug!(
            "not {operation} after waiting {:?} for in-process cache access",
            ARTIFACT_CACHE_LOCK_TIMEOUT
        );
        return Ok(None);
    };
    let filesystem = Filesystem::new(cache_root.to_path_buf());
    let mut retry_delay = Duration::from_millis(1);
    loop {
        match filesystem.try_open_rw_exclusive_create_strict(".cargo-artifact-cache-lock")? {
            TryLockResult::Acquired(filesystem_lock) => {
                return Ok(Some(ArtifactCachePublicationLock {
                    _process_lock: process_lock,
                    _filesystem_lock: filesystem_lock,
                }));
            }
            TryLockResult::WouldBlock if Instant::now() < deadline => {
                std::thread::sleep(retry_delay);
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_millis(32));
            }
            TryLockResult::WouldBlock => {
                debug!(
                    "not {operation} after waiting {:?} for the cache lock",
                    ARTIFACT_CACHE_LOCK_TIMEOUT
                );
                return Ok(None);
            }
            TryLockResult::LockingUnsupported => {
                debug!("not publishing artifact cache entries because locking is unsupported");
                return Ok(None);
            }
        }
    }
}

fn try_write_lock_rlib_cache_action(
    entry_root: &Path,
) -> CargoResult<Option<crate::util::FileLock>> {
    let cache_root = entry_root.parent().unwrap_or(entry_root);
    let lock_name = rlib_cache_action_lock_name(entry_root);
    let lock_path = cache_root.join(&lock_name);
    if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        debug!(
            "not publishing artifact cache entry because the action lock path is a symlink: {}",
            lock_path.display()
        );
        return Ok(None);
    }
    Ok(
        match Filesystem::new(cache_root.to_path_buf())
            .try_open_rw_exclusive_create_strict(&lock_name)?
        {
            TryLockResult::Acquired(lock) => Some(lock),
            TryLockResult::WouldBlock => {
                debug!("not publishing duplicate artifact cache action concurrently");
                None
            }
            TryLockResult::LockingUnsupported => {
                debug!("not publishing artifact cache entry because locking is unsupported");
                None
            }
        },
    )
}

fn rlib_cache_action_lock_name(entry_root: &Path) -> String {
    let action_name = entry_root.file_name().unwrap_or(entry_root.as_os_str());
    let action_digest = blake3::hash(action_name.as_encoded_bytes()).to_hex();
    format!(
        "{ARTIFACT_CACHE_ACTION_PUBLICATION_LOCK_PREFIX}-{}",
        &action_digest.as_str()[..ARTIFACT_CACHE_ACTION_LOCK_SHARD_HEX_LEN]
    )
}

fn cleanup_abandoned_rlib_cache_transients(cache_root: &Path) {
    let Ok(entry_roots) = fs::read_dir(cache_root) else {
        return;
    };
    for entry_root in entry_roots.flatten() {
        if !entry_root
            .file_type()
            .is_ok_and(|file_type| file_type.is_dir())
        {
            continue;
        }
        let entry_root = entry_root.path();
        let Ok(Some(_action_lock)) = try_write_lock_rlib_cache_action(&entry_root) else {
            continue;
        };
        cleanup_abandoned_rlib_cache_action_transients(&entry_root);
    }
}

fn cleanup_abandoned_rlib_cache_action_transients(entry_root: &Path) {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let retain_transients_for_tests =
        std::env::var_os(ARTIFACT_CACHE_TRANSIENT_REMOVE_FAILURE_FOR_TESTS).is_some();
    let Ok(entries) = fs::read_dir(entry_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.starts_with('.')
                && (name.contains(".publishing-")
                    || name.contains(".staging-v8-")
                    || name.contains(".rejected-"))
        }) {
            continue;
        }
        if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            continue;
        }
        if retain_transients_for_tests {
            debug!(
                "retaining abandoned artifact cache transient for test {}",
                path.display()
            );
            continue;
        }
        if let Err(error) = paths::remove_dir_all(&path) {
            debug!(
                "failed to remove abandoned artifact cache transient {}: {error:#}",
                path.display()
            );
        }
    }
}

fn recorded_rlib_cache_size(cache_root: &Path) -> Option<u64> {
    let state = cache_root.join(ARTIFACT_CACHE_SIZE_STATE);
    if !path_is_regular_file(&state) {
        return None;
    }
    paths::read(&state)
        .ok()?
        .trim()
        .strip_prefix(&format!("{ARTIFACT_CACHE_SIZE_STATE_VERSION} "))?
        .parse()
        .ok()
}

fn mark_rlib_cache_size_dirty(cache_root: &Path) -> CargoResult<()> {
    paths::write_atomic_no_follow(&cache_root.join(ARTIFACT_CACHE_SIZE_STATE), b"dirty\n")
}

fn rlib_cache_has_transients(cache_root: &Path) -> bool {
    let Ok(entry_roots) = fs::read_dir(cache_root) else {
        return true;
    };
    for entry_root in entry_roots {
        let Ok(entry_root) = entry_root else {
            return true;
        };
        if !entry_root
            .file_type()
            .is_ok_and(|file_type| file_type.is_dir())
        {
            continue;
        }
        let Ok(entries) = fs::read_dir(entry_root.path()) else {
            return true;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return true;
            };
            if entry.path().file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with('.') && name.contains(".rejected-")
            }) {
                return true;
            }
        }
    }
    false
}

fn write_rlib_cache_size(cache_root: &Path, size: u64) -> CargoResult<()> {
    if rlib_cache_has_transients(cache_root) {
        return mark_rlib_cache_size_dirty(cache_root);
    }
    paths::write_atomic_no_follow(
        &cache_root.join(ARTIFACT_CACHE_SIZE_STATE),
        format!("{ARTIFACT_CACHE_SIZE_STATE_VERSION} {size}\n").as_bytes(),
    )
}

fn reconcile_rlib_cache_size(
    cache_root: &Path,
    max_size: Option<u64>,
    retained_entry: Option<&Path>,
) -> CargoResult<u64> {
    cleanup_abandoned_rlib_cache_transients(cache_root);
    let size = match prune_rlib_cache_entries(cache_root, max_size, retained_entry) {
        Ok(size) => size,
        Err(error) => {
            mark_rlib_cache_size_dirty(cache_root)?;
            return Err(error);
        }
    };
    write_rlib_cache_size(cache_root, size)?;
    Ok(size)
}

fn try_read_lock_rlib_cache_within_limit(
    cache_root: &Path,
    max_size: Option<u64>,
) -> CargoResult<Option<ArtifactCacheRestoreLock>> {
    let Some(lock) = lock_rlib_cache_for_restore(cache_root)? else {
        return Ok(None);
    };
    if max_size.is_none()
        || recorded_rlib_cache_size(cache_root)
            .is_some_and(|size| rlib_cache_size_within_limit(size, max_size))
    {
        delay_rlib_cache_restore_admitted_for_tests()?;
        return Ok(Some(lock));
    }
    drop(lock);
    let Some(_lock) =
        lock_rlib_cache_exclusive(cache_root, "maintaining cache size before restore")?
    else {
        return Ok(None);
    };
    if !recorded_rlib_cache_size(cache_root)
        .is_some_and(|size| rlib_cache_size_within_limit(size, max_size))
    {
        reconcile_rlib_cache_size(cache_root, max_size, None)?;
    }
    drop(_lock);
    let Some(lock) = lock_rlib_cache_for_restore(cache_root)? else {
        return Ok(None);
    };
    if !recorded_rlib_cache_size(cache_root)
        .is_some_and(|size| rlib_cache_size_within_limit(size, max_size))
    {
        return Ok(None);
    }
    delay_rlib_cache_restore_admitted_for_tests()?;
    Ok(Some(lock))
}

fn prune_rlib_cache_entries(
    cache_root: &Path,
    max_size: Option<u64>,
    retained_entry: Option<&Path>,
) -> CargoResult<u64> {
    prune_rlib_cache_entries_with(cache_root, max_size, retained_entry, |path| {
        paths::remove_dir_all(path)
    })
}

fn prune_rlib_cache_entries_with(
    cache_root: &Path,
    max_size: Option<u64>,
    retained_entry: Option<&Path>,
    mut remove_entry: impl FnMut(&Path) -> CargoResult<()>,
) -> CargoResult<u64> {
    let entry_roots = fs::read_dir(cache_root)?;
    let mut completed = Vec::new();
    let mut total_size = 0u64;
    for entry_root in entry_roots {
        let entry_root = entry_root?;
        let file_type = entry_root.file_type()?;
        if file_type.is_symlink() {
            if entry_root.file_name() == OsStr::new(ARTIFACT_CACHE_SIZE_STATE) {
                continue;
            }
            anyhow::bail!(
                "artifact cache contains symlinked action root {}",
                entry_root.path().display()
            );
        }
        if !file_type.is_dir() {
            continue;
        }
        let entry_root = entry_root.path();
        let entries = fs::read_dir(&entry_root)?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                anyhow::bail!("artifact cache contains symlinked entry {}", path.display());
            }
            if !file_type.is_dir()
                || path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                || !path_is_regular_file(&path.join("complete"))
            {
                continue;
            }
            let size = artifact_cache_entry_size(&path)?;
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            total_size = total_size.saturating_add(size);
            completed.push((modified, size, path));
        }
    }
    let Some(max_size) = max_size else {
        return Ok(total_size);
    };
    if total_size <= max_size {
        return Ok(total_size);
    }
    completed.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in completed {
        if total_size <= max_size {
            break;
        }
        if retained_entry.is_some_and(|retained_entry| path == retained_entry) {
            continue;
        }
        if remove_entry(&path).is_ok() {
            total_size = total_size.saturating_sub(size);
            if let Some(parent) = path.parent()
                && let Ok(Some(_action_lock)) = try_write_lock_rlib_cache_action(parent)
            {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    if total_size > max_size {
        anyhow::bail!("failed to evict artifact cache entries below configured limit");
    }
    Ok(total_size)
}

fn rlib_cache_size_within_limit(size: u64, max_size: Option<u64>) -> bool {
    max_size.is_none_or(|max_size| size <= max_size)
}

#[cfg(test)]
mod artifact_cache_size_tests {
    use super::{
        ARTIFACT_CACHE_ACCESS_LOCK, ARTIFACT_CACHE_ACTION_PUBLICATION_LOCK_PREFIX,
        artifact_cache_entry_size, hash_path_tree, prune_rlib_cache_entries,
        prune_rlib_cache_entries_with, read_lock_artifact_cache_process_until,
        recorded_rlib_cache_size, rlib_cache_action_lock_name, rlib_cache_size_within_limit,
        staging_rlib_cache_entry, try_write_lock_rlib_cache_action,
        write_lock_artifact_cache_process_until, write_rlib_cache_size,
    };

    #[test]
    fn unconfigured_size_limit_is_unbounded() {
        assert!(rlib_cache_size_within_limit(u64::MAX, None));
        assert!(rlib_cache_size_within_limit(10, Some(10)));
        assert!(!rlib_cache_size_within_limit(11, Some(10)));
    }

    #[test]
    fn live_publication_does_not_dirty_committed_size_state() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("action").join(".entry.staging-v8-active");
        std::fs::create_dir_all(staging).unwrap();

        write_rlib_cache_size(temp.path(), 42).unwrap();

        assert_eq!(recorded_rlib_cache_size(temp.path()), Some(42));
    }

    #[test]
    fn staging_names_are_invisible_to_legacy_publication_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let action = temp.path().join("action");
        std::fs::create_dir(&action).unwrap();

        let staging = staging_rlib_cache_entry(&action.join("entry")).unwrap();
        let name = staging.file_name().unwrap().to_string_lossy();

        assert!(name.contains(".staging-v8-"));
        assert!(!name.contains(".publishing-"));
    }

    #[test]
    fn action_lock_shards_distinguish_one_byte_hash_collisions() {
        let temp = tempfile::tempdir().unwrap();
        let mut by_first_byte = std::collections::HashMap::new();
        for index in 0..10_000 {
            let action = temp.path().join(format!("action-{index}"));
            let lock_name = rlib_cache_action_lock_name(&action);
            let shard = lock_name
                .strip_prefix(ARTIFACT_CACHE_ACTION_PUBLICATION_LOCK_PREFIX)
                .unwrap()
                .strip_prefix('-')
                .unwrap();
            assert_eq!(shard.len(), 6);
            let first_byte = &shard[..2];
            if let Some(previous) = by_first_byte.insert(first_byte.to_string(), lock_name.clone())
                && previous != lock_name
            {
                return;
            }
        }
        panic!("failed to construct a one-byte action-lock hash collision");
    }

    #[test]
    fn process_cache_lock_attempts_respect_expired_deadlines() {
        let write = ARTIFACT_CACHE_ACCESS_LOCK.write().unwrap();
        assert!(read_lock_artifact_cache_process_until(std::time::Instant::now()).is_none());
        drop(write);

        let _read = ARTIFACT_CACHE_ACCESS_LOCK.read().unwrap();
        assert!(write_lock_artifact_cache_process_until(std::time::Instant::now()).is_none());
    }

    #[test]
    fn pruning_does_not_remove_an_action_root_held_by_a_publisher() {
        let temp = tempfile::tempdir().unwrap();
        let action = temp.path().join("action");
        let entry = action.join("entry");
        std::fs::create_dir_all(&entry).unwrap();
        std::fs::write(entry.join("complete"), b"complete").unwrap();
        let _action_lock = try_write_lock_rlib_cache_action(&action).unwrap().unwrap();

        assert_eq!(
            prune_rlib_cache_entries(temp.path(), Some(0), None).unwrap(),
            0
        );
        assert!(action.is_dir());
    }

    #[test]
    fn eviction_failure_does_not_report_oversized_cache_as_reconciled() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("action").join("entry");
        std::fs::create_dir_all(&entry).unwrap();
        std::fs::write(entry.join("complete"), b"complete").unwrap();

        let error = prune_rlib_cache_entries_with(temp.path(), Some(0), None, |_| {
            anyhow::bail!("injected removal failure")
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to evict artifact cache entries")
        );
    }

    #[test]
    #[cfg(unix)]
    fn entry_size_rejects_symlink_cycles() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file"), b"contents").unwrap();
        symlink(".", temp.path().join("loop")).unwrap();

        assert!(artifact_cache_entry_size(temp.path()).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn action_input_tree_rejects_symlink_cycles() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        symlink(".", temp.path().join("loop")).unwrap();

        assert!(
            hash_path_tree(
                &mut blake3::Hasher::new(),
                temp.path(),
                temp.path(),
                None,
                false,
                None,
                &mut Vec::new(),
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod artifact_cache_output_tests {
    use super::prepare_materialized_rlib_output_for_write;

    #[test]
    fn output_preparation_ignores_directories() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("output.dSYM");
        std::fs::create_dir(&directory).unwrap();

        prepare_materialized_rlib_output_for_write(&directory).unwrap();

        assert!(directory.is_dir());
    }
}

fn artifact_cache_entry_size(path: &Path) -> CargoResult<u64> {
    if !path_is_directory_no_follow(path) {
        anyhow::bail!(
            "artifact cache size root is not an ordinary directory: {}",
            path.display()
        );
    }
    let entries = fs::read_dir(path)?;
    let mut size = 0u64;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            anyhow::bail!("artifact cache contains symlink {}", path.display());
        } else if file_type.is_dir() {
            size = size.saturating_add(artifact_cache_entry_size(&path)?);
        } else if file_type.is_file() {
            size = size.saturating_add(entry.metadata()?.len());
        } else {
            anyhow::bail!(
                "artifact cache contains unsupported node {}",
                path.display()
            );
        }
    }
    Ok(size)
}

fn quarantine_rlib_cache_entry(entry: &Path) -> CargoResult<()> {
    let key = entry.file_name().unwrap_or_default().to_string_lossy();
    let sequence = ARTIFACT_CACHE_PUBLICATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let rejected =
        entry.with_file_name(format!(".{key}.rejected-{}-{sequence}", std::process::id()));
    match fs::rename(entry, &rejected) {
        Ok(()) => {
            if let Err(error) = paths::remove_dir_all(&rejected) {
                debug!(
                    "failed to remove rejected artifact cache entry {}: {error:#}",
                    rejected.display()
                );
            }
            Ok(())
        }
        Err(_error) if !entry.exists() => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn verbose_if_simple_exit_code(err: Error) -> Error {
    // If a signal on unix (`code == None`) or an abnormal termination
    // on Windows (codes like `0xC0000409`), don't hide the error details.
    match err
        .downcast_ref::<ProcessError>()
        .as_ref()
        .and_then(|perr| perr.code)
    {
        Some(n) if cargo_util::is_simple_exit_code(n) => VerboseError::new(err).into(),
        _ => err,
    }
}

fn prebuild_lock_exclusive(lock: LockKey) -> Work {
    Work::new(move |state| {
        state.lock_exclusive(&lock)?;
        Ok(())
    })
}

fn downgrade_lock_to_shared(lock: LockKey) -> Work {
    Work::new(move |state| {
        state.downgrade_to_shared(&lock)?;
        Ok(())
    })
}

/// Link the compiled target (often of form `foo-{metadata_hash}`) to the
/// final target. This must happen during both "Fresh" and "Compile".
fn link_targets(
    build_runner: &mut BuildRunner<'_, '_>,
    unit: &Unit,
    fresh: bool,
) -> CargoResult<Work> {
    let bcx = build_runner.bcx;
    let outputs = build_runner.outputs(unit)?;
    let export_dir = build_runner.files().export_dir();
    let package_id = unit.pkg.package_id();
    let manifest_path = PathBuf::from(unit.pkg.manifest_path());
    let profile = unit.profile.clone();
    let unit_mode = unit.mode;
    let features = unit.features.iter().map(|s| s.to_string()).collect();
    let json_messages = bcx.build_config.emit_json();
    let executable = build_runner.get_executable(unit)?;
    let preserve_sld_root_output = sld_native_incremental_root_output(build_runner, unit);
    let mut target = Target::clone(&unit.target);
    if let TargetSourcePath::Metabuild = target.src_path() {
        // Give it something to serialize.
        let path = unit
            .pkg
            .manifest()
            .metabuild_path(build_runner.bcx.ws.build_dir());
        target.set_src_path(TargetSourcePath::Path(path));
    }

    Ok(Work::new(move |state| {
        // If we're a "root crate", e.g., the target of this compilation, then we
        // hard link our outputs out of the `deps` directory into the directory
        // above. This means that `cargo build` will produce binaries in
        // `target/debug` which one probably expects.
        let mut destinations = vec![];
        for output in outputs.iter() {
            let src = &output.path;
            // This may have been a `cargo rustc` command which changes the
            // output, so the source may not actually exist.
            if !src.exists() {
                continue;
            }
            let Some(dst) = output.hardlink.as_ref() else {
                destinations.push(src.clone());
                continue;
            };
            destinations.push(dst.clone());
            let detach_sld_root_output =
                preserve_sld_root_output && output.flavor == FileFlavor::Normal;
            if detach_sld_root_output {
                copy_sld_native_incremental_artifact(src, dst)?;
            } else {
                paths::link_or_copy(src, dst)?;
            }
            if let Some(ref path) = output.export_path {
                let export_dir = export_dir.as_ref().unwrap();
                paths::create_dir_all(export_dir)?;

                if detach_sld_root_output {
                    copy_sld_native_incremental_artifact(src, path)?;
                } else {
                    paths::link_or_copy(src, path)?;
                }
            }
        }

        if json_messages {
            let debuginfo = match profile.debuginfo.into_inner() {
                TomlDebugInfo::None => machine_message::ArtifactDebuginfo::Int(0),
                TomlDebugInfo::Limited => machine_message::ArtifactDebuginfo::Int(1),
                TomlDebugInfo::Full => machine_message::ArtifactDebuginfo::Int(2),
                TomlDebugInfo::LineDirectivesOnly => {
                    machine_message::ArtifactDebuginfo::Named("line-directives-only")
                }
                TomlDebugInfo::LineTablesOnly => {
                    machine_message::ArtifactDebuginfo::Named("line-tables-only")
                }
            };
            let art_profile = machine_message::ArtifactProfile {
                opt_level: profile.opt_level.as_str(),
                debuginfo: Some(debuginfo),
                debug_assertions: profile.debug_assertions,
                overflow_checks: profile.overflow_checks,
                test: unit_mode.is_any_test(),
            };

            let msg = machine_message::Artifact {
                package_id: package_id.to_spec(),
                manifest_path,
                target: &target,
                profile: art_profile,
                features,
                filenames: destinations,
                executable,
                fresh,
            }
            .to_json_string();
            state.stdout(msg)?;
        }
        Ok(())
    }))
}

// For all plugin dependencies, add their -L paths (now calculated and present
// in `build_script_outputs`) to the dynamic library load path for the command
// to execute.
fn add_plugin_deps(
    rustc: &mut ProcessBuilder,
    build_script_outputs: &BuildScriptOutputs,
    build_scripts: &BuildScripts,
    root_output: &Path,
) -> CargoResult<()> {
    let var = paths::dylib_path_envvar();
    let search_path = rustc.get_env(var).unwrap_or_default();
    let mut search_path = env::split_paths(&search_path).collect::<Vec<_>>();
    for (pkg_id, metadata) in &build_scripts.plugins {
        let output = build_script_outputs
            .get(*metadata)
            .ok_or_else(|| internal(format!("couldn't find libs for plugin dep {}", pkg_id)))?;
        search_path.append(&mut filter_dynamic_search_path(
            output.library_paths.iter().map(AsRef::as_ref),
            root_output,
        ));
    }
    let search_path = paths::join_paths(&search_path, var)?;
    rustc.env(var, &search_path);
    Ok(())
}

fn get_dynamic_search_path(path: &Path) -> &Path {
    match path.to_str().and_then(|s| s.split_once("=")) {
        Some(("native" | "crate" | "dependency" | "framework" | "all", path)) => Path::new(path),
        _ => path,
    }
}

// Determine paths to add to the dynamic search path from -L entries
//
// Strip off prefixes like "native=" or "framework=" and filter out directories
// **not** inside our output directory since they are likely spurious and can cause
// clashes with system shared libraries (issue #3366).
fn filter_dynamic_search_path<'a, I>(paths: I, root_output: &Path) -> Vec<PathBuf>
where
    I: Iterator<Item = &'a PathBuf>,
{
    let mut search_path = vec![];
    for dir in paths {
        let dir = get_dynamic_search_path(dir);
        if dir.starts_with(&root_output) {
            search_path.push(dir.to_path_buf());
        } else {
            debug!(
                "Not including path {} in runtime library search path because it is \
                 outside target root {}",
                dir.display(),
                root_output.display()
            );
        }
    }
    search_path
}

/// Prepares flags and environments we can compute for a `rustc` invocation
/// before the job queue starts compiling any unit.
///
/// This builds a static view of the invocation. Flags depending on the
/// completion of other units will be added later in runtime, such as flags
/// from build scripts.
fn prepare_rustc(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> CargoResult<ProcessBuilder> {
    let gctx = build_runner.bcx.gctx;
    let is_primary = build_runner.is_primary_package(unit);
    let is_workspace = build_runner.bcx.ws.is_member(&unit.pkg);

    let mut base = build_runner
        .compilation
        .rustc_process(unit, is_primary, is_workspace)?;
    build_base_args(build_runner, &mut base, unit)?;
    if unit.pkg.manifest().is_embedded() {
        if !gctx.cli_unstable().script {
            anyhow::bail!(
                "parsing `{}` requires `-Zscript`",
                unit.pkg.manifest_path().display()
            );
        }
        base.arg("-Z").arg("crate-attr=feature(frontmatter)");
        base.arg("-Z").arg("crate-attr=allow(unused_features)");
    }

    base.inherit_jobserver(&build_runner.jobserver);
    build_deps_args(&mut base, build_runner, unit)?;
    add_cap_lints(build_runner.bcx, unit, &mut base);
    if let Some(args) = build_runner.bcx.extra_args_for(unit) {
        base.args(args);
    }
    base.args(&unit.rustflags);
    if gctx.cli_unstable().binary_dep_depinfo {
        base.arg("-Z").arg("binary-dep-depinfo");
    }
    if build_runner.bcx.gctx.cli_unstable().checksum_freshness {
        base.arg("-Z").arg("checksum-hash-algorithm=blake3");
    }

    if is_primary {
        base.env("CARGO_PRIMARY_PACKAGE", "1");
        let file_list = build_runner.sbom_output_files(unit)?;
        if !file_list.is_empty() {
            let file_list = std::env::join_paths(file_list)?;
            base.env("CARGO_SBOM_PATH", file_list);
        }
    }

    if unit.target.is_test() || unit.target.is_bench() {
        let tmp = build_runner
            .files()
            .layout(unit.kind)
            .build_dir()
            .prepare_tmp()?;
        base.env("CARGO_TARGET_TMPDIR", tmp.display().to_string());
    }

    if build_runner.bcx.gctx.cli_unstable().cargo_lints {
        // Added last to reduce the risk of RUSTFLAGS or `[lints]` from interfering with
        // `unused_dependencies` tracking
        base.arg("--force-warn=unused_crate_dependencies");
    }

    Ok(base)
}

/// Prepares flags and environments we can compute for a `rustdoc` invocation
/// before the job queue starts compiling any unit.
///
/// This builds a static view of the invocation. Flags depending on the
/// completion of other units will be added later in runtime, such as flags
/// from build scripts.
fn prepare_rustdoc(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> CargoResult<ProcessBuilder> {
    let bcx = build_runner.bcx;
    // script_metadata is not needed here, it is only for tests.
    let mut rustdoc = build_runner.compilation.rustdoc_process(unit, None)?;
    if unit.pkg.manifest().is_embedded() {
        if !bcx.gctx.cli_unstable().script {
            anyhow::bail!(
                "parsing `{}` requires `-Zscript`",
                unit.pkg.manifest_path().display()
            );
        }
        rustdoc.arg("-Z").arg("crate-attr=feature(frontmatter)");
        rustdoc.arg("-Z").arg("crate-attr=allow(unused_features)");
    }
    rustdoc.inherit_jobserver(&build_runner.jobserver);
    let crate_name = unit.target.crate_name();
    rustdoc.arg("--crate-name").arg(&crate_name);
    add_path_args(bcx.ws, unit, &mut rustdoc);
    add_cap_lints(bcx, unit, &mut rustdoc);

    unit.kind.add_target_arg(&mut rustdoc);

    let doc_dir = if build_runner.bcx.build_config.intent.wants_doc_json_output() {
        // Always use new layout for '--output-format=json'.
        // In fix for https://github.com/rust-lang/cargo/issues/16291

        build_runner.files().out_dir_new_layout(unit)
    } else {
        build_runner.files().output_dir(unit)
    };

    rustdoc.arg("-o").arg(&doc_dir);
    rustdoc.args(&features_args(unit));
    rustdoc.args(&check_cfg_args(unit));

    add_error_format_and_color(build_runner, &mut rustdoc);
    add_allow_features(build_runner, &mut rustdoc);

    if build_runner.bcx.gctx.cli_unstable().rustdoc_depinfo {
        // html-static-files is required for keeping the shared styling resources
        // html-non-static-files is required for keeping the original rustdoc emission
        let mut arg = if build_runner.bcx.gctx.cli_unstable().rustdoc_mergeable_info {
            // toolchain resources are written at the end, at the same time as merging
            OsString::from("--emit=html-non-static-files,dep-info=")
        } else {
            // if not using mergeable CCI, everything is written every time
            OsString::from("--emit=html-static-files,html-non-static-files,dep-info=")
        };
        arg.push(rustdoc_dep_info_loc(build_runner, unit));
        rustdoc.arg(arg);

        if build_runner.bcx.gctx.cli_unstable().checksum_freshness {
            rustdoc.arg("-Z").arg("checksum-hash-algorithm=blake3");
        }

        rustdoc.arg("-Zunstable-options");
    } else if build_runner.bcx.gctx.cli_unstable().rustdoc_mergeable_info {
        // toolchain resources are written at the end, at the same time as merging
        rustdoc.arg("--emit=html-non-static-files");
        rustdoc.arg("-Zunstable-options");
    }

    if build_runner.bcx.gctx.cli_unstable().rustdoc_mergeable_info {
        // write out mergeable data to be imported
        rustdoc.arg("--merge=none");
        let mut arg = OsString::from("--parts-out-dir=");
        // `-Zrustdoc-mergeable-info` always uses the new layout.
        arg.push(build_runner.files().out_dir_new_layout(unit));
        rustdoc.arg(arg);
    }

    if let Some(trim_paths) = unit.profile.trim_paths.as_ref() {
        trim_paths_args_rustdoc(&mut rustdoc, build_runner, unit, trim_paths)?;
    }

    rustdoc.args(unit.pkg.manifest().lint_rustflags());

    let metadata = build_runner.metadata_for_doc_units[unit];
    rustdoc
        .arg("-C")
        .arg(format!("metadata={}", metadata.c_metadata()));

    if unit.mode.is_doc_scrape() {
        debug_assert!(build_runner.bcx.scrape_units.contains(unit));

        if unit.target.is_test() {
            rustdoc.arg("--scrape-tests");
        }

        rustdoc.arg("-Zunstable-options");

        rustdoc
            .arg("--scrape-examples-output-path")
            .arg(scrape_output_path(build_runner, unit)?);

        // Only scrape example for items from crates in the workspace, to reduce generated file size
        for pkg in build_runner.bcx.packages.packages() {
            let names = pkg
                .targets()
                .iter()
                .map(|target| target.crate_name())
                .collect::<HashSet<_>>();
            for name in names {
                rustdoc.arg("--scrape-examples-target-crate").arg(name);
            }
        }
    }

    if should_include_scrape_units(build_runner.bcx, unit) {
        rustdoc.arg("-Zunstable-options");
    }

    build_deps_args(&mut rustdoc, build_runner, unit)?;
    rustdoc::add_root_urls(build_runner, unit, &mut rustdoc)?;

    rustdoc::add_output_format(build_runner, &mut rustdoc)?;

    if let Some(args) = build_runner.bcx.extra_args_for(unit) {
        rustdoc.args(args);
    }
    rustdoc.args(&unit.rustdocflags);

    if !crate_version_flag_already_present(&rustdoc) {
        append_crate_version_flag(unit, &mut rustdoc);
    }

    Ok(rustdoc)
}

/// Creates a unit of work invoking `rustdoc` for documenting the `unit`.
fn rustdoc(build_runner: &mut BuildRunner<'_, '_>, unit: &Unit) -> CargoResult<Work> {
    let mut rustdoc = prepare_rustdoc(build_runner, unit)?;

    let crate_name = unit.target.crate_name();
    let is_json_output = build_runner.bcx.build_config.intent.wants_doc_json_output();
    let doc_dir = build_runner.files().output_dir(unit);
    // Create the documentation directory ahead of time as rustdoc currently has
    // a bug where concurrent invocations will race to create this directory if
    // it doesn't already exist.
    paths::create_dir_all(&doc_dir)?;

    let target_desc = unit.target.description_named();
    let name = unit.pkg.name();
    let build_script_outputs = Arc::clone(&build_runner.build_script_outputs);
    let package_id = unit.pkg.package_id();
    let target = Target::clone(&unit.target);
    let manifest = ManifestErrorContext::new(build_runner, unit);

    let rustdoc_dep_info_loc = rustdoc_dep_info_loc(build_runner, unit);
    let dep_info_loc = fingerprint::dep_info_loc(build_runner, unit);
    let build_dir = build_runner.bcx.ws.build_dir().into_path_unlocked();
    let pkg_root = unit.pkg.root().to_path_buf();
    let cwd = rustdoc
        .get_cwd()
        .unwrap_or_else(|| build_runner.bcx.gctx.cwd())
        .to_path_buf();
    let fingerprint_dir = build_runner.files().fingerprint_dir(unit);
    let is_local = unit.is_local();
    let env_config = Arc::clone(build_runner.bcx.gctx.env_config()?);
    let rustdoc_depinfo_enabled = build_runner.bcx.gctx.cli_unstable().rustdoc_depinfo;

    let mut output_options = OutputOptions::for_dirty(build_runner, unit);
    let script_metadatas = build_runner.find_build_script_metadatas(unit);
    let scrape_outputs = if should_include_scrape_units(build_runner.bcx, unit) {
        Some(
            build_runner
                .bcx
                .scrape_units
                .iter()
                .map(|unit| {
                    Ok((
                        build_runner.files().metadata(unit).unit_id(),
                        scrape_output_path(build_runner, unit)?,
                    ))
                })
                .collect::<CargoResult<HashMap<_, _>>>()?,
        )
    } else {
        None
    };

    let failed_scrape_units = Arc::clone(&build_runner.failed_scrape_units);
    let hide_diagnostics_for_scrape_unit = build_runner.bcx.unit_can_fail_for_docscraping(unit)
        && !matches!(
            build_runner.bcx.gctx.shell().verbosity(),
            Verbosity::Verbose
        );
    let failed_scrape_diagnostic = hide_diagnostics_for_scrape_unit.then(|| {
        make_failed_scrape_diagnostic(
            build_runner,
            unit,
            format_args!("failed to scan {target_desc} in package `{name}` for example code usage"),
        )
    });
    if hide_diagnostics_for_scrape_unit {
        output_options.show_diagnostics = false;
    }

    Ok(Work::new(move |state| {
        add_custom_flags(
            &mut rustdoc,
            &build_script_outputs.lock().unwrap(),
            script_metadatas,
        )?;

        // Add the output of scraped examples to the rustdoc command.
        // This action must happen after the unit's dependencies have finished,
        // because some of those deps may be Docscrape units which have failed.
        // So we dynamically determine which `--with-examples` flags to pass here.
        if let Some(scrape_outputs) = scrape_outputs {
            let failed_scrape_units = failed_scrape_units.lock().unwrap();
            for (metadata, output_path) in &scrape_outputs {
                if !failed_scrape_units.contains(metadata) {
                    rustdoc.arg("--with-examples").arg(output_path);
                }
            }
        }

        if !is_json_output {
            let crate_dir = doc_dir.join(&crate_name);
            if crate_dir.exists() {
                // Remove output from a previous build. This ensures that stale
                // files for removed items are removed.
                debug!("removing pre-existing doc directory {:?}", crate_dir);
                paths::remove_dir_all(&crate_dir)?;
            }
        };
        state.running(&rustdoc);
        let timestamp = paths::set_invocation_time(&fingerprint_dir)?;

        let result = rustdoc
            .exec_with_streaming(
                &mut |line| on_stdout_line(state, line, package_id, &target),
                &mut |line| {
                    on_stderr_line(
                        state,
                        line,
                        package_id,
                        &manifest,
                        &target,
                        &mut output_options,
                    )
                },
                false,
            )
            .map_err(verbose_if_simple_exit_code)
            .with_context(|| format!("could not document `{}`", name));

        if let Err(e) = result {
            if let Some(diagnostic) = failed_scrape_diagnostic {
                state.warning(diagnostic);
            }

            return Err(e);
        }

        if rustdoc_depinfo_enabled && rustdoc_dep_info_loc.exists() {
            fingerprint::translate_dep_info(
                &rustdoc_dep_info_loc,
                &dep_info_loc,
                &cwd,
                &pkg_root,
                &build_dir,
                &rustdoc,
                // Should we track source file for doc gen?
                is_local,
                &env_config,
            )
            .with_context(|| {
                internal(format_args!(
                    "could not parse/generate dep info at: {}",
                    rustdoc_dep_info_loc.display()
                ))
            })?;
            // This mtime shift allows Cargo to detect if a source file was
            // modified in the middle of the build.
            paths::set_file_time_no_err(dep_info_loc, timestamp);
        }

        Ok(())
    }))
}

// The --crate-version flag could have already been passed in RUSTDOCFLAGS
// or as an extra compiler argument for rustdoc
fn crate_version_flag_already_present(rustdoc: &ProcessBuilder) -> bool {
    rustdoc.get_args().any(|flag| {
        flag.to_str()
            .map_or(false, |flag| flag.starts_with(RUSTDOC_CRATE_VERSION_FLAG))
    })
}

fn append_crate_version_flag(unit: &Unit, rustdoc: &mut ProcessBuilder) {
    rustdoc
        .arg(RUSTDOC_CRATE_VERSION_FLAG)
        .arg(unit.pkg.version().to_string());
}

/// Adds [`--cap-lints`] to the command to execute.
///
/// [`--cap-lints`]: https://doc.rust-lang.org/nightly/rustc/lints/levels.html#capping-lints
fn add_cap_lints(bcx: &BuildContext<'_, '_>, unit: &Unit, cmd: &mut ProcessBuilder) {
    // If this is an upstream dep we don't want warnings from, turn off all
    // lints.
    if !unit.show_warnings(bcx.gctx) {
        cmd.arg("--cap-lints").arg("allow");

    // If this is an upstream dep but we *do* want warnings, make sure that they
    // don't fail compilation.
    } else if !unit.is_local() {
        cmd.arg("--cap-lints").arg("warn");
    }
}

/// Forwards [`-Zallow-features`] if it is set for cargo.
///
/// [`-Zallow-features`]: https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#allow-features
fn add_allow_features(build_runner: &BuildRunner<'_, '_>, cmd: &mut ProcessBuilder) {
    if let Some(allow) = &build_runner.bcx.gctx.cli_unstable().allow_features {
        use std::fmt::Write;
        let mut arg = String::from("-Zallow-features=");
        for f in allow {
            let _ = write!(&mut arg, "{f},");
        }
        cmd.arg(arg.trim_end_matches(','));
    }
}

/// Adds [`--error-format`] to the command to execute.
///
/// Cargo always uses JSON output. This has several benefits, such as being
/// easier to parse, handles changing formats (for replaying cached messages),
/// ensures atomic output (so messages aren't interleaved), allows for
/// intercepting messages like rmeta artifacts, etc. rustc includes a
/// "rendered" field in the JSON message with the message properly formatted,
/// which Cargo will extract and display to the user.
///
/// [`--error-format`]: https://doc.rust-lang.org/nightly/rustc/command-line-arguments.html#--error-format-control-how-errors-are-produced
fn add_error_format_and_color(build_runner: &BuildRunner<'_, '_>, cmd: &mut ProcessBuilder) {
    let enable_timings =
        build_runner.bcx.gctx.cli_unstable().section_timings && build_runner.bcx.logger.is_some();
    if enable_timings {
        cmd.arg("-Zunstable-options");
    }

    cmd.arg("--error-format=json");

    let mut json = String::from("--json=diagnostic-rendered-ansi,artifacts,future-incompat");
    if build_runner.bcx.gctx.cli_unstable().cargo_lints {
        json.push_str(",unused-externs-silent");
    }
    if let MessageFormat::Short | MessageFormat::Json { short: true, .. } =
        build_runner.bcx.build_config.message_format
    {
        json.push_str(",diagnostic-short");
    } else if build_runner.bcx.gctx.shell().err_unicode()
        && build_runner.bcx.gctx.cli_unstable().rustc_unicode
    {
        json.push_str(",diagnostic-unicode");
    }
    if enable_timings {
        json.push_str(",timings");
    }
    cmd.arg(json);

    let gctx = build_runner.bcx.gctx;
    if let Some(width) = gctx.shell().err_width().diagnostic_terminal_width() {
        cmd.arg(format!("--diagnostic-width={width}"));
    }
}

/// Adds essential rustc flags and environment variables to the command to execute.
fn build_base_args(
    build_runner: &BuildRunner<'_, '_>,
    cmd: &mut ProcessBuilder,
    unit: &Unit,
) -> CargoResult<()> {
    assert!(!unit.mode.is_run_custom_build());

    let bcx = build_runner.bcx;
    let Profile {
        ref opt_level,
        codegen_backend,
        codegen_units,
        debuginfo,
        debug_assertions,
        split_debuginfo,
        overflow_checks,
        rpath,
        ref panic,
        incremental,
        strip,
        rustflags: profile_rustflags,
        trim_paths,
        hint_mostly_unused: profile_hint_mostly_unused,
        ..
    } = unit.profile.clone();
    let hints = unit.pkg.hints().cloned().unwrap_or_default();
    let test = unit.mode.is_any_test();

    let warn = |msg: &str| {
        bcx.gctx.shell().warn(format!(
            "{}@{}: {msg}",
            unit.pkg.package_id().name(),
            unit.pkg.package_id().version()
        ))
    };
    let unit_capped_warn = |msg: &str| {
        if unit.show_warnings(bcx.gctx) {
            warn(msg)
        } else {
            Ok(())
        }
    };

    cmd.arg("--crate-name").arg(&unit.target.crate_name());

    let edition = unit.target.edition();
    edition.cmd_edition_arg(cmd);

    add_path_args(bcx.ws, unit, cmd);
    add_error_format_and_color(build_runner, cmd);
    add_allow_features(build_runner, cmd);

    let mut contains_dy_lib = false;
    if !test {
        for crate_type in &unit.target.rustc_crate_types() {
            cmd.arg("--crate-type").arg(crate_type.as_str());
            contains_dy_lib |= crate_type == &CrateType::Dylib;
        }
    }

    if unit.mode.is_check() {
        cmd.arg("--emit=dep-info,metadata");
    } else if build_runner.bcx.gctx.cli_unstable().no_embed_metadata {
        // Nightly rustc supports the -Zembed-metadata=no flag, which tells it to avoid including
        // full metadata in rlib/dylib artifacts, to save space on disk. In this case, metadata
        // will only be stored in .rmeta files.
        // When we use this flag, we should also pass --emit=metadata to all artifacts that
        // contain useful metadata (rlib/dylib/proc macros), so that a .rmeta file is actually
        // generated. If we didn't do this, the full metadata would not get written anywhere.
        // However, we do not want to pass --emit=metadata to artifacts that never produce useful
        // metadata, such as binaries, because that would just unnecessarily create empty .rmeta
        // files on disk.
        if unit.benefits_from_no_embed_metadata() {
            cmd.arg("--emit=dep-info,metadata,link");
            cmd.args(&["-Z", "embed-metadata=no"]);
        } else {
            cmd.arg("--emit=dep-info,link");
        }
    } else {
        // If we don't use -Zembed-metadata=no, we emit .rmeta files only for rlib outputs.
        // This metadata may be used in this session for a pipelined compilation, or it may
        // be used in a future Cargo session as part of a pipelined compile.
        if !unit.requires_upstream_objects() {
            cmd.arg("--emit=dep-info,metadata,link");
        } else {
            cmd.arg("--emit=dep-info,link");
        }
    }

    let prefer_dynamic = (unit.target.for_host() && !unit.target.is_custom_build())
        || (contains_dy_lib && !build_runner.is_primary_package(unit));
    if prefer_dynamic {
        cmd.arg("-C").arg("prefer-dynamic");
    }

    if opt_level.as_str() != "0" {
        cmd.arg("-C").arg(&format!("opt-level={}", opt_level));
    }

    if *panic != PanicStrategy::Unwind {
        cmd.arg("-C").arg(format!("panic={}", panic));
    }
    if *panic == PanicStrategy::ImmediateAbort {
        cmd.arg("-Z").arg("unstable-options");
    }

    cmd.args(&lto_args(build_runner, unit));

    if let Some(backend) = codegen_backend {
        cmd.arg("-Z").arg(&format!("codegen-backend={}", backend));
    }

    if let Some(n) = codegen_units {
        cmd.arg("-C").arg(&format!("codegen-units={}", n));
    }

    let debuginfo = debuginfo.into_inner();
    // Shorten the number of arguments if possible.
    if debuginfo != TomlDebugInfo::None {
        cmd.arg("-C").arg(format!("debuginfo={debuginfo}"));
        // This is generally just an optimization on build time so if we don't
        // pass it then it's ok. The values for the flag (off, packed, unpacked)
        // may be supported or not depending on the platform, so availability is
        // checked per-value. For example, at the time of writing this code, on
        // Windows the only stable valid value for split-debuginfo is "packed",
        // while on Linux "unpacked" is also stable.
        if let Some(split) = split_debuginfo {
            if build_runner
                .bcx
                .target_data
                .info(unit.kind)
                .supports_debuginfo_split(split)
            {
                cmd.arg("-C").arg(format!("split-debuginfo={split}"));
            }
        }
    }

    if let Some(trim_paths) = trim_paths {
        trim_paths_args(cmd, build_runner, unit, &trim_paths)?;
    }

    cmd.args(unit.pkg.manifest().lint_rustflags());
    cmd.args(&profile_rustflags);

    // `-C overflow-checks` is implied by the setting of `-C debug-assertions`,
    // so we only need to provide `-C overflow-checks` if it differs from
    // the value of `-C debug-assertions` we would provide.
    if opt_level.as_str() != "0" {
        if debug_assertions {
            cmd.args(&["-C", "debug-assertions=on"]);
            if !overflow_checks {
                cmd.args(&["-C", "overflow-checks=off"]);
            }
        } else if overflow_checks {
            cmd.args(&["-C", "overflow-checks=on"]);
        }
    } else if !debug_assertions {
        cmd.args(&["-C", "debug-assertions=off"]);
        if overflow_checks {
            cmd.args(&["-C", "overflow-checks=on"]);
        }
    } else if !overflow_checks {
        cmd.args(&["-C", "overflow-checks=off"]);
    }

    if test && unit.target.harness() {
        cmd.arg("--test");

        // Cargo has historically never compiled `--test` binaries with
        // `panic=abort` because the `test` crate itself didn't support it.
        // Support is now upstream, however, but requires an unstable flag to be
        // passed when compiling the test. We require, in Cargo, an unstable
        // flag to pass to rustc, so register that here. Eventually this flag
        // will simply not be needed when the behavior is stabilized in the Rust
        // compiler itself.
        if *panic == PanicStrategy::Abort || *panic == PanicStrategy::ImmediateAbort {
            cmd.arg("-Z").arg("panic-abort-tests");
        }
    } else if test {
        cmd.arg("--cfg").arg("test");
    }

    cmd.args(&features_args(unit));
    cmd.args(&check_cfg_args(unit));

    let meta = build_runner.files().metadata(unit);
    cmd.arg("-C")
        .arg(&format!("metadata={}", meta.c_metadata()));
    if let Some(c_extra_filename) = meta.c_extra_filename() {
        cmd.arg("-C")
            .arg(&format!("extra-filename=-{c_extra_filename}"));
    }

    if rpath {
        cmd.arg("-C").arg("rpath");
    }

    cmd.arg("--out-dir")
        .arg(&build_runner.files().output_dir(unit));

    unit.kind.add_target_arg(cmd);

    add_codegen_linker(cmd, build_runner, unit, bcx.gctx.target_applies_to_host()?);

    if incremental {
        add_codegen_incremental(cmd, build_runner, unit)
    }

    let pkg_hint_mostly_unused = match hints.mostly_unused {
        None => None,
        Some(toml::Value::Boolean(b)) => Some(b),
        Some(v) => {
            unit_capped_warn(&format!(
                "ignoring unsupported value type ({}) for 'hints.mostly-unused', which expects a boolean",
                v.type_str()
            ))?;
            None
        }
    };
    if profile_hint_mostly_unused
        .or(pkg_hint_mostly_unused)
        .unwrap_or(false)
    {
        if bcx.gctx.cli_unstable().profile_hint_mostly_unused {
            cmd.arg("-Zhint-mostly-unused");
        } else {
            if profile_hint_mostly_unused.is_some() {
                // Profiles come from the top-level unit, so we don't use `unit_capped_warn` here.
                warn(
                    "ignoring 'hint-mostly-unused' profile option, pass `-Zprofile-hint-mostly-unused` to enable it",
                )?;
            } else if pkg_hint_mostly_unused.is_some() {
                unit_capped_warn(
                    "ignoring 'hints.mostly-unused', pass `-Zprofile-hint-mostly-unused` to enable it",
                )?;
            }
        }
    }

    let strip = strip.into_inner();
    if strip != StripInner::None {
        cmd.arg("-C").arg(format!("strip={}", strip));
    }

    if unit.is_std {
        // -Zforce-unstable-if-unmarked prevents the accidental use of
        // unstable crates within the sysroot (such as "extern crate libc" or
        // any non-public crate in the sysroot).
        //
        // RUSTC_BOOTSTRAP allows unstable features on stable.
        cmd.arg("-Z")
            .arg("force-unstable-if-unmarked")
            .env("RUSTC_BOOTSTRAP", "1");
    }

    Ok(())
}

/// All active features for the unit passed as `--cfg features=<feature-name>`.
fn features_args(unit: &Unit) -> Vec<OsString> {
    let mut args = Vec::with_capacity(unit.features.len() * 2);

    for feat in &unit.features {
        args.push(OsString::from("--cfg"));
        args.push(OsString::from(format!("feature=\"{}\"", feat)));
    }

    args
}

/// Like [`trim_paths_args`] but for rustdoc invocations.
fn trim_paths_args_rustdoc(
    cmd: &mut ProcessBuilder,
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
    trim_paths: &TomlTrimPaths,
) -> CargoResult<()> {
    match trim_paths {
        // rustdoc supports diagnostics trimming only.
        TomlTrimPaths::Values(values) if !values.contains(&TomlTrimPathsValue::Diagnostics) => {
            return Ok(());
        }
        _ => {}
    }

    // feature gate was checked during manifest/config parsing.
    cmd.arg("-Zunstable-options");

    // Order of `--remap-path-prefix` flags is important for `-Zbuild-std`.
    // We want to show `/rustc/<hash>/library/std` instead of `std-0.0.0`.
    cmd.arg(package_remap(build_runner, unit));
    cmd.arg(build_dir_remap(build_runner));
    cmd.arg(sysroot_remap(build_runner, unit));

    Ok(())
}

/// Generates the `--remap-path-scope` and `--remap-path-prefix` for [RFC 3127].
/// See also unstable feature [`-Ztrim-paths`].
///
/// [RFC 3127]: https://rust-lang.github.io/rfcs/3127-trim-paths.html
/// [`-Ztrim-paths`]: https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#profile-trim-paths-option
fn trim_paths_args(
    cmd: &mut ProcessBuilder,
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
    trim_paths: &TomlTrimPaths,
) -> CargoResult<()> {
    if trim_paths.is_none() {
        return Ok(());
    }

    // feature gate was checked during manifest/config parsing.
    cmd.arg(format!("--remap-path-scope={trim_paths}"));

    // Order of `--remap-path-prefix` flags is important for `-Zbuild-std`.
    // We want to show `/rustc/<hash>/library/std` instead of `std-0.0.0`.
    cmd.arg(package_remap(build_runner, unit));
    cmd.arg(build_dir_remap(build_runner));
    cmd.arg(sysroot_remap(build_runner, unit));

    Ok(())
}

/// Path prefix remap rules for sysroot.
///
/// This remap logic aligns with rustc:
/// <https://github.com/rust-lang/rust/blob/c2ef3516/src/bootstrap/src/lib.rs#L1113-L1116>
fn sysroot_remap(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> OsString {
    let mut remap = OsString::from("--remap-path-prefix=");
    remap.push({
        // See also `detect_sysroot_src_path()`.
        let mut sysroot = build_runner.bcx.target_data.info(unit.kind).sysroot.clone();
        sysroot.push("lib");
        sysroot.push("rustlib");
        sysroot.push("src");
        sysroot.push("rust");
        sysroot
    });
    remap.push("=");
    remap.push("/rustc/");
    if let Some(commit_hash) = build_runner.bcx.rustc().commit_hash.as_ref() {
        remap.push(commit_hash);
    } else {
        remap.push(build_runner.bcx.rustc().version.to_string());
    }
    remap
}

/// Path prefix remap rules for dependencies.
///
/// * Git dependencies: remove `~/.cargo/git/checkouts` prefix.
/// * Registry dependencies: remove `~/.cargo/registry/src` prefix.
/// * Others (e.g. path dependencies):
///     * relative paths to workspace root if inside the workspace directory.
///     * otherwise remapped to `<pkg>-<version>`.
fn package_remap(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> OsString {
    let pkg_root = unit.pkg.root();
    let ws_root = build_runner.bcx.ws.root();
    let mut remap = OsString::from("--remap-path-prefix=");
    let source_id = unit.pkg.package_id().source_id();
    if source_id.is_git() {
        remap.push(
            build_runner
                .bcx
                .gctx
                .git_checkouts_path()
                .as_path_unlocked(),
        );
        remap.push("=");
    } else if source_id.is_registry() {
        remap.push(
            build_runner
                .bcx
                .gctx
                .registry_source_path()
                .as_path_unlocked(),
        );
        remap.push("=");
    } else if pkg_root.strip_prefix(ws_root).is_ok() {
        remap.push(ws_root);
        remap.push("=."); // remap to relative rustc work dir explicitly
    } else {
        remap.push(pkg_root);
        remap.push("=");
        remap.push(unit.pkg.name());
        remap.push("-");
        remap.push(unit.pkg.version().to_string());
    }
    remap
}

/// Remap all paths pointing to `build.build-dir`,
/// i.e., `[BUILD_DIR]/debug/deps/foo-[HASH].dwo` would be remapped to
/// `/cargo/build-dir/debug/deps/foo-[HASH].dwo`
/// (note the `/cargo/build-dir` prefix).
///
/// This covers scenarios like:
///
/// * Build script generated code. For example, a build script may call `file!`
///   macros, and the associated crate uses [`include!`] to include the expanded
///   [`file!`] macro in-place via the `OUT_DIR` environment.
/// * On Linux, `DW_AT_GNU_dwo_name` that contains paths to split debuginfo
///   files (dwp and dwo).
fn build_dir_remap(build_runner: &BuildRunner<'_, '_>) -> OsString {
    let build_dir = build_runner.bcx.ws.build_dir();
    let mut remap = OsString::from("--remap-path-prefix=");
    remap.push(build_dir.as_path_unlocked());
    remap.push("=/cargo/build-dir");
    remap
}

/// Generates the `--check-cfg` arguments for the `unit`.
fn check_cfg_args(unit: &Unit) -> Vec<OsString> {
    // The routine below generates the --check-cfg arguments. Our goals here are to
    // enable the checking of conditionals and pass the list of declared features.
    //
    // In the simplified case, it would resemble something like this:
    //
    //   --check-cfg=cfg() --check-cfg=cfg(feature, values(...))
    //
    // but having `cfg()` is redundant with the second argument (as well-known names
    // and values are implicitly enabled when one or more `--check-cfg` argument is
    // passed) so we don't emit it and just pass:
    //
    //   --check-cfg=cfg(feature, values(...))
    //
    // This way, even if there are no declared features, the config `feature` will
    // still be expected, meaning users would get "unexpected value" instead of name.
    // This wasn't always the case, see rust-lang#119930 for some details.

    let gross_cap_estimation = unit.pkg.summary().features().len() * 7 + 25;
    let mut arg_feature = OsString::with_capacity(gross_cap_estimation);

    arg_feature.push("cfg(feature, values(");
    for (i, feature) in unit.pkg.summary().features().keys().enumerate() {
        if i != 0 {
            arg_feature.push(", ");
        }
        arg_feature.push("\"");
        arg_feature.push(feature);
        arg_feature.push("\"");
    }
    arg_feature.push("))");

    // In addition to the package features, we also include the `test` cfg (since
    // compiler-team#785, as to be able to someday apply it conditionally), as well
    // the `docsrs` cfg from the docs.rs service.
    //
    // We include `docsrs` here (in Cargo) instead of rustc, since there is a much closer
    // relationship between Cargo and docs.rs than rustc and docs.rs. In particular, all
    // users of docs.rs use Cargo, but not all users of rustc (like Rust-for-Linux) use docs.rs.

    vec![
        OsString::from("--check-cfg"),
        OsString::from("cfg(docsrs,test)"),
        OsString::from("--check-cfg"),
        arg_feature,
    ]
}

/// Adds LTO related codegen flags.
fn lto_args(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> Vec<OsString> {
    let mut result = Vec::new();
    let mut push = |arg: &str| {
        result.push(OsString::from("-C"));
        result.push(OsString::from(arg));
    };
    match build_runner.lto[unit] {
        lto::Lto::Run(None) => push("lto"),
        lto::Lto::Run(Some(s)) => push(&format!("lto={}", s)),
        lto::Lto::Off => {
            push("lto=off");
            push("embed-bitcode=no");
        }
        lto::Lto::ObjectAndBitcode => {} // this is rustc's default
        lto::Lto::OnlyBitcode => push("linker-plugin-lto"),
        lto::Lto::OnlyObject => push("embed-bitcode=no"),
    }
    result
}

/// Adds dependency-relevant rustc flags and environment variables
/// to the command to execute, such as [`-L`] and [`--extern`].
///
/// [`-L`]: https://doc.rust-lang.org/nightly/rustc/command-line-arguments.html#-l-add-a-directory-to-the-library-search-path
/// [`--extern`]: https://doc.rust-lang.org/nightly/rustc/command-line-arguments.html#--extern-specify-where-an-external-library-is-located
fn build_deps_args(
    cmd: &mut ProcessBuilder,
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
) -> CargoResult<()> {
    let bcx = build_runner.bcx;

    for arg in lib_search_paths(build_runner, unit)? {
        cmd.arg(arg);
    }

    let deps = build_runner.unit_deps(unit);

    // If there is not one linkable target but should, rustc fails later
    // on if there is an `extern crate` for it. This may turn into a hard
    // error in the future (see PR #4797).
    if !deps
        .iter()
        .any(|dep| !dep.unit.mode.is_doc() && dep.unit.target.is_linkable())
    {
        if let Some(dep) = deps.iter().find(|dep| {
            !dep.unit.mode.is_doc() && dep.unit.target.is_lib() && !dep.unit.artifact.is_true()
        }) {
            let dep_name = dep.unit.target.crate_name();
            let name = unit.target.crate_name();
            bcx.gctx.shell().print_report(&[
                Level::WARNING.secondary_title(format!("the package `{dep_name}` provides no linkable target"))
                    .elements([
                        Level::NOTE.message(format!("this might cause `{name}` to fail compilation")),
                        Level::NOTE.message("this warning might turn into a hard error in the future"),
                        Level::HELP.message(format!("consider adding 'dylib' or 'rlib' to key 'crate-type' in `{dep_name}`'s Cargo.toml"))
                    ])
            ], false)?;
        }
    }

    let mut unstable_opts = false;

    // Add `OUT_DIR` environment variables for build scripts
    let first_custom_build_dep = deps.iter().find(|dep| dep.unit.mode.is_run_custom_build());
    if let Some(dep) = first_custom_build_dep {
        let out_dir = if bcx.gctx.cli_unstable().build_dir_new_layout {
            build_runner.files().out_dir_new_layout(&dep.unit)
        } else {
            build_runner.files().build_script_out_dir(&dep.unit)
        };
        cmd.env("OUT_DIR", &out_dir);
    }

    // Adding output directory for each build script
    let is_multiple_build_scripts_enabled = unit
        .pkg
        .manifest()
        .unstable_features()
        .require(Feature::multiple_build_scripts())
        .is_ok();

    if is_multiple_build_scripts_enabled {
        for dep in deps {
            if dep.unit.mode.is_run_custom_build() {
                let out_dir = if bcx.gctx.cli_unstable().build_dir_new_layout {
                    build_runner.files().out_dir_new_layout(&dep.unit)
                } else {
                    build_runner.files().build_script_out_dir(&dep.unit)
                };
                let target_name = dep.unit.target.name();
                let out_dir_prefix = target_name
                    .strip_prefix("build-script-")
                    .unwrap_or(target_name);
                let out_dir_name = format!("{out_dir_prefix}_OUT_DIR");
                cmd.env(&out_dir_name, &out_dir);
            }
        }
    }
    for arg in extern_args(build_runner, unit, &mut unstable_opts)? {
        cmd.arg(arg);
    }

    for (var, env) in artifact::get_env(build_runner, unit, deps)? {
        cmd.env(&var, env);
    }

    // This will only be set if we're already using a feature
    // requiring nightly rust
    if unstable_opts {
        cmd.arg("-Z").arg("unstable-options");
    }

    Ok(())
}

fn add_dep_arg<'a, 'b: 'a>(
    map: &mut BTreeMap<&'a Unit, PathBuf>,
    build_runner: &'b BuildRunner<'b, '_>,
    unit: &'a Unit,
) {
    if map.contains_key(&unit) {
        return;
    }
    map.insert(&unit, build_runner.files().deps_dir(&unit));

    for dep in build_runner.unit_deps(unit) {
        add_dep_arg(map, build_runner, &dep.unit);
    }
}

/// Adds extra rustc flags and environment variables collected from the output
/// of a build-script to the command to execute, include custom environment
/// variables and `cfg`.
fn add_custom_flags(
    cmd: &mut ProcessBuilder,
    build_script_outputs: &BuildScriptOutputs,
    metadata_vec: Option<Vec<UnitHash>>,
) -> CargoResult<()> {
    if let Some(metadata_vec) = metadata_vec {
        for metadata in metadata_vec {
            if let Some(output) = build_script_outputs.get(metadata) {
                for cfg in output.cfgs.iter() {
                    cmd.arg("--cfg").arg(cfg);
                }
                for check_cfg in &output.check_cfgs {
                    cmd.arg("--check-cfg").arg(check_cfg);
                }
                for (name, value) in output.env.iter() {
                    cmd.env(name, value);
                }
            }
        }
    }

    Ok(())
}

/// Generate a list of `-L` arguments
pub fn lib_search_paths(
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
) -> CargoResult<Vec<OsString>> {
    let mut lib_search_paths = Vec::new();
    if build_runner.bcx.gctx.cli_unstable().build_dir_new_layout {
        let mut map = BTreeMap::new();

        // Recursively add all dependency args to rustc process
        add_dep_arg(&mut map, build_runner, unit);

        let paths = map.into_iter().map(|(_, path)| path).sorted_unstable();

        for path in paths {
            let mut deps = OsString::from("dependency=");
            deps.push(path);
            lib_search_paths.extend(["-L".into(), deps]);
        }
    } else {
        let mut deps = OsString::from("dependency=");
        deps.push(build_runner.files().deps_dir(unit));
        lib_search_paths.extend(["-L".into(), deps]);
    }

    // Be sure that the host path is also listed. This'll ensure that proc macro
    // dependencies are correctly found (for reexported macros).
    if !unit.kind.is_host() {
        let mut deps = OsString::from("dependency=");
        deps.push(build_runner.files().host_deps(unit));
        lib_search_paths.extend(["-L".into(), deps]);
    }

    Ok(lib_search_paths)
}

fn is_public_dependency_enabled(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> bool {
    unit.pkg
        .manifest()
        .unstable_features()
        .require(Feature::public_dependency())
        .is_ok()
        || build_runner.bcx.gctx.cli_unstable().public_dependency
}

/// Generates a list of `--extern` arguments.
pub fn extern_args(
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
    unstable_opts: &mut bool,
) -> CargoResult<Vec<OsString>> {
    let mut result = Vec::new();
    let deps = build_runner.unit_deps(unit);

    let no_embed_metadata = build_runner.bcx.gctx.cli_unstable().no_embed_metadata;
    let public_dependency_enabled = is_public_dependency_enabled(build_runner, unit);

    // Closure to add one dependency to `result`.
    let mut link_to = |dep: &UnitDep,
                       extern_crate_name: InternedString,
                       noprelude: bool,
                       nounused: bool|
     -> CargoResult<()> {
        let mut value = OsString::new();
        let mut opts = Vec::new();
        if !dep.public && unit.target.is_lib() && public_dependency_enabled {
            opts.push("priv");
            *unstable_opts = true;
        }
        if noprelude {
            opts.push("noprelude");
            *unstable_opts = true;
        }
        if nounused {
            opts.push("nounused");
            *unstable_opts = true;
        }
        if !opts.is_empty() {
            value.push(opts.join(","));
            value.push(":");
        }
        value.push(extern_crate_name.as_str());
        value.push("=");

        let mut pass = |file| {
            let mut value = value.clone();
            value.push(file);
            result.push(OsString::from("--extern"));
            result.push(value);
        };

        let outputs = build_runner.outputs(&dep.unit)?;

        if build_runner.only_requires_rmeta(unit, &dep.unit) || dep.unit.mode.is_check() {
            // Example: rlib dependency for an rlib, rmeta is all that is required.
            let output = outputs
                .iter()
                .find(|output| output.flavor == FileFlavor::Rmeta)
                .expect("failed to find rmeta dep for pipelined dep");
            pass(&output.path);
        } else {
            // Example: a bin needs `rlib` for dependencies, it cannot use rmeta.
            for output in outputs.iter() {
                if output.flavor == FileFlavor::Linkable {
                    pass(&output.path);
                }
                // If we use -Zembed-metadata=no, we also need to pass the path to the
                // corresponding .rmeta file to the linkable artifact, because the
                // normal dependency (rlib) doesn't contain the full metadata.
                else if no_embed_metadata && output.flavor == FileFlavor::Rmeta {
                    pass(&output.path);
                }
            }
        }
        Ok(())
    };

    for dep in deps {
        if dep.unit.target.is_linkable() && !dep.unit.mode.is_doc() {
            link_to(dep, dep.extern_crate_name, dep.noprelude, dep.nounused)?;
        }
    }
    if unit.target.proc_macro() {
        // Automatically import `proc_macro`.
        result.push(OsString::from("--extern"));
        result.push(OsString::from("proc_macro"));
    }

    Ok(result)
}

/// Adds `-C linker=<path>` if specified.
fn add_codegen_linker(
    cmd: &mut ProcessBuilder,
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
    target_applies_to_host: bool,
) {
    let linker = if unit.target.for_host() && !target_applies_to_host {
        build_runner
            .compilation
            .host_linker()
            .map(|s| s.as_os_str())
    } else {
        build_runner
            .compilation
            .target_linker(unit.kind)
            .map(|s| s.as_os_str())
    };

    if let Some(linker) = linker {
        let mut arg = OsString::from("linker=");
        arg.push(linker);
        cmd.arg("-C").arg(arg);
    }
}

/// Adds `-C incremental=<path>`.
fn add_codegen_incremental(
    cmd: &mut ProcessBuilder,
    build_runner: &BuildRunner<'_, '_>,
    unit: &Unit,
) {
    let dir = build_runner.files().incremental_dir(&unit);
    let mut arg = OsString::from("incremental=");
    arg.push(dir.as_os_str());
    cmd.arg("-C").arg(arg);
}

fn envify(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_uppercase())
        .map(|c| if c == '-' { '_' } else { c })
        .collect()
}

/// Configuration of the display of messages emitted by the compiler,
/// e.g. diagnostics, warnings, errors, and message caching.
struct OutputOptions {
    /// What format we're emitting from Cargo itself.
    format: MessageFormat,
    /// Where to write the JSON messages to support playback later if the unit
    /// is fresh. The file is created lazily so that in the normal case, lots
    /// of empty files are not created. If this is None, the output will not
    /// be cached (such as when replaying cached messages).
    cache_cell: Option<(PathBuf, OnceCell<File>)>,
    /// If `true`, display any diagnostics.
    /// Other types of JSON messages are processed regardless
    /// of the value of this flag.
    ///
    /// This is used primarily for cache replay. If you build with `-vv`, the
    /// cache will be filled with diagnostics from dependencies. When the
    /// cache is replayed without `-vv`, we don't want to show them.
    show_diagnostics: bool,
    /// Tracks the number of warnings we've seen so far.
    warnings_seen: usize,
    /// Tracks the number of errors we've seen so far.
    errors_seen: usize,
}

impl OutputOptions {
    fn for_dirty(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> OutputOptions {
        let path = build_runner.files().message_cache_path(unit);
        // Remove old cache, ignore ENOENT, which is the common case.
        drop(fs::remove_file(&path));
        let cache_cell = Some((path, OnceCell::new()));

        let show_diagnostics = true;

        let format = build_runner.bcx.build_config.message_format;

        OutputOptions {
            format,
            cache_cell,
            show_diagnostics,
            warnings_seen: 0,
            errors_seen: 0,
        }
    }

    fn for_fresh(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> OutputOptions {
        let cache_cell = None;

        // We always replay the output cache,
        // since it might contain future-incompat-report messages
        let show_diagnostics = unit.show_warnings(build_runner.bcx.gctx);

        let format = build_runner.bcx.build_config.message_format;

        OutputOptions {
            format,
            cache_cell,
            show_diagnostics,
            warnings_seen: 0,
            errors_seen: 0,
        }
    }
}

/// Cloned and sendable context about the manifest file.
///
/// Sometimes we enrich rustc's errors with some locations in the manifest file; this
/// contains a `Send`-able copy of the manifest information that we need for the
/// enriched errors.
struct ManifestErrorContext {
    /// The path to the manifest.
    path: PathBuf,
    /// The locations of various spans within the manifest.
    spans: Option<toml::Spanned<toml::de::DeTable<'static>>>,
    /// The raw manifest contents.
    contents: Option<String>,
    /// A lookup for all the unambiguous renamings, mapping from the original package
    /// name to the renamed one.
    rename_table: HashMap<InternedString, InternedString>,
    /// A list of targets we're compiling for, to determine which of the `[target.<something>.dependencies]`
    /// tables might be of interest.
    requested_kinds: Vec<CompileKind>,
    /// A list of all the collections of cfg values, one collection for each target, to determine
    /// which of the `[target.'cfg(...)'.dependencies]` tables might be of interest.
    cfgs: Vec<Vec<Cfg>>,
    host_name: InternedString,
    /// Cargo's working directory (for printing out a more friendly manifest path).
    cwd: PathBuf,
    /// Terminal width for formatting diagnostics.
    term_width: usize,
}

fn on_stdout_line(
    state: &JobState<'_, '_>,
    line: &str,
    _package_id: PackageId,
    _target: &Target,
) -> CargoResult<()> {
    state.stdout(line.to_string())?;
    Ok(())
}

fn on_stderr_line(
    state: &JobState<'_, '_>,
    line: &str,
    package_id: PackageId,
    manifest: &ManifestErrorContext,
    target: &Target,
    options: &mut OutputOptions,
) -> CargoResult<()> {
    if on_stderr_line_inner(state, line, package_id, manifest, target, options)? {
        // Check if caching is enabled.
        if let Some((path, cell)) = &mut options.cache_cell {
            // Cache the output, which will be replayed later when Fresh.
            let f = cell.try_borrow_mut_with(|| paths::create(path))?;
            debug_assert!(!line.contains('\n'));
            f.write_all(line.as_bytes())?;
            f.write_all(&[b'\n'])?;
        }
    }
    Ok(())
}

/// Returns true if the line should be cached.
fn on_stderr_line_inner(
    state: &JobState<'_, '_>,
    line: &str,
    package_id: PackageId,
    manifest: &ManifestErrorContext,
    target: &Target,
    options: &mut OutputOptions,
) -> CargoResult<bool> {
    // We primarily want to use this function to process JSON messages from
    // rustc. The compiler should always print one JSON message per line, and
    // otherwise it may have other output intermingled (think RUST_LOG or
    // something like that), so skip over everything that doesn't look like a
    // JSON message.
    if !line.starts_with('{') {
        state.stderr(line.to_string())?;
        return Ok(true);
    }

    let mut compiler_message: Box<serde_json::value::RawValue> = match serde_json::from_str(line) {
        Ok(msg) => msg,

        // If the compiler produced a line that started with `{` but it wasn't
        // valid JSON, maybe it wasn't JSON in the first place! Forward it along
        // to stderr.
        Err(e) => {
            debug!("failed to parse json: {:?}", e);
            state.stderr(line.to_string())?;
            return Ok(true);
        }
    };

    let count_diagnostic = |level, options: &mut OutputOptions| {
        if level == "warning" {
            options.warnings_seen += 1;
        } else if level == "error" {
            options.errors_seen += 1;
        }
    };

    if let Ok(report) = serde_json::from_str::<FutureIncompatReport>(compiler_message.get()) {
        for item in &report.future_incompat_report {
            count_diagnostic(&*item.diagnostic.level, options);
        }
        state.future_incompat_report(report.future_incompat_report);
        return Ok(true);
    }

    let res = serde_json::from_str::<SectionTiming>(compiler_message.get());
    if let Ok(timing_record) = res {
        state.on_section_timing_emitted(timing_record);
        return Ok(false);
    }

    // Returns `true` if the diagnostic was modified.
    let add_pub_in_priv_diagnostic = |diag: &mut String| -> bool {
        // We are parsing the compiler diagnostic here, as this information isn't
        // currently exposed elsewhere.
        // At the time of writing this comment, rustc emits two different
        // "exported_private_dependencies" errors:
        //  - type `FromPriv` from private dependency 'priv_dep' in public interface
        //  - struct `FromPriv` from private dependency 'priv_dep' is re-exported
        // This regex matches them both. To see if it needs to be updated, grep the rust
        // source for "EXPORTED_PRIVATE_DEPENDENCIES".
        static PRIV_DEP_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new("from private dependency '([A-Za-z0-9-_]+)'").unwrap());
        if let Some(crate_name) = PRIV_DEP_REGEX.captures(diag).and_then(|m| m.get(1))
            && let Some(ref contents) = manifest.contents
            && let Some(span) = manifest.find_crate_span(crate_name.as_str())
        {
            let rel_path = pathdiff::diff_paths(&manifest.path, &manifest.cwd)
                .unwrap_or_else(|| manifest.path.clone())
                .display()
                .to_string();
            let report = [Group::with_title(Level::NOTE.secondary_title(format!(
                "dependency `{}` declared here",
                crate_name.as_str()
            )))
            .element(
                Snippet::source(contents)
                    .path(rel_path)
                    .annotation(AnnotationKind::Context.span(span)),
            )];

            let rendered = Renderer::styled()
                .term_width(manifest.term_width)
                .render(&report);
            diag.push_str(&rendered);
            diag.push('\n');
            return true;
        }
        false
    };

    // Depending on what we're emitting from Cargo itself, we figure out what to
    // do with this JSON message.
    match options.format {
        // In the "human" output formats (human/short) or if diagnostic messages
        // from rustc aren't being included in the output of Cargo's JSON
        // messages then we extract the diagnostic (if present) here and handle
        // it ourselves.
        MessageFormat::Human
        | MessageFormat::Short
        | MessageFormat::Json {
            render_diagnostics: true,
            ..
        } => {
            #[derive(serde::Deserialize)]
            struct CompilerMessage<'a> {
                // `rendered` contains escape sequences, which can't be
                // zero-copy deserialized by serde_json.
                // See https://github.com/serde-rs/json/issues/742
                rendered: String,
                #[serde(borrow)]
                message: Cow<'a, str>,
                #[serde(borrow)]
                level: Cow<'a, str>,
                children: Vec<PartialDiagnostic>,
                code: Option<DiagnosticCode>,
            }

            // A partial rustfix::diagnostics::Diagnostic. We deserialize only a
            // subset of the fields because rustc's output can be extremely
            // deeply nested JSON in pathological cases involving macro
            // expansion. Rustfix's Diagnostic struct is recursive containing a
            // field `children: Vec<Self>`, and it can cause deserialization to
            // hit serde_json's default recursion limit, or overflow the stack
            // if we turn that off. Cargo only cares about the 1 field listed
            // here.
            #[derive(serde::Deserialize)]
            struct PartialDiagnostic {
                spans: Vec<PartialDiagnosticSpan>,
            }

            // A partial rustfix::diagnostics::DiagnosticSpan.
            #[derive(serde::Deserialize)]
            struct PartialDiagnosticSpan {
                suggestion_applicability: Option<Applicability>,
            }

            #[derive(serde::Deserialize)]
            struct DiagnosticCode {
                code: String,
            }

            if let Ok(mut msg) = serde_json::from_str::<CompilerMessage<'_>>(compiler_message.get())
            {
                if msg.message.starts_with("aborting due to")
                    || msg.message.ends_with("warning emitted")
                    || msg.message.ends_with("warnings emitted")
                {
                    // Skip this line; we'll print our own summary at the end.
                    return Ok(true);
                }
                // state.stderr will add a newline
                if msg.rendered.ends_with('\n') {
                    msg.rendered.pop();
                }
                let mut rendered = msg.rendered;
                if options.show_diagnostics {
                    let machine_applicable: bool = msg
                        .children
                        .iter()
                        .map(|child| {
                            child
                                .spans
                                .iter()
                                .filter_map(|span| span.suggestion_applicability)
                                .any(|app| app == Applicability::MachineApplicable)
                        })
                        .any(|b| b);
                    count_diagnostic(&msg.level, options);
                    if msg
                        .code
                        .as_ref()
                        .is_some_and(|c| c.code == "exported_private_dependencies")
                        && options.format != MessageFormat::Short
                    {
                        add_pub_in_priv_diagnostic(&mut rendered);
                    }
                    let lint = msg.code.is_some();
                    state.emit_diag(&msg.level, rendered, lint, machine_applicable)?;
                }
                return Ok(true);
            }
        }

        MessageFormat::Json { ansi, .. } => {
            #[derive(serde::Deserialize, serde::Serialize)]
            struct CompilerMessage<'a> {
                rendered: String,
                #[serde(flatten, borrow)]
                other: std::collections::BTreeMap<Cow<'a, str>, serde_json::Value>,
                code: Option<DiagnosticCode<'a>>,
            }

            #[derive(serde::Deserialize, serde::Serialize)]
            struct DiagnosticCode<'a> {
                code: String,
                #[serde(flatten, borrow)]
                other: std::collections::BTreeMap<Cow<'a, str>, serde_json::Value>,
            }

            if let Ok(mut error) =
                serde_json::from_str::<CompilerMessage<'_>>(compiler_message.get())
            {
                let modified_diag = if error
                    .code
                    .as_ref()
                    .is_some_and(|c| c.code == "exported_private_dependencies")
                {
                    add_pub_in_priv_diagnostic(&mut error.rendered)
                } else {
                    false
                };

                // Remove color information from the rendered string if color is not
                // enabled. Cargo always asks for ANSI colors from rustc. This allows
                // cached replay to enable/disable colors without re-invoking rustc.
                if !ansi {
                    error.rendered = anstream::adapter::strip_str(&error.rendered).to_string();
                }
                if !ansi || modified_diag {
                    let new_line = serde_json::to_string(&error)?;
                    compiler_message = serde_json::value::RawValue::from_string(new_line)?;
                }
            }
        }
    }

    // We always tell rustc to emit messages about artifacts being produced.
    // These messages feed into pipelined compilation, as well as timing
    // information.
    //
    // Look for a matching directive and inform Cargo internally that a
    // metadata file has been produced.
    #[derive(serde::Deserialize)]
    struct ArtifactNotification<'a> {
        #[serde(borrow)]
        artifact: Cow<'a, str>,
    }

    if let Ok(artifact) = serde_json::from_str::<ArtifactNotification<'_>>(compiler_message.get()) {
        trace!("found directive from rustc: `{}`", artifact.artifact);
        if artifact.artifact.ends_with(".rmeta") {
            debug!("looks like metadata finished early!");
            state.rmeta_produced();
        }
        return Ok(false);
    }

    #[derive(serde::Deserialize)]
    struct UnusedExterns {
        unused_extern_names: std::collections::BTreeSet<InternedString>,
    }
    if let Ok(uext) = serde_json::from_str::<UnusedExterns>(compiler_message.get()) {
        trace!(
            "obtained unused externs list from rustc: `{:?}`",
            uext.unused_extern_names
        );
        state.unused_externs(uext.unused_extern_names);
        return Ok(true);
    }

    // And failing all that above we should have a legitimate JSON diagnostic
    // from the compiler, so wrap it in an external Cargo JSON message
    // indicating which package it came from and then emit it.

    if !options.show_diagnostics {
        return Ok(true);
    }

    #[derive(serde::Deserialize)]
    struct CompilerMessage<'a> {
        #[serde(borrow)]
        message: Cow<'a, str>,
        #[serde(borrow)]
        level: Cow<'a, str>,
    }

    if let Ok(msg) = serde_json::from_str::<CompilerMessage<'_>>(compiler_message.get()) {
        if msg.message.starts_with("aborting due to")
            || msg.message.ends_with("warning emitted")
            || msg.message.ends_with("warnings emitted")
        {
            // Skip this line; we'll print our own summary at the end.
            return Ok(true);
        }
        count_diagnostic(&msg.level, options);
    }

    let msg = machine_message::FromCompiler {
        package_id: package_id.to_spec(),
        manifest_path: &manifest.path,
        target,
        message: compiler_message,
    }
    .to_json_string();

    // Switch json lines from rustc/rustdoc that appear on stderr to stdout
    // instead. We want the stdout of Cargo to always be machine parseable as
    // stderr has our colorized human-readable messages.
    state.stdout(msg)?;
    Ok(true)
}

impl ManifestErrorContext {
    fn new(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> ManifestErrorContext {
        let mut duplicates = HashSet::new();
        let mut rename_table = HashMap::new();

        for dep in build_runner.unit_deps(unit) {
            let unrenamed_id = dep.unit.pkg.package_id().name();
            if duplicates.contains(&unrenamed_id) {
                continue;
            }
            match rename_table.entry(unrenamed_id) {
                std::collections::hash_map::Entry::Occupied(occ) => {
                    occ.remove_entry();
                    duplicates.insert(unrenamed_id);
                }
                std::collections::hash_map::Entry::Vacant(vac) => {
                    vac.insert(dep.extern_crate_name);
                }
            }
        }

        let bcx = build_runner.bcx;
        ManifestErrorContext {
            path: unit.pkg.manifest_path().to_owned(),
            spans: unit.pkg.manifest().document().cloned(),
            contents: unit.pkg.manifest().contents().map(String::from),
            requested_kinds: bcx.target_data.requested_kinds().to_owned(),
            host_name: bcx.rustc().host,
            rename_table,
            cwd: path_args(build_runner.bcx.ws, unit).1,
            cfgs: bcx
                .target_data
                .requested_kinds()
                .iter()
                .map(|k| bcx.target_data.cfg(*k).to_owned())
                .collect(),
            term_width: bcx
                .gctx
                .shell()
                .err_width()
                .diagnostic_terminal_width()
                .unwrap_or(cargo_util_terminal::report::renderer::DEFAULT_TERM_WIDTH),
        }
    }

    fn requested_target_names(&self) -> impl Iterator<Item = &str> {
        self.requested_kinds.iter().map(|kind| match kind {
            CompileKind::Host => &self.host_name,
            CompileKind::Target(target) => target.short_name(),
        })
    }

    /// Find a span for the dependency that specifies this unrenamed crate, if it's unique.
    ///
    /// rustc diagnostics (at least for public-in-private) mention the un-renamed
    /// crate: if you have `foo = { package = "bar" }`, the rustc diagnostic will
    /// say "bar".
    ///
    /// This function does its best to find a span for "bar", but it could fail if
    /// there are multiple candidates:
    ///
    /// ```toml
    /// foo = { package = "bar" }
    /// baz = { path = "../bar", package = "bar" }
    /// ```
    fn find_crate_span(&self, unrenamed: &str) -> Option<Range<usize>> {
        let Some(ref spans) = self.spans else {
            return None;
        };

        let orig_name = self.rename_table.get(unrenamed)?.as_str();

        if let Some((k, v)) = get_key_value(&spans, &["dependencies", orig_name]) {
            // We make some effort to find the unrenamed text: in
            //
            // ```
            // foo = { package = "bar" }
            // ```
            //
            // we try to find the "bar", but fall back to "foo" if we can't (which might
            // happen if the renaming took place in the workspace, for example).
            if let Some(package) = v.get_ref().as_table().and_then(|t| t.get("package")) {
                return Some(package.span());
            } else {
                return Some(k.span());
            }
        }

        // The dependency could also be in a target-specific table, like
        // [target.x86_64-unknown-linux-gnu.dependencies] or
        // [target.'cfg(something)'.dependencies]. We filter out target tables
        // that don't match a requested target or a requested cfg.
        if let Some(target) = spans
            .as_ref()
            .get("target")
            .and_then(|t| t.as_ref().as_table())
        {
            for (platform, platform_table) in target.iter() {
                match platform.as_ref().parse::<Platform>() {
                    Ok(Platform::Name(name)) => {
                        if !self.requested_target_names().any(|n| n == name) {
                            continue;
                        }
                    }
                    Ok(Platform::Cfg(cfg_expr)) => {
                        if !self.cfgs.iter().any(|cfgs| cfg_expr.matches(cfgs)) {
                            continue;
                        }
                    }
                    Err(_) => continue,
                }

                let Some(platform_table) = platform_table.as_ref().as_table() else {
                    continue;
                };

                if let Some(deps) = platform_table
                    .get("dependencies")
                    .and_then(|d| d.as_ref().as_table())
                {
                    if let Some((k, v)) = deps.get_key_value(orig_name) {
                        if let Some(package) = v.get_ref().as_table().and_then(|t| t.get("package"))
                        {
                            return Some(package.span());
                        } else {
                            return Some(k.span());
                        }
                    }
                }
            }
        }
        None
    }
}

/// Creates a unit of work that replays the cached compiler message.
///
/// Usually used when a job is fresh and doesn't need to recompile.
fn replay_output_cache(
    package_id: PackageId,
    manifest: ManifestErrorContext,
    target: &Target,
    path: PathBuf,
    mut output_options: OutputOptions,
) -> Work {
    let target = target.clone();
    Work::new(move |state| {
        replay_output_cache_file(
            state,
            package_id,
            &manifest,
            &target,
            &path,
            &mut output_options,
        )
    })
}

fn replay_output_cache_file(
    state: &JobState<'_, '_>,
    package_id: PackageId,
    manifest: &ManifestErrorContext,
    target: &Target,
    path: &Path,
    output_options: &mut OutputOptions,
) -> CargoResult<()> {
    if !path.exists() {
        // No cached output, probably didn't emit anything.
        return Ok(());
    }
    // We sometimes have gigabytes of output from the compiler, so avoid
    // loading it all into memory at once, as that can cause OOM where
    // otherwise there would be none.
    let file = paths::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    loop {
        let length = reader.read_line(&mut line)?;
        if length == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(&['\n', '\r'][..]);
        on_stderr_line(state, trimmed, package_id, manifest, target, output_options)?;
        line.clear();
    }
    Ok(())
}

/// Provides a package name with descriptive target information,
/// e.g., '`foo` (bin "bar" test)', '`foo` (lib doctest)'.
fn descriptive_pkg_name(name: &str, target: &Target, mode: &CompileMode) -> String {
    let desc_name = target.description_named();
    let mode = if mode.is_rustc_test() && !(target.is_test() || target.is_bench()) {
        " test"
    } else if mode.is_doc_test() {
        " doctest"
    } else if mode.is_doc() {
        " doc"
    } else {
        ""
    };
    format!("`{name}` ({desc_name}{mode})")
}

/// Applies environment variables from config `[env]` to [`ProcessBuilder`].
pub(crate) fn apply_env_config(
    gctx: &crate::GlobalContext,
    cmd: &mut ProcessBuilder,
) -> CargoResult<()> {
    for (key, value) in gctx.env_config()?.iter() {
        // never override a value that has already been set by cargo
        if cmd.get_envs().contains_key(key) {
            continue;
        }
        cmd.env(key, value);
    }
    Ok(())
}

/// Checks if there are some scrape units waiting to be processed.
fn should_include_scrape_units(bcx: &BuildContext<'_, '_>, unit: &Unit) -> bool {
    unit.mode.is_doc() && bcx.scrape_units.len() > 0 && bcx.ws.unit_needs_doc_scrape(unit)
}

/// Gets the file path of function call information output from `rustdoc`.
fn scrape_output_path(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> CargoResult<PathBuf> {
    assert!(unit.mode.is_doc() || unit.mode.is_doc_scrape());
    build_runner
        .outputs(unit)
        .map(|outputs| outputs[0].path.clone())
}

/// Gets the dep-info file emitted by rustdoc.
fn rustdoc_dep_info_loc(build_runner: &BuildRunner<'_, '_>, unit: &Unit) -> PathBuf {
    let mut loc = build_runner.files().fingerprint_file_path(unit, "");
    loc.set_extension("d");
    loc
}
