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
use std::io::{self, BufRead, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Error};
use cargo_platform::{Cfg, Platform};
use cargo_util_terminal::report::{AnnotationKind, Group, Level, Renderer, Snippet};
use itertools::Itertools;
use portable_atomic::{AtomicU64, Ordering};
use regex::Regex;
use tracing::{debug, instrument, trace};

pub use self::build_config::UserIntent;
pub use self::build_config::{
    ArtifactCacheMaterialization, BuildConfig, CompileMode, MessageFormat,
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
use crate::util::{Filesystem, add_path_args, internal, path_args};

use cargo_util::{ProcessBuilder, ProcessError, paths};
use cargo_util_schemas::manifest::TomlDebugInfo;
use cargo_util_schemas::manifest::TomlTrimPaths;
use cargo_util_schemas::manifest::TomlTrimPathsValue;
use cargo_util_terminal::Verbosity;
use rustfix::diagnostics::Applicability;

const RUSTDOC_CRATE_VERSION_FLAG: &str = "--crate-version";
const ARTIFACT_CACHE_PUBLISH_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_PUBLISH_DELAY_MS";
const ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS";
const ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE";
const ARTIFACT_CACHE_RESTORE_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_DELAY_MS";
const ARTIFACT_CACHE_RESTORE_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_READY_FILE";
const ARTIFACT_CACHE_RESTORE_ADMITTED_DELAY_MS_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_ADMITTED_DELAY_MS";
const ARTIFACT_CACHE_RESTORE_ADMITTED_READY_FILE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_ADMITTED_READY_FILE";
const ARTIFACT_CACHE_KEY_FAILURE_FOR_TESTS: &str = "__CARGO_TEST_ARTIFACT_CACHE_KEY_FAILURE";
const ARTIFACT_CACHE_STORE_FAILURE_AFTER_STAGING_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_STORE_FAILURE_AFTER_STAGING";
const ARTIFACT_CACHE_TRANSIENT_REMOVE_FAILURE_FOR_TESTS: &str =
    "__CARGO_TEST_ARTIFACT_CACHE_TRANSIENT_REMOVE_FAILURE";
const ARTIFACT_CACHE_SIZE_STATE: &str = ".cargo-artifact-cache-size";
const ARTIFACT_CACHE_SIZE_STATE_VERSION: &str = "v1";
pub(super) const ARTIFACT_CACHE_FRESHNESS_STAMP: &str = "artifact-cache-complete.timestamp";
static ARTIFACT_CACHE_PUBLICATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A glorified callback for executing calls to rustc. Rather than calling rustc
/// directly, we'll use an `Executor`, giving clients an opportunity to intercept
/// the build calls.
pub trait Executor: Send + Sync + 'static {
    /// Called after a rustc process invocation is prepared up-front for a given
    /// unit of work (may still be modified for runtime-known dependencies, when
    /// the work is actually executed).
    fn init(&self, _build_runner: &BuildRunner<'_, '_>, _unit: &Unit) {}

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
        false
    }
}

/// A `DefaultExecutor` calls rustc without doing anything else. It is Cargo's
/// default behaviour.
#[derive(Copy, Clone)]
pub struct DefaultExecutor;

impl Executor for DefaultExecutor {
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
    force_rebuild: bool,
) -> CargoResult<()> {
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
            let force = exec.force_rebuild(unit) || force_rebuild;
            let mut job = fingerprint::prepare_target(build_runner, unit, force)?;
            job.before(if job.freshness().is_dirty() {
                let work = if unit.mode.is_doc() || unit.mode.is_doc_scrape() {
                    rustdoc(build_runner, unit)?
                } else {
                    rustc(build_runner, unit, exec)?
                };
                work.then(link_targets(build_runner, unit, false)?)
            } else {
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
        compile(build_runner, jobs, &dep.unit, exec, false)?;
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

/// Creates a unit of work invoking `rustc` for building the `unit`.
fn rustc(
    build_runner: &mut BuildRunner<'_, '_>,
    unit: &Unit,
    exec: &Arc<dyn Executor>,
) -> CargoResult<Work> {
    let mut rustc = prepare_rustc(build_runner, unit)?;

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

    let dep_info_name =
        if let Some(c_extra_filename) = build_runner.files().metadata(unit).c_extra_filename() {
            format!("{}-{}.d", unit.target.crate_name(), c_extra_filename)
        } else {
            format!("{}.d", unit.target.crate_name())
        };
    let rustc_dep_info_loc = root.join(dep_info_name);
    let dep_info_loc = fingerprint::dep_info_loc(build_runner, unit);

    let mut output_options = OutputOptions::for_dirty(build_runner, unit);
    let package_id = unit.pkg.package_id();
    let target = Target::clone(&unit.target);
    let mode = unit.mode;

    exec.init(build_runner, unit);
    let exec = exec.clone();

    let root_output = build_runner.files().host_dest().map(|v| v.to_path_buf());
    let build_dir = build_runner.bcx.ws.build_dir().into_path_unlocked();
    let pkg_root = unit.pkg.root().to_path_buf();
    let cwd = rustc
        .get_cwd()
        .unwrap_or_else(|| build_runner.bcx.gctx.cwd())
        .to_path_buf();
    let rustc_verbose_version = build_runner.bcx.rustc().verbose_version.clone();
    let rustc_host = build_runner.bcx.rustc().host.to_string();
    let fingerprint_dir = build_runner.files().fingerprint_dir(unit);
    let message_cache_path = build_runner.files().message_cache_path(unit);
    let show_cached_diagnostics = unit.show_warnings(build_runner.bcx.gctx);
    let script_metadatas = build_runner.find_build_script_metadatas(unit);
    let is_local = unit.is_local();
    let artifact = unit.artifact;
    let sbom_files = build_runner.sbom_output_files(unit)?;
    let sbom = build_sbom(build_runner, unit)?;
    let artifact_cache = build_runner
        .bcx
        .build_config
        .artifact_cache
        .clone()
        .filter(|_| {
            unit.target.is_lib()
                && !unit.target.proc_macro()
                && matches!(unit.mode, CompileMode::Build)
                && !unit.pkg.has_custom_build()
                && sbom_files.is_empty()
                && artifact_cache_host_is_supported()
                && artifact_cache_loader_environment_is_modeled(build_runner.bcx.gctx, &rustc)
        });
    let artifact_cache_compiler_identity = artifact_cache
        .as_ref()
        .and_then(|_| build_runner.bcx.rustc().artifact_cache_identity());
    let artifact_cache_identity_witness = artifact_cache
        .as_ref()
        .and_then(|_| build_runner.bcx.rustc().artifact_cache_identity_witness());
    let artifact_cache_compiler_program = build_runner.bcx.rustc().path.clone();
    let artifact_cache_loader_input_paths = artifact_cache
        .as_ref()
        .map(|_| compiler_loader_input_paths(build_runner.bcx.gctx, &rustc, &cwd))
        .unwrap_or_default();
    let artifact_cache = artifact_cache.filter(|_| {
        artifact_cache_loader_input_paths_are_modeled(&artifact_cache_loader_input_paths)
    });
    let artifact_cache_dependency_search_paths = artifact_cache
        .as_ref()
        .map(|_| {
            lib_search_paths(build_runner, unit).map(|args| {
                args.chunks_exact(2)
                    .filter(|pair| pair[0] == OsStr::new("-L"))
                    .map(|pair| pair[1].clone())
                    .collect::<Vec<_>>()
            })
        })
        .transpose()?
        .unwrap_or_default();
    let cache_crate_name = unit.target.crate_name().to_string();

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

        // Record the invocation before reading cache-discovered inputs so edits
        // racing a restore leave the restored output dirty for the next build.
        let timestamp = paths::set_invocation_time(&fingerprint_dir)?;
        let cache_entry = match artifact_cache
            .as_ref()
            .zip(artifact_cache_compiler_identity.as_ref())
            .filter(|_| {
                rlib_action_is_cacheable_with_search_paths(
                    &rustc,
                    &root,
                    &artifact_cache_compiler_program,
                    &rustc_host,
                    &artifact_cache_dependency_search_paths,
                )
            }) {
            Some((cache, compiler_identity)) => match rlib_cache_entry(
                &cache.dir,
                &rustc,
                &build_dir,
                &root,
                &rustc_verbose_version,
                compiler_identity,
                &artifact_cache_dependency_search_paths,
                &artifact_cache_loader_input_paths,
            ) {
                Ok(entry) => Some(entry),
                Err(error) => {
                    debug!("ignoring artifact cache key failure for {cache_crate_name}: {error:#}");
                    None
                }
            },
            None => None,
        };
        let cache_hit = match cache_entry
            .as_ref()
            .zip(artifact_cache.as_ref())
            .zip(artifact_cache_identity_witness.as_ref())
        {
            Some((((entry, loader_inputs_digest), cache), identity_witness)) => {
                match restore_rlib_cache(
                    entry,
                    outputs.as_slice(),
                    &rustc_dep_info_loc,
                    &message_cache_path,
                    &rustc,
                    &cwd,
                    &pkg_root,
                    &root,
                    identity_witness,
                    &artifact_cache_loader_input_paths,
                    loader_inputs_digest,
                    cache.materialization,
                    cache.max_size,
                ) {
                    Ok(cache_hit) => cache_hit,
                    Err(error) => {
                        debug!(
                            "ignoring artifact cache restore failure for {cache_crate_name}: {error:#}"
                        );
                        false
                    }
                }
            }
            None => false,
        };

        if cache_hit {
            debug!("artifact cache hit for {cache_crate_name}");
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
                prepare_materialized_rlib_output_for_write(&output.path)?;

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
                if output.hardlink.is_some() && output.path.exists() {
                    _ = paths::remove_file(&output.path).map_err(|e| {
                        tracing::debug!(
                            "failed to delete previous output file `{:?}`: {e:?}",
                            output.path
                        );
                    });
                }
            }

            state.running(&rustc);
            for file in sbom_files {
                tracing::debug!("writing sbom to {}", file.display());
                let outfile = BufWriter::new(paths::create(&file)?);
                serde_json::to_writer(outfile, &sbom)?;
            }

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

            if let Err(e) = result {
                if let Some(diagnostic) = failed_scrape_diagnostic {
                    state.warning(diagnostic);
                }

                return Err(e);
            }

            // Exec should never return with success *and* generate an error.
            debug_assert_eq!(output_options.errors_seen, 0);
        }

        if rustc_dep_info_loc.exists() {
            fingerprint::translate_dep_info(
                &rustc_dep_info_loc,
                &dep_info_loc,
                &cwd,
                &pkg_root,
                &build_dir,
                &rustc,
                // Do not track source files in the fingerprint for registry dependencies.
                is_local,
                &env_config,
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
        //
        // The situation is like this:
        //
        // 1. When build script execution's external dependendies
        //    (rerun-if-changed, rerun-if-env-changed) got updated,
        //    the execution unit reran and got a newer mtime.
        // 2. rustc type-checked the associated crate, though with incremental
        //    compilation, no rmeta regeneration. Its `.rmeta` stays old.
        // 3. Run `cargo check` again. Cargo found build script execution had
        //    a new mtime than existing crate rmeta, so re-checking the crate.
        //    However the check is a no-op (input has no change), so stuck.
        if mode.is_check() {
            for output in outputs.iter() {
                paths::set_file_time_no_err(&output.path, timestamp);
            }
        }

        if artifact_cache.is_some() {
            let stamp = fingerprint_dir.join(ARTIFACT_CACHE_FRESHNESS_STAMP);
            drop(paths::create(&stamp)?);
            paths::set_file_time_no_err(stamp, timestamp);
        }

        if !cache_hit
            && let Some(((entry, loader_inputs_digest), cache)) =
                cache_entry.as_ref().zip(artifact_cache.as_ref())
            && let Some(identity_witness) = artifact_cache_identity_witness.as_ref()
        {
            match store_rlib_cache(
                entry,
                outputs.as_slice(),
                &rustc_dep_info_loc,
                &message_cache_path,
                timestamp,
                &cwd,
                &pkg_root,
                &build_dir,
                &root,
                identity_witness,
                &artifact_cache_loader_input_paths,
                loader_inputs_digest,
                cache.max_size,
            ) {
                Ok(true) => debug!("stored artifact cache entry for {cache_crate_name}"),
                Ok(false) => {}
                Err(error) => {
                    debug!(
                        "ignoring artifact cache store failure for {cache_crate_name}: {error:#}"
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

fn unmodeled_codegen_behavior_flag(arg: &str) -> bool {
    ["profile-use", "profile-sample-use", "llvm-args"]
        .iter()
        .any(|flag| arg == *flag || arg.starts_with(&format!("{flag}=")))
}

fn modeled_sysroot_codegen_backend_flag(arg: &str) -> bool {
    ["codegen-backend=", "codegen_backend="]
        .iter()
        .find_map(|prefix| arg.strip_prefix(prefix))
        .is_some_and(|backend| !backend.contains('.'))
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
        "RUSTC_LOG",
        "RUSTC_LOG_COLOR",
        "RUSTC_LOG_ENTRY_EXIT",
        "RUSTC_LOG_THREAD_IDS",
        "RUSTC_LOG_BACKTRACE",
        "RUSTC_LOG_LINES",
        "RUSTC_LOG_FORMAT_JSON",
        "RUSTC_LOG_OUTPUT_TARGET",
        "RUST_TARGET_PATH",
    ];
    rustc.get_programs().count() == 1
        && action_program == compiler_program
        && !unmodeled_environment
            .iter()
            .any(|key| rustc.get_env(key).is_some())
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
            .all(|emit| matches!(emit, "dep-info,link" | "dep-info,metadata,link"))
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
                || arg == "-o"
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
                        .is_some_and(|arg| !modeled_sysroot_codegen_backend_flag(arg)))
        })
        && !args
            .windows(2)
            .any(|pair| pair[0] == "-Z" && !modeled_sysroot_codegen_backend_flag(&pair[1]))
}

#[cfg(test)]
mod artifact_cache_admission_tests {
    use super::*;

    fn ordinary_rlib_command() -> ProcessBuilder {
        let mut rustc = ProcessBuilder::new("rustc");
        rustc
            .arg("--crate-type")
            .arg("lib")
            .arg("--emit=dep-info,metadata,link")
            .arg("--out-dir")
            .arg("target/debug/deps");
        rustc
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
        for flag in ["profile-use", "profile-sample-use"] {
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

        let mut compact = ordinary_rlib_command();
        compact.arg("-Cllvm-args=-load=/path/to/plugin.dylib");
        assert!(!rlib_action_is_cacheable(
            &compact,
            output_root,
            compiler,
            host
        ));

        let mut split = ordinary_rlib_command();
        split.arg("-C").arg("llvm-args=-load=/path/to/plugin.dylib");
        assert!(!rlib_action_is_cacheable(
            &split,
            output_root,
            compiler,
            host
        ));
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
}

fn rlib_cache_entry(
    cache_root: &Path,
    rustc: &ProcessBuilder,
    build_dir: &Path,
    output_root: &Path,
    rustc_verbose_version: &str,
    compiler_identity: &blake3::Hash,
    modeled_dependency_search_paths: &[OsString],
    loader_input_paths: &[(OsString, PathBuf)],
) -> CargoResult<(PathBuf, blake3::Hash)> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if std::env::var_os(ARTIFACT_CACHE_KEY_FAILURE_FOR_TESTS).is_some() {
        return Err(internal("test-only artifact cache key failure".to_string()));
    }
    let target_profile_root = output_root.parent().unwrap_or(output_root);
    let normalize = |value: &str| {
        value
            .replace(
                &target_profile_root.to_string_lossy().to_string(),
                "/__cargo_artifact_cache_target_profile",
            )
            .replace(
                &build_dir.to_string_lossy().to_string(),
                "/__cargo_artifact_cache_build_dir",
            )
    };
    let normalize_cargo_dylib_path = |value: &OsStr| -> CargoResult<OsString> {
        let mut search_path = env::split_paths(value).collect::<Vec<_>>();
        if let Some(cargo_path) = search_path.first_mut() {
            *cargo_path = PathBuf::from(normalize(&cargo_path.to_string_lossy()));
        }
        paths::join_paths(&search_path, paths::dylib_path_envvar())
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-v6\0");
    hasher.update(b"rustc-verbose-version\0");
    hasher.update(rustc_verbose_version.as_bytes());
    hasher.update(b"\0");
    let program = paths::resolve_executable(Path::new(rustc.get_program()))
        .unwrap_or_else(|_| PathBuf::from(rustc.get_program()));
    hasher.update(b"rustc-command-program\0");
    hasher.update(program.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(b"rustc-command-program-content\0");
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
        if normalize_operand || value.starts_with("-Cincremental=") {
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
            } else {
                hasher.update(value.to_string_lossy().as_bytes());
            }
        }
        hasher.update(b"\0");
    }
    let loader_inputs_digest = compiler_loader_inputs_digest(loader_input_paths)?;
    hasher.update(b"compiler-loader-inputs-content\0");
    hasher.update(loader_inputs_digest.as_bytes());
    hasher.update(b"\0");
    for pair in args.windows(2) {
        if pair[0] != OsStr::new("--extern") {
            continue;
        }
        let value = pair[1].to_string_lossy();
        let Some((_, path)) = value.split_once('=') else {
            continue;
        };
        let path = Path::new(path);
        if path.is_file() {
            hasher.update(b"extern-content\0");
            hasher.update(normalize(&path.to_string_lossy()).as_bytes());
            hasher.update(b"\0");
            hasher.update(&fs::read(path)?);
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
        let path = Path::new(value);
        if path.is_dir() {
            hasher.update(b"generated-input-tree\0");
            hasher.update(key.as_bytes());
            hasher.update(b"\0");
            hash_path_tree(&mut hasher, path, path, None, false)?;
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
        let path = Path::new(path);
        if path.is_dir() {
            hasher.update(b"link-search-input-tree\0");
            hasher.update(normalize(&value).as_bytes());
            hasher.update(b"\0");
            hash_path_tree(&mut hasher, path, path, None, true)?;
        }
    }
    Ok((
        cache_root.join(hasher.finalize().to_hex().as_str()),
        loader_inputs_digest,
    ))
}

fn artifact_cache_host_is_supported() -> bool {
    cfg!(target_os = "linux") || cfg!(target_os = "macos")
}

const CARGO_INJECTED_COMPILER_LOADER_ROOT: &str = "cargo-injected-compiler-loader-root";

fn artifact_cache_loader_environment_is_modeled(
    gctx: &crate::util::GlobalContext,
    rustc: &ProcessBuilder,
) -> bool {
    fn variable_is_modeled(key: &str, value: &OsStr) -> bool {
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

    gctx.env()
        .all(|(key, value)| variable_is_modeled(key, OsStr::new(value)))
        && rustc.get_envs().iter().all(|(key, value)| {
            value
                .as_deref()
                .is_none_or(|value| variable_is_modeled(key, value))
        })
}

fn compiler_loader_input_paths(
    gctx: &crate::util::GlobalContext,
    rustc: &ProcessBuilder,
    rustc_cwd: &Path,
) -> Vec<(OsString, PathBuf)> {
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
        inputs: &mut Vec<(OsString, PathBuf)>,
        key: &str,
        value: &OsStr,
        rustc_cwd: &Path,
    ) {
        inputs.extend(
            env::split_paths(value)
                .map(|path| (OsString::from(key), resolve_path(path, rustc_cwd))),
        );
    }

    let mut inputs = Vec::new();
    let primary = paths::dylib_path_envvar();
    if let Some(value) = rustc.get_env(primary).filter(|value| !value.is_empty()) {
        let mut paths = env::split_paths(&value);
        if let Some(path) = paths.next() {
            inputs.push((
                OsString::from(CARGO_INJECTED_COMPILER_LOADER_ROOT),
                resolve_path(path, rustc_cwd),
            ));
        }
        inputs.extend(paths.map(|path| (OsString::from(primary), resolve_path(path, rustc_cwd))));
    } else if cfg!(target_os = "macos") {
        if let Some(home) = gctx.get_env_os("HOME") {
            inputs.push((
                OsString::from(primary),
                resolve_path(PathBuf::from(home).join("lib"), rustc_cwd),
            ));
        }
        inputs.push((OsString::from(primary), PathBuf::from("/usr/local/lib")));
        inputs.push((OsString::from(primary), PathBuf::from("/usr/lib")));
    }
    if cfg!(target_os = "macos") {
        for key in ["DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"] {
            if let Some(value) = rustc.get_env(key).filter(|value| !value.is_empty()) {
                extend_paths(&mut inputs, key, &value, rustc_cwd);
            }
        }
        inputs.push((OsString::from("PWD"), rustc_cwd.to_path_buf()));
    }
    inputs
}

fn compiler_loader_inputs_digest(
    loader_input_paths: &[(OsString, PathBuf)],
) -> CargoResult<blake3::Hash> {
    // Bind Linux nested-library admission to key creation and publication as
    // well as the early eligibility check, since loader trees can change.
    if !artifact_cache_loader_input_paths_are_modeled(loader_input_paths) {
        anyhow::bail!("Linux compiler loader roots contain nested dynamic libraries");
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-compiler-loader-inputs-v1\0");
    for (source, path) in loader_input_paths {
        hasher.update(source.as_encoded_bytes());
        hasher.update(b"\0");
        if source == OsStr::new(CARGO_INJECTED_COMPILER_LOADER_ROOT) {
            hasher.update(b"/__cargo_artifact_cache_compiler_loader_root");
        } else {
            hasher.update(path.as_os_str().as_encoded_bytes());
        }
        hasher.update(b"\0");
        hash_dynamic_library_inputs(
            &mut hasher,
            path,
            source == OsStr::new(CARGO_INJECTED_COMPILER_LOADER_ROOT),
        )?;
    }
    Ok(hasher.finalize())
}

fn artifact_cache_loader_input_paths_are_modeled(
    loader_input_paths: &[(OsString, PathBuf)],
) -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }

    fn is_dynamic_library(path: &Path) -> bool {
        path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.ends_with(".dylib") || name.ends_with(".dll") || name.contains(".so")
        })
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
            if nested && path.is_file() && is_dynamic_library(&path) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    loader_input_paths.iter().all(|(_, path)| {
        !has_nested_dynamic_library(path, false, &mut HashSet::new()).unwrap_or(true)
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
        hasher.update(b"compiler-loader-search-directory\0");
        hasher.update(
            path.strip_prefix(root)
                .unwrap_or(path)
                .as_os_str()
                .as_encoded_bytes(),
        );
        hasher.update(b"\0");
        if normalize_directory_locations {
            hasher.update(b"/__cargo_artifact_cache_compiler_loader_root/");
            hasher.update(
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .as_os_str()
                    .as_encoded_bytes(),
            );
        } else {
            hasher.update(canonical.as_os_str().as_encoded_bytes());
        }
        hasher.update(b"\0");
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
                hasher.update(b"compiler-loader-search-symlink\0");
                hasher.update(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .as_os_str()
                        .as_encoded_bytes(),
                );
                hasher.update(b"\0");
                hasher.update(fs::read_link(&path)?.as_os_str().as_encoded_bytes());
                hasher.update(b"\0");
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
            let name = path.file_name().map(|name| name.to_string_lossy());
            if !path.is_file()
                || !name.is_some_and(|name| {
                    name.ends_with(".dylib") || name.ends_with(".dll") || name.contains(".so")
                })
            {
                continue;
            }
            hasher.update(b"compiler-loader-search-input\0");
            hasher.update(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .as_os_str()
                    .as_encoded_bytes(),
            );
            hasher.update(b"\0");
            hasher.update(&fs::read(path)?);
            hasher.update(b"\0");
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
) -> CargoResult<()> {
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if excluded_path.is_some_and(|excluded| path.starts_with(excluded))
            || path.file_name() == Some(OsStr::new(".git"))
            || (link_search_input && path.extension() == Some(OsStr::new("dSYM")))
        {
            continue;
        }
        if path.is_dir() {
            hash_path_tree(hasher, root, &path, excluded_path, link_search_input)?;
        } else if path.is_file() {
            hasher.update(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .as_os_str()
                    .as_encoded_bytes(),
            );
            hasher.update(b"\0");
            let bytes = fs::read(&path)?;
            if link_search_input && path.extension() == Some(OsStr::new("a")) {
                hasher.update(&normalize_ar_timestamps(&bytes));
            } else {
                hasher.update(&bytes);
            }
            hasher.update(b"\0");
        }
    }
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
    loader_input_paths: &[(OsString, PathBuf)],
    loader_inputs_digest: &blake3::Hash,
    materialization: ArtifactCacheMaterialization,
    max_size: u64,
) -> CargoResult<bool> {
    if !entry_root.exists() {
        return Ok(false);
    }
    let cache_root = entry_root.parent().unwrap_or(entry_root);
    let Some(_lock) = try_read_lock_rlib_cache_within_limit(cache_root, max_size)? else {
        return Ok(false);
    };
    if !entry_root.exists() {
        return Ok(false);
    }
    delay_rlib_cache_restore_for_tests()?;
    if !identity_witness.is_current() {
        debug!("not restoring artifact cache entry with compiler identity modified after lookup");
        return Ok(false);
    }
    if compiler_loader_inputs_digest(loader_input_paths)? != *loader_inputs_digest {
        debug!(
            "not restoring artifact cache entry with compiler loader inputs modified after lookup"
        );
        return Ok(false);
    }
    let mut entries = fs::read_dir(entry_root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let entry = entry.path();
        if !entry.is_dir()
            || entry
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            || !entry.join("complete").exists()
        {
            continue;
        }
        match verify_rlib_cache_entry(&entry, outputs) {
            Ok(true) => {}
            Ok(false) => {
                debug!("rejecting corrupt artifact cache entry {}", entry.display());
                continue;
            }
            Err(error) => {
                debug!(
                    "rejecting unreadable artifact cache entry {}: {error:#}",
                    entry.display()
                );
                continue;
            }
        }
        match verify_rlib_cache_inputs(&entry, rustc, rustc_cwd) {
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
        let stored_files = entry.join("files");
        for output in outputs {
            let stored = stored_files.join(output.path.file_name().unwrap());
            materialize_rlib_cache_file(&stored, &output.path, materialization)?;
        }
        paths::copy(&entry.join("compiler-messages"), message_cache_path)?;
        let stored_dep_info = entry.join("rustc-dep-info");
        if stored_dep_info.exists() {
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
        return Ok(true);
    }
    Ok(false)
}

fn materialize_rlib_cache_file(
    stored: &Path,
    output: &Path,
    materialization: ArtifactCacheMaterialization,
) -> CargoResult<()> {
    match materialization {
        ArtifactCacheMaterialization::Hardlink => {
            #[cfg(unix)]
            {
                if output.exists() {
                    paths::remove_file(output)?;
                }
                match fs::hard_link(stored, output) {
                    Ok(()) => {
                        debug!(
                            "hardlinked cached artifact {} -> {}",
                            stored.display(),
                            output.display()
                        );
                    }
                    Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                        paths::copy(stored, output)?;
                        debug!(
                            "copied cached artifact across filesystems {} -> {}",
                            stored.display(),
                            output.display()
                        );
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
            }
        }
        ArtifactCacheMaterialization::Copy => {
            if output.exists() {
                paths::remove_file(output)?;
            }
            paths::copy(stored, output)?;
            debug!(
                "copied cached artifact {} -> {}",
                stored.display(),
                output.display()
            );
        }
    }
    Ok(())
}

fn prepare_materialized_rlib_output_for_write(output: &Path) -> CargoResult<()> {
    if !output.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if fs::metadata(output)?.nlink() > 1 {
            paths::remove_file(output)?;
            debug!(
                "detached hardlinked cached artifact before rebuild {}",
                output.display()
            );
        }
    }
    Ok(())
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
    loader_input_paths: &[(OsString, PathBuf)],
    loader_inputs_digest: &blake3::Hash,
    max_size: u64,
) -> CargoResult<bool> {
    if !rlib_cache_inputs_are_supported(rustc_dep_info_loc, rustc_cwd, build_dir, output_root)? {
        debug!("not storing artifact cache entry with generated build-directory inputs");
        return Ok(false);
    }
    delay_rlib_cache_input_digest_for_tests()?;
    if !identity_witness.is_current() {
        debug!(
            "not storing artifact cache entry with compiler identity modified during compilation"
        );
        return Ok(false);
    }
    if compiler_loader_inputs_digest(loader_input_paths)? != *loader_inputs_digest {
        debug!(
            "not storing artifact cache entry with compiler loader inputs modified during compilation"
        );
        return Ok(false);
    }
    let Some(inputs_digest) = rlib_cache_inputs_digest(rustc_dep_info_loc, rustc_cwd)? else {
        debug!("not storing artifact cache entry with unreadable compiler-discovered inputs");
        return Ok(false);
    };
    if rlib_cache_inputs_modified_since(rustc_dep_info_loc, rustc_cwd, invocation_time)? {
        debug!("not storing artifact cache entry with inputs modified during compilation");
        return Ok(false);
    }
    let cache_root = entry_root.parent().unwrap_or(entry_root);
    let Some(_lock) = try_write_lock_rlib_cache(cache_root)? else {
        return Ok(false);
    };
    let mut cache_size = match recorded_rlib_cache_size(cache_root) {
        Some(size) if size <= max_size => size,
        Some(_) | None => reconcile_rlib_cache_size(cache_root, max_size, None)?,
    };
    paths::create_dir_all(entry_root)?;
    let entry = entry_root.join(&inputs_digest);
    if entry.exists() {
        if entry.join("complete").exists()
            && verify_rlib_cache_entry(&entry, outputs).unwrap_or(false)
            && artifact_cache_entry_size(&entry).is_some_and(|size| size <= max_size)
        {
            return Ok(false);
        }
        mark_rlib_cache_size_dirty(cache_root)?;
        quarantine_rlib_cache_entry(&entry)?;
        cache_size = reconcile_rlib_cache_size(cache_root, max_size, None)?;
    }
    mark_rlib_cache_size_dirty(cache_root)?;
    let staging = match staging_rlib_cache_entry(&entry) {
        Ok(staging) => staging,
        Err(error) => {
            write_rlib_cache_size(cache_root, cache_size)?;
            return Err(error);
        }
    };
    let result = (|| -> CargoResult<bool> {
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
            }
        }
        if rustc_dep_info_loc.exists() {
            let stored = staging.join("rustc-dep-info");
            paths::copy(rustc_dep_info_loc, &stored)?;
            append_rlib_cache_manifest(&mut manifest, &staging, &stored)?;
        }
        let stored_messages = staging.join("compiler-messages");
        if message_cache_path.exists() {
            paths::copy(message_cache_path, &stored_messages)?;
        } else {
            paths::write(&stored_messages, b"")?;
        }
        append_rlib_cache_manifest(&mut manifest, &staging, &stored_messages)?;
        paths::write(
            staging.join("inputs.blake3"),
            format!("{inputs_digest}\n").as_bytes(),
        )?;
        append_rlib_cache_manifest(&mut manifest, &staging, &staging.join("inputs.blake3"))?;
        paths::write(
            staging.join("origin-pkg-root"),
            pkg_root.to_string_lossy().as_bytes(),
        )?;
        append_rlib_cache_manifest(&mut manifest, &staging, &staging.join("origin-pkg-root"))?;
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
        let manifest_path = staging.join("manifest.blake3");
        paths::write(&manifest_path, manifest.as_bytes())?;
        let manifest_digest = rlib_cache_digest(&manifest_path)?;
        paths::write(
            staging.join("complete"),
            format!("{manifest_digest}\n").as_bytes(),
        )?;
        let entry_size = artifact_cache_entry_size(&staging).ok_or_else(|| {
            internal(format!(
                "failed to size artifact cache publication {}",
                staging.display()
            ))
        })?;
        if entry_size > max_size {
            paths::remove_dir_all(&staging)?;
            write_rlib_cache_size(cache_root, cache_size)?;
            debug!(
                "not storing artifact cache entry larger than configured maximum: {} > {}",
                entry_size, max_size
            );
            return Ok(false);
        }
        #[expect(
            clippy::disallowed_methods,
            reason = "test-only hook is intentionally outside user configuration"
        )]
        let publish_delay = std::env::var_os(ARTIFACT_CACHE_PUBLISH_DELAY_MS_FOR_TESTS);
        if let Some(delay) =
            publish_delay.and_then(|value| value.to_string_lossy().parse::<u64>().ok())
        {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match fs::rename(&staging, &entry) {
            Ok(()) => {}
            Err(_error) if entry.join("complete").exists() => {
                paths::remove_dir_all(&staging)?;
                reconcile_rlib_cache_size(cache_root, max_size, None)?;
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        }
        cache_size = cache_size.saturating_add(entry_size);
        if cache_size > max_size {
            reconcile_rlib_cache_size(cache_root, max_size, Some(&entry))?;
        } else {
            write_rlib_cache_size(cache_root, cache_size)?;
        }
        Ok(true)
    })();
    if result.is_err() && staging.exists() {
        if let Err(error) = paths::remove_dir_all(&staging) {
            debug!(
                "failed to remove abandoned artifact cache publication {}: {error:#}",
                staging.display()
            );
        } else if let Err(error) = write_rlib_cache_size(cache_root, cache_size) {
            debug!(
                "failed to restore artifact cache size state after removing {}: {error:#}",
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
    let manifest_path = entry.join("manifest.blake3");
    if !manifest_path.exists() {
        return Ok(false);
    }
    let complete_digest = paths::read(&entry.join("complete"))?;
    if rlib_cache_digest(&manifest_path)? != complete_digest.trim() {
        return Ok(false);
    }
    let manifest = paths::read(&manifest_path)?;
    let expected = manifest
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect::<HashMap<_, _>>();
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
    for path in required {
        let relative = path.strip_prefix(entry).unwrap_or(&path).to_string_lossy();
        let Some(expected_digest) = expected.get(relative.as_ref()) else {
            return Ok(false);
        };
        if !path.exists() || rlib_cache_digest(&path)? != *expected_digest {
            return Ok(false);
        }
    }
    Ok(true)
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

fn rlib_cache_inputs_are_supported(
    rustc_dep_info_loc: &Path,
    rustc_cwd: &Path,
    build_dir: &Path,
    output_root: &Path,
) -> CargoResult<bool> {
    if !rustc_dep_info_loc.exists() {
        return Ok(false);
    }
    let depinfo = fingerprint::parse_rustc_dep_info(rustc_dep_info_loc)?;
    Ok(depinfo.files.keys().all(|path| {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            rustc_cwd.join(path)
        };
        !path.starts_with(build_dir) && !path.starts_with(output_root)
    }))
}

fn rlib_cache_inputs_digest(
    rustc_dep_info_loc: &Path,
    rustc_cwd: &Path,
) -> CargoResult<Option<String>> {
    rlib_cache_inputs_digest_with_env(rustc_dep_info_loc, rustc_cwd, |_, value| value.clone())
}

fn rlib_cache_inputs_modified_since(
    rustc_dep_info_loc: &Path,
    rustc_cwd: &Path,
    invocation_time: filetime::FileTime,
) -> CargoResult<bool> {
    let depinfo = fingerprint::parse_rustc_dep_info(rustc_dep_info_loc)?;
    for file in depinfo.files.keys() {
        let path = if file.is_absolute() {
            file.to_path_buf()
        } else {
            rustc_cwd.join(file)
        };
        let Ok(mtime) = paths::mtime(&path) else {
            return Ok(true);
        };
        if mtime > invocation_time {
            return Ok(true);
        }
    }
    Ok(false)
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
    let delay = std::env::var_os(ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS_FOR_TESTS);
    let Some(delay) = delay.and_then(|value| value.to_string_lossy().parse::<u64>().ok()) else {
        return Ok(());
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    std::thread::sleep(Duration::from_millis(delay));
    Ok(())
}

fn delay_rlib_cache_restore_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let delay = std::env::var_os(ARTIFACT_CACHE_RESTORE_DELAY_MS_FOR_TESTS);
    let Some(delay) = delay.and_then(|value| value.to_string_lossy().parse::<u64>().ok()) else {
        return Ok(());
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_RESTORE_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    std::thread::sleep(Duration::from_millis(delay));
    Ok(())
}

fn delay_rlib_cache_restore_admitted_for_tests() -> CargoResult<()> {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let delay = std::env::var_os(ARTIFACT_CACHE_RESTORE_ADMITTED_DELAY_MS_FOR_TESTS);
    let Some(delay) = delay.and_then(|value| value.to_string_lossy().parse::<u64>().ok()) else {
        return Ok(());
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    if let Some(path) = std::env::var_os(ARTIFACT_CACHE_RESTORE_ADMITTED_READY_FILE_FOR_TESTS) {
        paths::write(Path::new(&path), b"ready")?;
    }
    std::thread::sleep(Duration::from_millis(delay));
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

fn staging_rlib_cache_entry(entry: &Path) -> CargoResult<PathBuf> {
    let key = entry.file_name().unwrap_or_default().to_string_lossy();
    let sequence = ARTIFACT_CACHE_PUBLICATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = entry.with_file_name(format!(
        ".{key}.publishing-{}-{sequence}",
        std::process::id()
    ));
    paths::create_dir_all(&staging)?;
    Ok(staging)
}

fn try_read_lock_rlib_cache(cache_root: &Path) -> CargoResult<Option<crate::util::FileLock>> {
    paths::create_dir_all(cache_root)?;
    Filesystem::new(cache_root.to_path_buf())
        .try_open_ro_shared_create(".cargo-artifact-cache-lock")
}

fn try_write_lock_rlib_cache(cache_root: &Path) -> CargoResult<Option<crate::util::FileLock>> {
    paths::create_dir_all(cache_root)?;
    Filesystem::new(cache_root.to_path_buf())
        .try_open_rw_exclusive_create(".cargo-artifact-cache-lock")
}

fn cleanup_abandoned_rlib_cache_transients(cache_root: &Path) {
    #[expect(
        clippy::disallowed_methods,
        reason = "test-only hook is intentionally outside user configuration"
    )]
    let retain_transients_for_tests =
        std::env::var_os(ARTIFACT_CACHE_TRANSIENT_REMOVE_FAILURE_FOR_TESTS).is_some();
    let Ok(entry_roots) = fs::read_dir(cache_root) else {
        return;
    };
    for entry_root in entry_roots.flatten() {
        if !entry_root.path().is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(entry_root.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with('.')
                    && (name.contains(".publishing-") || name.contains(".rejected-"))
            }) {
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
}

fn recorded_rlib_cache_size(cache_root: &Path) -> Option<u64> {
    paths::read(&cache_root.join(ARTIFACT_CACHE_SIZE_STATE))
        .ok()?
        .trim()
        .strip_prefix(&format!("{ARTIFACT_CACHE_SIZE_STATE_VERSION} "))?
        .parse()
        .ok()
}

fn mark_rlib_cache_size_dirty(cache_root: &Path) -> CargoResult<()> {
    paths::write_atomic(&cache_root.join(ARTIFACT_CACHE_SIZE_STATE), b"dirty\n")
}

fn rlib_cache_has_transients(cache_root: &Path) -> bool {
    let Ok(entry_roots) = fs::read_dir(cache_root) else {
        return true;
    };
    for entry_root in entry_roots {
        let Ok(entry_root) = entry_root else {
            return true;
        };
        if !entry_root.path().is_dir() {
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
                name.starts_with('.')
                    && (name.contains(".publishing-") || name.contains(".rejected-"))
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
    paths::write_atomic(
        &cache_root.join(ARTIFACT_CACHE_SIZE_STATE),
        format!("{ARTIFACT_CACHE_SIZE_STATE_VERSION} {size}\n").as_bytes(),
    )
}

fn reconcile_rlib_cache_size(
    cache_root: &Path,
    max_size: u64,
    retained_entry: Option<&Path>,
) -> CargoResult<u64> {
    cleanup_abandoned_rlib_cache_transients(cache_root);
    let size = prune_rlib_cache_entries(cache_root, max_size, retained_entry);
    write_rlib_cache_size(cache_root, size)?;
    Ok(size)
}

fn try_read_lock_rlib_cache_within_limit(
    cache_root: &Path,
    max_size: u64,
) -> CargoResult<Option<crate::util::FileLock>> {
    let Some(lock) = try_read_lock_rlib_cache(cache_root)? else {
        return Ok(None);
    };
    if recorded_rlib_cache_size(cache_root).is_some_and(|size| size <= max_size) {
        delay_rlib_cache_restore_admitted_for_tests()?;
        return Ok(Some(lock));
    }
    drop(lock);
    let Some(_lock) = try_write_lock_rlib_cache(cache_root)? else {
        return Ok(None);
    };
    if !recorded_rlib_cache_size(cache_root).is_some_and(|size| size <= max_size) {
        reconcile_rlib_cache_size(cache_root, max_size, None)?;
    }
    drop(_lock);
    let Some(lock) = try_read_lock_rlib_cache(cache_root)? else {
        return Ok(None);
    };
    if !recorded_rlib_cache_size(cache_root).is_some_and(|size| size <= max_size) {
        return Ok(None);
    }
    delay_rlib_cache_restore_admitted_for_tests()?;
    Ok(Some(lock))
}

fn prune_rlib_cache_entries(
    cache_root: &Path,
    max_size: u64,
    retained_entry: Option<&Path>,
) -> u64 {
    let Ok(entry_roots) = fs::read_dir(cache_root) else {
        return 0;
    };
    let mut completed = Vec::new();
    let mut total_size = 0u64;
    for entry_root in entry_roots.flatten() {
        let entry_root = entry_root.path();
        if !entry_root.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&entry_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir()
                || path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                || !path.join("complete").exists()
            {
                continue;
            }
            let Some(size) = artifact_cache_entry_size(&path) else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            total_size = total_size.saturating_add(size);
            completed.push((modified, size, path));
        }
    }
    if total_size <= max_size {
        return total_size;
    }
    completed.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in completed {
        if total_size <= max_size {
            break;
        }
        if retained_entry.is_some_and(|retained_entry| path == retained_entry) {
            continue;
        }
        if paths::remove_dir_all(&path).is_ok() {
            total_size = total_size.saturating_sub(size);
            if let Some(parent) = path.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    total_size
}

fn artifact_cache_entry_size(path: &Path) -> Option<u64> {
    let entries = fs::read_dir(path).ok()?;
    let mut size = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            size = size.saturating_add(artifact_cache_entry_size(&path)?);
        } else {
            size = size.saturating_add(entry.metadata().ok()?.len());
        }
    }
    Some(size)
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
            paths::link_or_copy(src, dst)?;
            if let Some(ref path) = output.export_path {
                let export_dir = export_dir.as_ref().unwrap();
                paths::create_dir_all(export_dir)?;

                paths::link_or_copy(src, path)?;
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
