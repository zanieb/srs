// TODO
#![allow(unused_variables)]
#![allow(unused)]

use crate::alignment::Alignment;
use crate::alignment::MACHO_PAGE_ALIGNMENT;
use crate::args::ArgumentParser;
use crate::args::CommonArgs;
use crate::args::FILES_PER_GROUP_ENV;
use crate::args::FileWriteMode;
use crate::args::Modifiers;
use crate::args::REFERENCE_LINKER_ENV;
use crate::args::RelocationModel;
use crate::args::VersionMode;
use crate::ensure;
use crate::error::Context;
use crate::error::Result;
use crate::macho::DylibLoadKind;
use crate::platform;
use crate::platform::Args as _;
use crate::platform::Symbol as _;
use crate::save_dir::SaveDir;
use jobserver::Client;
use object::Endian as _;
use object::Endianness;
use object::FileKind;
use object::macho;
use object::read::macho::FatArch as _;
use object::read::macho::MachHeader;
use object::read::macho::MachOFatFile32;
use object::read::macho::MachOFatFile64;
use object::read::macho::Nlist;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use zerocopy::IntoBytes;

const EXPERIMENT_PRIVATE_PERSISTENT_OUTPUT_ENV: &str = "SLD_EXPERIMENT_PRIVATE_PERSISTENT_OUTPUT";
const EXPERIMENT_UNSIGNED_PERSISTENT_OUTPUT_ENV: &str = "SLD_EXPERIMENT_UNSIGNED_PERSISTENT_OUTPUT";

#[derive(Debug)]
pub struct MachOArgs {
    pub(crate) common: super::CommonArgs,

    pub(crate) output: Arc<Path>,
    pub(crate) lib_search_path: Vec<Box<Path>>,
    pending_linked_libraries: Vec<(usize, String)>,
    pending_direct_dylib_symbols: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    pub(crate) extra_dylib_paths: Vec<Vec<u8>>,
    pub(crate) weak_dylib_paths: BTreeSet<Vec<u8>>,
    pub(crate) dylib_symbol_ordinals: BTreeMap<Vec<u8>, u8>,
    pub(crate) install_name: Option<Vec<u8>>,
    pub(crate) sysroot: Option<PathBuf>,
    pub(crate) export_list_path: Option<PathBuf>,
    pub(crate) relocation_model: RelocationModel,
    pub(crate) should_output_executable: bool,
    pub(crate) is_dynamiclib: bool,
    pub(crate) should_adhoc_codesign: bool,
    pub(crate) should_emit_code_signature: bool,
    pub(crate) has_private_persistent_output_contract: bool,
    pub(crate) dead_strip: bool,
    pub(crate) platform_version: MachOPlatformVersion,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct MachOPlatformVersion {
    pub(crate) platform: u32,
    pub(crate) minimum_os: u32,
    pub(crate) sdk: u32,
}

impl MachOArgs {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            common: CommonArgs::from_env()?,
            ..Default::default()
        })
    }
}

impl Default for MachOArgs {
    fn default() -> Self {
        Self {
            common: CommonArgs::default(),

            // TODO: move to CommonArgs
            relocation_model: RelocationModel::NonRelocatable,
            should_output_executable: true,
            is_dynamiclib: false,
            output: Arc::from(Path::new("a.out")),
            lib_search_path: Vec::new(),
            pending_linked_libraries: Vec::new(),
            pending_direct_dylib_symbols: BTreeMap::new(),
            extra_dylib_paths: Vec::new(),
            weak_dylib_paths: BTreeSet::new(),
            dylib_symbol_ordinals: BTreeMap::new(),
            install_name: None,
            sysroot: None,
            export_list_path: None,
            should_adhoc_codesign: false,
            should_emit_code_signature: true,
            has_private_persistent_output_contract: false,
            dead_strip: false,
            platform_version: MachOPlatformVersion {
                platform: object::macho::PLATFORM_MACOS,
                minimum_os: encode_macho_version(11, 0, 0),
                sdk: encode_macho_version(11, 0, 0),
            },
        }
    }
}

struct MachOIncrementalLinkOptions<'a>(&'a MachOArgs);

impl fmt::Debug for MachOIncrementalLinkOptions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let args = self.0;
        let common = args.common.incremental_link_options();
        let mut dylib_symbol_ordinals = args.dylib_symbol_ordinals.iter().collect::<Vec<_>>();
        dylib_symbol_ordinals.sort_by_key(|(left, _)| *left);
        f.debug_struct("MachOArgs")
            .field("common", &common)
            .field("output", &args.output)
            .field("lib_search_path", &args.lib_search_path)
            .field("extra_dylib_paths", &args.extra_dylib_paths)
            .field("weak_dylib_paths", &args.weak_dylib_paths)
            .field("dylib_symbol_ordinals", &dylib_symbol_ordinals)
            .field("install_name", &args.install_name)
            .field("sysroot", &args.sysroot)
            .field("export_list_path", &args.export_list_path)
            .field("relocation_model", &args.relocation_model)
            .field("should_output_executable", &args.should_output_executable)
            .field("is_dynamiclib", &args.is_dynamiclib)
            .field("should_adhoc_codesign", &args.should_adhoc_codesign)
            .field(
                "should_emit_code_signature",
                &args.should_emit_code_signature,
            )
            .field(
                "has_private_persistent_output_contract",
                &args.has_private_persistent_output_contract,
            )
            .field("dead_strip", &args.dead_strip)
            .field("platform_version", &args.platform_version)
            .finish()
    }
}

impl platform::Args for MachOArgs {
    fn parse<S, I>(&mut self, input: I) -> Result
    where
        S: AsRef<str>,
        I: Iterator<Item = S>,
    {
        parse(self, input)
    }

    fn should_strip_debug(&self) -> bool {
        false
    }

    fn should_strip_all(&self) -> bool {
        false
    }

    fn entry_symbol_name<'a>(&'a self, linker_script_entry: Option<&'a [u8]>) -> &'a [u8] {
        // TODO: probably add option
        b"_main"
    }

    fn lib_search_path(&self) -> &[Box<std::path::Path>] {
        &self.lib_search_path
    }

    fn output(&self) -> &std::sync::Arc<std::path::Path> {
        &self.output
    }

    fn common(&self) -> &crate::args::CommonArgs {
        &self.common
    }

    fn common_mut(&mut self) -> &mut crate::args::CommonArgs {
        &mut self.common
    }

    fn should_mmap_output_file(&self, file_write_mode: FileWriteMode) -> bool {
        // A replaced signed output has no bytes to reuse, so avoid mmap and its required
        // macOS signature-cache invalidation after publication.
        !(cfg!(target_os = "macos")
            && self.should_emit_code_signature
            && file_write_mode == FileWriteMode::UnlinkAndReplace)
            && self.common.mmap_output_file
    }

    fn incremental_link_options(&self) -> String {
        format!("{:?}", MachOIncrementalLinkOptions(self))
    }

    fn should_export_all_dynamic_symbols(&self) -> bool {
        false
    }

    fn should_export_dynamic(&self, lib_name: &[u8]) -> bool {
        false
    }

    fn sysroot(&self) -> Option<&Path> {
        self.sysroot.as_deref()
    }

    fn export_list_path(&self) -> Option<&Path> {
        self.export_list_path.as_deref()
    }

    fn export_list_roots_archive_symbols(&self) -> bool {
        true
    }

    fn loadable_segment_alignment(&self) -> crate::alignment::Alignment {
        MACHO_PAGE_ALIGNMENT
    }

    fn should_merge_sections(&self) -> bool {
        // TODO
        true
    }

    fn relocation_model(&self) -> crate::args::RelocationModel {
        self.relocation_model
    }

    fn should_output_executable(&self) -> bool {
        self.should_output_executable && !self.is_dynamiclib
    }

    fn should_patch_changed_inputs_before_loading(&self) -> bool {
        !self.should_emit_code_signature || self.has_private_persistent_output_contract
    }

    fn should_replace_directly_patched_output(&self) -> bool {
        cfg!(target_os = "macos")
            && self.should_emit_code_signature
            && self.has_private_persistent_output_contract
    }

    fn finalize_directly_patched_output(
        &self,
        output: &mut [u8],
        flush_ranges: &mut Vec<std::ops::Range<usize>>,
        should_invalidate_code_signature_cache: bool,
    ) -> Result {
        if self.should_emit_code_signature && self.has_private_persistent_output_contract {
            let code_signature_ranges = crate::macho_writer::refresh_code_signature(
                output,
                flush_ranges,
                should_invalidate_code_signature_cache,
            )?;
            flush_ranges.extend(code_signature_ranges);
        }
        Ok(())
    }

    fn should_snapshot_changed_inputs_while_finalizing_direct_patches(&self) -> bool {
        self.should_emit_code_signature && self.has_private_persistent_output_contract
    }

    fn should_hash_directly_patched_output(&self) -> bool {
        !self.has_private_persistent_output_contract
    }

    fn should_trust_persistent_output_data_identity(&self) -> bool {
        self.has_private_persistent_output_contract
    }

    fn should_retain_output_snapshot(&self) -> bool {
        !self.has_private_persistent_output_contract
    }

    fn should_validate_macho_cstring_patches(&self) -> bool {
        true
    }

    fn should_activate_macho_text_sections(&self) -> bool {
        !self.dead_strip
    }

    fn should_activate_macho_archive_members(&self) -> bool {
        true
    }

    fn should_normalize_rust_archive_patch_inputs(&self) -> bool {
        true
    }

    fn should_publish_incremental_state_in_background(&self) -> bool {
        // Signed changed-input relinks need complete prior section state; unsigned Mach-O
        // incremental links are already the latency-sensitive patch path.
        false
    }

    fn should_allow_object_undefined(&self, _output_kind: crate::output_kind::OutputKind) -> bool {
        // Mach-O links against libSystem by default. We currently model undefined external
        // references as libSystem chained imports.
        true
    }
}

impl MachOArgs {
    fn add_dylib_path(&mut self, path: impl Into<Vec<u8>>, kind: DylibLoadKind) -> Result<u8> {
        let path = path.into();
        let index = if let Some(index) = self
            .extra_dylib_paths
            .iter()
            .position(|existing| existing == &path)
        {
            if matches!(kind, DylibLoadKind::Regular) {
                self.weak_dylib_paths.remove(&path);
            }
            index
        } else {
            if matches!(kind, DylibLoadKind::Weak) {
                self.weak_dylib_paths.insert(path.clone());
            }
            self.extra_dylib_paths.push(path);
            self.extra_dylib_paths.len() - 1
        };
        u8::try_from(index + 2).context("Mach-O dylib ordinal exceeds u8")
    }

    fn add_framework(&mut self, framework: &str, kind: DylibLoadKind) -> Result {
        self.add_dylib_path(framework_dylib_path(framework), kind)?;
        Ok(())
    }

    fn add_linked_library(&mut self, library: &str) -> Result {
        match library {
            "System" | "c" | "m" => {}
            "objc" => {
                self.add_dylib_path(b"/usr/lib/libobjc.A.dylib".to_vec(), DylibLoadKind::Regular)?;
            }
            "iconv" => {
                self.add_dylib_path(
                    b"/usr/lib/libiconv.2.dylib".to_vec(),
                    DylibLoadKind::Regular,
                )?;
            }
            "c++" => {
                self.add_dylib_path(b"/usr/lib/libc++.1.dylib".to_vec(), DylibLoadKind::Regular)?;
            }
            "z" => {
                self.add_dylib_path(b"/usr/lib/libz.1.dylib".to_vec(), DylibLoadKind::Regular)?;
            }
            "bz2" => {
                self.add_dylib_path(
                    b"/usr/lib/libbz2.1.0.dylib".to_vec(),
                    DylibLoadKind::Regular,
                )?;
            }
            _ => {
                self.pending_linked_libraries
                    .push((self.extra_dylib_paths.len(), library.to_owned()));
            }
        }
        Ok(())
    }

    fn resolve_pending_linked_libraries(&mut self) -> Result {
        let mut resolved_libraries = Vec::new();
        for (dylib_index, library) in std::mem::take(&mut self.pending_linked_libraries) {
            let dylib_name = format!("lib{library}.dylib");
            let dylib_path = self
                .lib_search_path
                .iter()
                .map(|directory| directory.join(&dylib_name))
                .find(|path| path.is_file());
            if let Some(path) = dylib_path {
                resolved_libraries.push((dylib_index, path));
            } else {
                self.warn_unsupported(&format!("-l{library}"))?;
            }
        }
        for (dylib_index, path) in resolved_libraries.into_iter().rev() {
            self.add_direct_dylib(
                path.to_str()
                    .with_context(|| format!("non-UTF-8 dylib path `{}`", path.display()))?,
                Some(dylib_index),
            )?;
        }
        self.finalize_direct_dylib_symbols()?;
        Ok(())
    }

    fn add_direct_dylib(&mut self, path: &str, dylib_index: Option<usize>) -> Result {
        self.common_mut().save_dir.handle_file(path);

        let metadata = read_direct_dylib_metadata(Path::new(path))?;
        let path = path.as_bytes().to_vec();
        self.pending_direct_dylib_symbols
            .entry(path.clone())
            .or_insert(metadata.exported_symbols);
        // Use the path that rustc/clang passed to us as the load command. Most dylibs also carry
        // an install name, but direct Rust dylib inputs often use @rpath install names and rely on
        // the driver environment to make the original path available at runtime.
        if let Some(dylib_index) = dylib_index {
            self.insert_dylib_path(path, dylib_index);
        } else {
            self.add_dylib_path(path, DylibLoadKind::Regular)?;
        }

        Ok(())
    }

    fn insert_dylib_path(&mut self, path: Vec<u8>, dylib_index: usize) {
        self.weak_dylib_paths.remove(&path);
        if let Some(existing_index) = self
            .extra_dylib_paths
            .iter()
            .position(|existing| existing == &path)
        {
            if existing_index < dylib_index {
                return;
            }
            self.extra_dylib_paths.remove(existing_index);
        }
        self.extra_dylib_paths
            .insert(dylib_index.min(self.extra_dylib_paths.len()), path);
    }

    fn finalize_direct_dylib_symbols(&mut self) -> Result {
        let pending_symbols = std::mem::take(&mut self.pending_direct_dylib_symbols);
        for (index, path) in self.extra_dylib_paths.iter().enumerate() {
            let Some(exported_symbols) = pending_symbols.get(path) else {
                continue;
            };
            let ordinal = u8::try_from(index + 2).context("Mach-O dylib ordinal exceeds u8")?;
            for symbol_name in exported_symbols {
                self.dylib_symbol_ordinals
                    .entry(symbol_name.clone())
                    .or_insert(ordinal);
            }
        }
        Ok(())
    }
}

// Parse the supplied input arguments, which should not include the program name.
pub(crate) fn parse<S: AsRef<str>, I: Iterator<Item = S>>(
    args: &mut MachOArgs,
    mut input: I,
) -> Result {
    let mut modifier_stack = vec![Modifiers::default()];

    let arg_parser = setup_argument_parser();
    while let Some(arg) = input.next() {
        let arg = arg.as_ref();

        if handle_ld64_multi_arg(args, arg, &mut input)? {
            continue;
        }

        if is_direct_dylib_arg(arg) {
            args.add_direct_dylib(arg, None)?;
            continue;
        }

        arg_parser.handle_argument(args, &mut modifier_stack, arg, &mut input)?;
    }

    args.resolve_pending_linked_libraries()?;

    if !args.common.unrecognized_options.is_empty() {
        let options_list = args.common.unrecognized_options.join(", ");
        crate::bail!("unrecognized option(s): {}", options_list);
    }

    if !args.common.incremental && args.common.file_write_mode.is_none() {
        args.common.file_write_mode = Some(FileWriteMode::UnlinkAndReplace);
    }

    // Cargo applies the private output environment only to its preserved root output link.
    apply_experimental_persistent_output_policy(
        args,
        std::env::var_os(EXPERIMENT_PRIVATE_PERSISTENT_OUTPUT_ENV).is_some(),
        std::env::var_os(EXPERIMENT_UNSIGNED_PERSISTENT_OUTPUT_ENV).is_some(),
    );

    Ok(())
}

fn apply_experimental_persistent_output_policy(
    args: &mut MachOArgs,
    private_output: bool,
    unsigned_output: bool,
) {
    args.has_private_persistent_output_contract = private_output || unsigned_output;
    if args.has_private_persistent_output_contract {
        args.should_adhoc_codesign = false;
    }
    if unsigned_output {
        args.should_emit_code_signature = false;
    }
}

fn setup_argument_parser() -> ArgumentParser<MachOArgs> {
    let mut parser = ArgumentParser::<MachOArgs>::new();

    parser
        .declare_with_param()
        .prefix("L")
        .help("Add directory to library search path")
        .execute(|args, _modifier_stack, value| {
            let path = Path::new(value);
            let dir = args
                .sysroot
                .as_ref()
                .filter(|_| path.is_absolute())
                .and_then(|sysroot| path.strip_prefix("/").ok().map(|p| sysroot.join(p)))
                .unwrap_or_else(|| path.to_owned());
            args.common_mut().save_dir.handle_file(value);
            args.lib_search_path.push(dir.into_boxed_path());
            Ok(())
        });

    parser
        .declare_with_param()
        .prefix("l")
        .help("Link with library")
        .execute(|args, _modifier_stack, value| {
            args.add_linked_library(value)?;
            Ok(())
        });

    parser
        .declare_with_param()
        .long("output")
        .short("o")
        .help("Set the output filename")
        .execute(|args, _modifier_stack, value| {
            args.output = Arc::from(Path::new(value));
            Ok(())
        });
    parser
        .declare_with_optional_param()
        .long("time")
        .help("Show timing information")
        .execute(|args, _modifier_stack, value| {
            args.common.time_phase_options = match value {
                Some(v) => Some(super::parse_time_phase_options(v)?),
                None => Some(Vec::new()),
            };
            Ok(())
        });
    parser
        .declare()
        .long("incremental")
        .help("Enable incremental linking")
        .execute(|args, _modifier_stack| {
            args.common.incremental = true;
            Ok(())
        });
    parser
        .declare()
        .long("no-incremental")
        .help("Disable incremental linking")
        .execute(|args, _modifier_stack| {
            args.common.incremental = false;
            Ok(())
        });
    parser
        .declare_with_param()
        .long("incremental-padding-percent")
        .help("Add this percentage of extra capacity after patchable input sections")
        .execute(|args, _modifier_stack, value| {
            args.common.incremental_padding_percent = value.parse()?;
            Ok(())
        });

    parser
        .declare()
        .long("help")
        .help("Show this help message")
        .execute(|_args, _modifier_stack| {
            use std::io::Write;

            let parser = setup_argument_parser();
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{}", parser.generate_help())?;
            std::process::exit(0);
        });

    parser
        .declare()
        .long("version")
        .help("Show version information and exit")
        .execute(|args, _modifier_stack| {
            args.common.version_mode = VersionMode::ExitAfterPrint;
            Ok(())
        });

    parser
        .declare()
        .short("v")
        .help("Print version and continue linking")
        .execute(|args, _modifier_stack| {
            args.common.version_mode = VersionMode::Verbose;
            Ok(())
        });

    parser
        .declare()
        .long("demangle")
        .help("Enable symbol demangling")
        .execute(|args, _modifier_stack| {
            args.common.demangle = true;
            Ok(())
        });

    parser
        .declare()
        .long("no_demangle")
        .long("no-demangle")
        .help("Disable symbol demangling")
        .execute(|args, _modifier_stack| {
            args.common.demangle = false;
            Ok(())
        });

    parser
        .declare()
        .long("dynamic")
        .help("Write a dynamic executable")
        .execute(|_args, _modifier_stack| Ok(()));

    parser
        .declare_with_param()
        .long("arch")
        .help("Set target architecture")
        .execute(|_args, _modifier_stack, value| {
            ensure!(
                matches!(value, "arm64" | "aarch64"),
                "Only arm64 Mach-O output is currently supported"
            );
            Ok(())
        });

    parser
        .declare_with_param()
        .long("syslibroot")
        .help("Set SDK root")
        .execute(|args, _modifier_stack, value| {
            args.sysroot = Some(PathBuf::from(value));
            Ok(())
        });

    parser
        .declare_with_param()
        .long("lto_library")
        .help("Set LTO library path")
        .execute(|args, _modifier_stack, value| {
            args.common_mut().save_dir.handle_file(value);
            Ok(())
        });

    parser
        .declare()
        .long("no_deduplicate")
        .help("Disable deduplication")
        .execute(|_args, _modifier_stack| Ok(()));

    parser
        .declare()
        .long("adhoc_codesign")
        .long("adhoc-codesign")
        .help("Ad-hoc sign the output executable")
        .execute(|args, _modifier_stack| {
            args.should_adhoc_codesign = true;
            Ok(())
        });

    parser
        .declare()
        .long("no_adhoc_codesign")
        .long("no-adhoc-codesign")
        .help("Do not ad-hoc sign the output executable")
        .execute(|args, _modifier_stack| {
            args.should_adhoc_codesign = false;
            Ok(())
        });

    parser
        .declare()
        .long("validate-output")
        .execute(|args, _modifier_stack| {
            args.common_mut().validate_output = true;
            Ok(())
        });

    parser
        .declare()
        .long("write-layout")
        .execute(|args, _modifier_stack| {
            args.common_mut().write_layout = true;
            Ok(())
        });

    parser
        .declare()
        .long("write-trace")
        .execute(|args, _modifier_stack| {
            args.common_mut().write_trace = true;
            Ok(())
        });

    parser
        .declare_with_param()
        .long("sym-info")
        .help("Show symbol information. Accepts symbol name or ID.")
        .execute(|args, _modifier_stack, value| {
            args.common_mut().sym_info = Some(value.to_owned());
            Ok(())
        });

    parser
        .declare()
        .long("no-fork")
        .execute(|args, _modifier_stack| {
            args.common_mut().should_fork = false;
            Ok(())
        });

    parser
        .declare()
        .long("update-in-place")
        .execute(|args, _modifier_stack| {
            args.common_mut().file_write_mode = Some(FileWriteMode::UpdateInPlace);
            Ok(())
        });

    parser
        .declare()
        .long("no-update-in-place")
        .execute(|args, _modifier_stack| {
            args.common_mut().file_write_mode = Some(FileWriteMode::UnlinkAndReplace);
            Ok(())
        });

    parser
        .declare_with_param()
        .long("threads")
        .execute(|args, _modifier_stack, value| {
            args.common_mut().num_threads = Some(NonZeroUsize::try_from(value.parse::<usize>()?)?);
            Ok(())
        });

    parser
}

fn handle_ld64_multi_arg<S: AsRef<str>, I: Iterator<Item = S>>(
    args: &mut MachOArgs,
    arg: &str,
    input: &mut I,
) -> Result<bool> {
    if let Some(minimum_os) = arg
        .strip_prefix("-mmacosx-version-min=")
        .or_else(|| arg.strip_prefix("--macosx-version-min="))
    {
        args.platform_version.minimum_os = parse_macho_version(minimum_os)?;
        return Ok(true);
    }

    match arg {
        "-flavor" | "--flavor" => {
            let flavor = input.next().context("-flavor requires an argument")?;
            ensure!(
                matches!(flavor.as_ref(), "darwin" | "ld64"),
                "Mach-O parser cannot handle -flavor {}",
                flavor.as_ref()
            );
            Ok(true)
        }
        "-platform_version" | "--platform_version" => {
            let platform = input
                .next()
                .context("-platform_version requires a platform name")?;
            let minimum_os = input
                .next()
                .context("-platform_version requires a minimum OS version")?;
            let sdk = input
                .next()
                .context("-platform_version requires an SDK version")?;
            args.platform_version = MachOPlatformVersion {
                platform: parse_macho_platform(platform.as_ref())?,
                minimum_os: parse_macho_version(minimum_os.as_ref())?,
                sdk: parse_macho_version(sdk.as_ref())?,
            };
            Ok(true)
        }
        "-macosx_version_min" | "--macosx_version_min" => {
            let minimum_os = input
                .next()
                .context("-macosx_version_min requires a minimum OS version")?;
            args.platform_version.minimum_os = parse_macho_version(minimum_os.as_ref())?;
            Ok(true)
        }
        "-mllvm" | "--mllvm" => {
            input.next().context("-mllvm requires an argument")?;
            Ok(true)
        }
        "-undefined" | "--undefined" => {
            let value = input.next().context("-undefined requires an argument")?;
            args.warn_unsupported(&format!("-undefined {}", value.as_ref()))?;
            Ok(true)
        }
        "-framework" | "--framework" => {
            let framework = input.next().context("-framework requires an argument")?;
            args.add_framework(framework.as_ref(), DylibLoadKind::Regular)?;
            Ok(true)
        }
        "-weak_framework" | "--weak_framework" => {
            let framework = input
                .next()
                .context("-weak_framework requires an argument")?;
            args.add_framework(framework.as_ref(), DylibLoadKind::Weak)?;
            Ok(true)
        }
        "-dynamiclib" | "--dynamiclib" | "-dylib" | "--dylib" => {
            args.is_dynamiclib = true;
            args.should_output_executable = false;
            Ok(true)
        }
        "-install_name" | "--install_name" => {
            let value = input.next().context("-install_name requires an argument")?;
            args.install_name = Some(value.as_ref().as_bytes().to_vec());
            Ok(true)
        }
        "-exported_symbols_list" | "--exported_symbols_list" | "-Wl,-exported_symbols_list" => {
            let value = input
                .next()
                .context("-exported_symbols_list requires an argument")?;
            let value = value
                .as_ref()
                .strip_prefix("-Wl,")
                .unwrap_or(value.as_ref());
            args.common_mut().save_dir.handle_file(value);
            args.export_list_path = Some(PathBuf::from(value));
            Ok(true)
        }
        "-ObjC" | "-nodefaultlibs" => Ok(true),
        "-dead_strip" | "--dead_strip" | "-Wl,-dead_strip" => {
            args.dead_strip = true;
            Ok(true)
        }
        _ if arg.starts_with("-Wl,") => handle_wl_arg(args, arg, input),
        _ => Ok(false),
    }
}

fn handle_wl_arg<S: AsRef<str>, I: Iterator<Item = S>>(
    args: &mut MachOArgs,
    arg: &str,
    input: &mut I,
) -> Result<bool> {
    let Some(rest) = arg.strip_prefix("-Wl,") else {
        return Ok(false);
    };
    let mut values = rest.split(',');
    while let Some(value) = values.next() {
        match value {
            "-framework" => {
                let framework = values
                    .next()
                    .context("-Wl,-framework requires an argument")?;
                args.add_framework(framework, DylibLoadKind::Regular)?;
            }
            "-weak_framework" => {
                let framework = values
                    .next()
                    .context("-Wl,-weak_framework requires an argument")?;
                args.add_framework(framework, DylibLoadKind::Weak)?;
            }
            "-install_name" => {
                let value = match values.next() {
                    Some(value) => value.to_owned(),
                    None => {
                        let value = input
                            .next()
                            .context("-Wl,-install_name requires an argument")?;
                        value
                            .as_ref()
                            .strip_prefix("-Wl,")
                            .unwrap_or(value.as_ref())
                            .to_owned()
                    }
                };
                args.install_name = Some(value.as_bytes().to_vec());
            }
            "-exported_symbols_list" => {
                let value = match values.next() {
                    Some(value) => value.to_owned(),
                    None => {
                        let value = input
                            .next()
                            .context("-Wl,-exported_symbols_list requires an argument")?;
                        value
                            .as_ref()
                            .strip_prefix("-Wl,")
                            .unwrap_or(value.as_ref())
                            .to_owned()
                    }
                };
                args.common_mut().save_dir.handle_file(&value);
                args.export_list_path = Some(PathBuf::from(value));
            }
            _ if value.starts_with("-l") && value.len() > 2 => {
                args.add_linked_library(&value[2..])?;
            }
            "-dead_strip" => {
                args.dead_strip = true;
            }
            _ => {}
        }
    }
    Ok(true)
}

fn parse_macho_platform(platform: &str) -> Result<u32> {
    match platform {
        "macos" => Ok(object::macho::PLATFORM_MACOS),
        other => crate::bail!("unsupported Mach-O platform `{other}`"),
    }
}

fn parse_macho_version(version: &str) -> Result<u32> {
    let mut components = version.split('.');
    let major = parse_macho_version_component(version, components.next(), u16::MAX.into())?;
    let minor = parse_macho_version_component(version, components.next(), u8::MAX.into())?;
    let patch = parse_macho_version_component(version, components.next(), u8::MAX.into())?;
    ensure!(
        components.next().is_none(),
        "Mach-O version `{version}` has too many components"
    );
    Ok(encode_macho_version(major, minor, patch))
}

fn parse_macho_version_component(version: &str, component: Option<&str>, max: u32) -> Result<u32> {
    let Some(component) = component else {
        return Ok(0);
    };
    let value = component
        .parse::<u32>()
        .with_context(|| format!("invalid Mach-O version `{version}`"))?;
    ensure!(
        value <= max,
        "Mach-O version `{version}` component `{component}` is too large"
    );
    Ok(value)
}

const fn encode_macho_version(major: u32, minor: u32, patch: u32) -> u32 {
    (major << 16) | (minor << 8) | patch
}

fn framework_dylib_path(framework: &str) -> Vec<u8> {
    let framework = canonical_framework_name(framework);
    let version = match framework {
        "AppKit" | "Foundation" => "C",
        _ => "A",
    };
    format!("/System/Library/Frameworks/{framework}.framework/Versions/{version}/{framework}")
        .into_bytes()
}

fn canonical_framework_name(framework: &str) -> &str {
    match framework {
        "Appkit" | "appkit" => "AppKit",
        other => other,
    }
}

fn is_direct_dylib_arg(arg: &str) -> bool {
    !arg.starts_with('-') && Path::new(arg).extension().is_some_and(|ext| ext == "dylib")
}

#[derive(Debug, Default)]
struct DirectDylibMetadata {
    exported_symbols: BTreeSet<Vec<u8>>,
}

fn read_direct_dylib_metadata(path: &Path) -> Result<DirectDylibMetadata> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read dylib `{}`", path.display()))?;

    direct_dylib_metadata_from_file_bytes(bytes.as_slice())
        .with_context(|| format!("failed to inspect dylib `{}`", path.display()))
}

fn direct_dylib_metadata_from_file_bytes(bytes: &[u8]) -> Result<DirectDylibMetadata> {
    let Ok(kind) = FileKind::parse(bytes) else {
        return Ok(DirectDylibMetadata::default());
    };

    match kind {
        FileKind::MachO64 => direct_dylib_metadata_from_macho_bytes(bytes),
        FileKind::MachOFat32 => {
            let fat = MachOFatFile32::parse(bytes)?;
            for arch in fat.arches() {
                if arch.cputype() == macho::CPU_TYPE_ARM64 {
                    return direct_dylib_metadata_from_macho_bytes(arch.data(bytes)?);
                }
            }
            Ok(DirectDylibMetadata::default())
        }
        FileKind::MachOFat64 => {
            let fat = MachOFatFile64::parse(bytes)?;
            for arch in fat.arches() {
                if arch.cputype() == macho::CPU_TYPE_ARM64 {
                    return direct_dylib_metadata_from_macho_bytes(arch.data(bytes)?);
                }
            }
            Ok(DirectDylibMetadata::default())
        }
        _ => Ok(DirectDylibMetadata::default()),
    }
}

fn direct_dylib_metadata_from_macho_bytes(bytes: &[u8]) -> Result<DirectDylibMetadata> {
    let header = macho::MachHeader64::<Endianness>::parse(bytes, 0)?;
    ensure!(
        header.endian()?.is_little_endian(),
        "only little-endian Mach-O dylibs are currently supported"
    );
    ensure!(
        header.cputype(Endianness::Little) == macho::CPU_TYPE_ARM64,
        "only ARM64 Mach-O dylibs are currently supported"
    );

    if header.filetype(Endianness::Little) != macho::MH_DYLIB {
        return Ok(DirectDylibMetadata::default());
    }

    let mut commands = header.load_commands(Endianness::Little, bytes, 0)?;
    let mut exported_symbols = BTreeSet::new();

    while let Some(command) = commands.next()? {
        if command.cmd() == macho::LC_DYLD_EXPORTS_TRIE {
            let export_command: &macho::LinkeditDataCommand<_> = command.data()?;
            for export in export_command.exports_trie(Endianness::Little, bytes)? {
                exported_symbols.insert(export?.name().to_vec());
            }
        }
        if let Some(symtab_command) = command.symtab()? {
            let symbols =
                symtab_command.symbols::<macho::MachHeader64<_>, _>(Endianness::Little, bytes)?;

            for symbol in symbols.iter() {
                if symbol.has_name()
                    && !symbol.is_local()
                    && !platform::Symbol::is_undefined(symbol)
                    && !symbol.is_hidden()
                {
                    exported_symbols
                        .insert(symbol.name(Endianness::Little, symbols.strings())?.to_vec());
                }
            }
        }
    }

    Ok(DirectDylibMetadata { exported_symbols })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Args as _;

    #[test]
    fn dynamiclib_disables_executable_output() {
        let mut args = MachOArgs::default();

        parse(&mut args, ["-dynamiclib"].into_iter()).unwrap();

        assert!(args.is_dynamiclib);
        assert!(!args.should_output_executable);
        assert!(!args.should_output_executable());
    }

    #[test]
    fn install_name_records_dynamiclib_id_path() {
        let mut args = MachOArgs::default();

        parse(
            &mut args,
            ["-install_name", "@rpath/libsld-custom.dylib"].into_iter(),
        )
        .unwrap();

        assert_eq!(
            args.install_name.as_deref(),
            Some(b"@rpath/libsld-custom.dylib".as_slice())
        );
        assert_eq!(
            crate::macho::id_dylib_path(&args),
            b"@rpath/libsld-custom.dylib"
        );
    }

    #[test]
    fn wl_install_name_records_dynamiclib_id_path() {
        let mut args = MachOArgs::default();

        parse(
            &mut args,
            ["-Wl,-install_name,@rpath/libsld-wl.dylib"].into_iter(),
        )
        .unwrap();

        assert_eq!(
            args.install_name.as_deref(),
            Some(b"@rpath/libsld-wl.dylib".as_slice())
        );
    }

    #[test]
    fn split_wl_install_name_records_dynamiclib_id_path() {
        let mut args = MachOArgs::default();

        parse(
            &mut args,
            ["-Wl,-install_name", "-Wl,@rpath/libsld-split-wl.dylib"].into_iter(),
        )
        .unwrap();

        assert_eq!(
            args.install_name.as_deref(),
            Some(b"@rpath/libsld-split-wl.dylib".as_slice())
        );
    }

    #[test]
    fn weak_framework_records_weak_load_command() {
        let mut args = MachOArgs::default();

        parse(&mut args, ["-weak_framework", "Foundation"].into_iter()).unwrap();

        let foundation = framework_dylib_path("Foundation");
        assert!(args.extra_dylib_paths.contains(&foundation));
        assert!(args.weak_dylib_paths.contains(&foundation));
        let commands = crate::macho::load_dylib_commands(&args).collect::<Vec<_>>();
        assert_eq!(commands[0].kind, DylibLoadKind::Regular);
        assert_eq!(commands[1].path, foundation.as_slice());
        assert_eq!(commands[1].kind, DylibLoadKind::Weak);
    }

    #[test]
    fn regular_framework_overrides_weak_framework() {
        let mut args = MachOArgs::default();

        parse(
            &mut args,
            ["-weak_framework", "Foundation", "-framework", "Foundation"].into_iter(),
        )
        .unwrap();

        let foundation = framework_dylib_path("Foundation");
        assert!(args.extra_dylib_paths.contains(&foundation));
        assert!(!args.weak_dylib_paths.contains(&foundation));
    }

    #[test]
    fn linked_bz2_records_system_dylib_load_command() {
        let mut args = MachOArgs::default();

        parse(&mut args, ["-lbz2"].into_iter()).unwrap();

        let commands = crate::macho::load_dylib_commands(&args).collect::<Vec<_>>();
        assert_eq!(commands[1].path, b"/usr/lib/libbz2.1.0.dylib");
        assert_eq!(commands[1].kind, DylibLoadKind::Regular);
    }

    #[test]
    fn linked_library_finds_dylib_on_search_path() {
        let directory = tempfile::tempdir().unwrap();
        let before_dylib = directory.path().join("libbefore.dylib");
        let direct_dylib = directory.path().join("libdirect.dylib");
        let after_dylib = directory.path().join("libafter.dylib");
        std::fs::write(&before_dylib, synthetic_dylib_with_symbol(b"_before")).unwrap();
        std::fs::write(&direct_dylib, synthetic_dylib_with_symbol(b"_direct")).unwrap();
        std::fs::write(&after_dylib, synthetic_dylib_with_symbol(b"_after")).unwrap();
        let search_path = format!("-L{}", directory.path().display());

        let mut args = MachOArgs::default();
        parse(
            &mut args,
            [
                "-lbefore",
                direct_dylib.to_str().unwrap(),
                "-lafter",
                search_path.as_str(),
            ]
            .into_iter(),
        )
        .unwrap();

        assert_eq!(
            args.extra_dylib_paths,
            vec![
                before_dylib.as_os_str().as_encoded_bytes().to_vec(),
                direct_dylib.as_os_str().as_encoded_bytes().to_vec(),
                after_dylib.as_os_str().as_encoded_bytes().to_vec(),
            ]
        );
        assert_eq!(
            args.dylib_symbol_ordinals.get(b"_before".as_slice()),
            Some(&2)
        );
        assert_eq!(
            args.dylib_symbol_ordinals.get(b"_direct".as_slice()),
            Some(&3)
        );
        assert_eq!(
            args.dylib_symbol_ordinals.get(b"_after".as_slice()),
            Some(&4)
        );
    }

    #[test]
    fn non_incremental_links_unlink_existing_outputs_by_default() {
        let mut args = MachOArgs::default();
        args.common.incremental = false;

        parse(&mut args, std::iter::empty::<&str>()).unwrap();

        assert_eq!(
            args.common.file_write_mode,
            Some(FileWriteMode::UnlinkAndReplace)
        );
    }

    #[test]
    fn incremental_links_keep_update_in_place_fallback_default() {
        let mut args = MachOArgs::default();
        args.common.incremental = false;

        parse(&mut args, ["--incremental"].into_iter()).unwrap();

        assert_eq!(args.common.file_write_mode, None);
    }

    #[test]
    fn explicit_update_in_place_overrides_macho_default() {
        let mut args = MachOArgs::default();
        args.common.incremental = false;

        parse(&mut args, ["--update-in-place"].into_iter()).unwrap();

        assert_eq!(
            args.common.file_write_mode,
            Some(FileWriteMode::UpdateInPlace)
        );
    }

    #[test]
    fn incremental_link_options_include_macho_specific_options() {
        let baseline = incremental_link_options_after(|_| {});

        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.lib_search_path
                    .push(PathBuf::from("/sld/lib").into_boxed_path());
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.extra_dylib_paths
                    .push(b"/usr/lib/libz.1.dylib".to_vec());
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.weak_dylib_paths
                    .insert(b"/System/Library/Frameworks/Foundation.framework/Foundation".to_vec());
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.dylib_symbol_ordinals
                    .insert(b"_zlibVersion".to_vec(), 2);
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.install_name = Some(b"@rpath/libsld-custom.dylib".to_vec());
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.sysroot = Some(PathBuf::from("/sld/SDK"));
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.should_output_executable = false;
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.is_dynamiclib = true;
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.should_adhoc_codesign = !args.should_adhoc_codesign;
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.should_emit_code_signature = !args.should_emit_code_signature;
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.has_private_persistent_output_contract = true;
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.dead_strip = true;
            })
        );
        assert_ne!(
            baseline,
            incremental_link_options_after(|args| {
                args.platform_version.minimum_os = encode_macho_version(13, 0, 0);
            })
        );
    }

    #[test]
    fn macho_args_debug_orders_dylib_symbols_deterministically() {
        let mut forward = MachOArgs::default();
        forward
            .dylib_symbol_ordinals
            .insert(b"_lzma_code".to_vec(), 2);
        forward
            .dylib_symbol_ordinals
            .insert(b"_lzma_end".to_vec(), 2);

        let mut reverse = MachOArgs::default();
        reverse
            .dylib_symbol_ordinals
            .insert(b"_lzma_end".to_vec(), 2);
        reverse
            .dylib_symbol_ordinals
            .insert(b"_lzma_code".to_vec(), 2);

        assert_eq!(format!("{forward:?}"), format!("{reverse:?}"));
    }

    #[test]
    fn default_macho_output_uses_linker_generated_signature() {
        let args = MachOArgs::default();

        assert!(args.should_emit_code_signature);
        assert!(!args.should_adhoc_codesign);
    }

    #[test]
    fn signed_macho_replaced_output_avoids_file_backed_mmap_on_macos() {
        let args = MachOArgs::default();

        assert_eq!(
            args.should_mmap_output_file(FileWriteMode::UnlinkAndReplace),
            !cfg!(target_os = "macos")
        );
        assert!(args.should_mmap_output_file(FileWriteMode::UpdateInPlaceWithFallback));
    }

    #[test]
    fn macho_incremental_state_is_published_synchronously() {
        let mut args = MachOArgs::default();
        assert!(!args.should_patch_changed_inputs_before_loading());
        assert!(!args.should_replace_directly_patched_output());
        assert!(!args.should_snapshot_changed_inputs_while_finalizing_direct_patches());
        assert!(args.should_hash_directly_patched_output());
        assert!(!args.should_trust_persistent_output_data_identity());
        assert!(args.should_retain_output_snapshot());
        assert!(args.should_validate_macho_cstring_patches());
        assert!(args.should_activate_macho_archive_members());
        assert!(!args.should_validate_x86_64_elf_got_relaxation_contexts());
        assert!(args.should_normalize_rust_archive_patch_inputs());
        assert!(!args.should_publish_incremental_state_in_background());

        args.should_emit_code_signature = false;
        assert!(args.should_patch_changed_inputs_before_loading());
        assert!(!args.should_replace_directly_patched_output());
        assert!(!args.should_snapshot_changed_inputs_while_finalizing_direct_patches());
        assert!(args.should_hash_directly_patched_output());
        assert!(!args.should_trust_persistent_output_data_identity());
        assert!(args.should_retain_output_snapshot());
        assert!(args.should_validate_macho_cstring_patches());
        assert!(args.should_activate_macho_archive_members());
        assert!(!args.should_validate_x86_64_elf_got_relaxation_contexts());
        assert!(args.should_normalize_rust_archive_patch_inputs());
        assert!(!args.should_publish_incremental_state_in_background());

        args.dead_strip = true;
        assert!(!args.should_activate_macho_text_sections());
        assert!(args.should_activate_macho_archive_members());
    }

    #[test]
    fn experimental_signed_private_output_policy_retains_embedded_signing() {
        let mut args = MachOArgs::default();
        args.should_adhoc_codesign = true;
        args.should_emit_code_signature = true;

        apply_experimental_persistent_output_policy(&mut args, true, false);
        assert!(!args.should_adhoc_codesign);
        assert!(args.should_emit_code_signature);
        assert_eq!(
            args.should_mmap_output_file(FileWriteMode::UnlinkAndReplace),
            !cfg!(target_os = "macos")
        );
        assert!(args.should_mmap_output_file(FileWriteMode::UpdateInPlaceWithFallback));
        assert!(args.should_patch_changed_inputs_before_loading());
        assert_eq!(
            args.should_replace_directly_patched_output(),
            cfg!(target_os = "macos")
        );
        assert!(args.should_snapshot_changed_inputs_while_finalizing_direct_patches());
        assert!(!args.should_hash_directly_patched_output());
        assert!(args.should_trust_persistent_output_data_identity());
        assert!(!args.should_retain_output_snapshot());
        assert!(!args.should_publish_incremental_state_in_background());
    }

    #[test]
    fn experimental_unsigned_persistent_output_policy_omits_signing() {
        let mut args = MachOArgs::default();
        args.should_adhoc_codesign = true;
        args.should_emit_code_signature = true;

        apply_experimental_persistent_output_policy(&mut args, false, true);
        assert!(!args.should_adhoc_codesign);
        assert!(!args.should_emit_code_signature);
        assert!(args.should_mmap_output_file(FileWriteMode::UnlinkAndReplace));
        assert!(!args.should_replace_directly_patched_output());
        assert!(!args.should_snapshot_changed_inputs_while_finalizing_direct_patches());
        assert!(!args.should_hash_directly_patched_output());
        assert!(args.should_trust_persistent_output_data_identity());
        assert!(!args.should_retain_output_snapshot());
        assert!(!args.should_publish_incremental_state_in_background());
    }

    #[test]
    fn direct_dylib_input_records_export_ordinals() {
        let bytes = synthetic_dylib_with_symbol(b"_sld_exported_symbol");
        let metadata = direct_dylib_metadata_from_macho_bytes(&bytes).unwrap();

        assert_one_export(metadata, b"_sld_exported_symbol");
    }

    #[test]
    fn direct_dylib_input_reads_export_trie() {
        let bytes = synthetic_dylib_with_export_trie(b"_sld_trie_symbol");
        let metadata = direct_dylib_metadata_from_macho_bytes(&bytes).unwrap();

        assert_one_export(metadata, b"_sld_trie_symbol");
    }

    #[test]
    fn direct_dylib_input_reads_universal_arm64_slice() {
        let slice = synthetic_dylib_with_symbol(b"_sld_fat_symbol");
        let bytes = synthetic_fat_with_arm64_slice(&slice);
        let metadata = direct_dylib_metadata_from_file_bytes(&bytes).unwrap();

        assert_one_export(metadata, b"_sld_fat_symbol");
    }

    fn assert_one_export(metadata: DirectDylibMetadata, symbol_name: &[u8]) {
        assert_eq!(metadata.exported_symbols.len(), 1);
        assert!(metadata.exported_symbols.contains(symbol_name));
    }

    fn incremental_link_options_after(mutate: impl FnOnce(&mut MachOArgs)) -> String {
        let mut args = MachOArgs::default();
        mutate(&mut args);
        args.incremental_link_options()
    }

    fn synthetic_dylib_with_symbol(symbol_name: &[u8]) -> Vec<u8> {
        const HEADER_SIZE: usize = 32;
        const DYLIB_COMMAND_SIZE: usize = 24;
        const SYMTAB_COMMAND_SIZE: usize = 24;
        const NLIST_SIZE: usize = 16;

        let install_name = b"@rpath/libsld-test.dylib";
        let id_command_size = (DYLIB_COMMAND_SIZE + install_name.len() + 1)
            .next_multiple_of(crate::macho::MACHO_COMMAND_ALIGNMENT);
        let sizeofcmds = id_command_size + SYMTAB_COMMAND_SIZE;
        let symoff = HEADER_SIZE + sizeofcmds;
        let stroff = symoff + NLIST_SIZE;

        let mut string_table = Vec::with_capacity(symbol_name.len() + 2);
        string_table.push(0);
        string_table.extend_from_slice(symbol_name);
        string_table.push(0);

        let mut bytes = Vec::new();
        push_u32(&mut bytes, macho::MH_MAGIC_64);
        push_u32(&mut bytes, macho::CPU_TYPE_ARM64 as u32);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, macho::MH_DYLIB);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, sizeofcmds as u32);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);

        push_u32(&mut bytes, macho::LC_ID_DYLIB);
        push_u32(&mut bytes, id_command_size as u32);
        push_u32(&mut bytes, DYLIB_COMMAND_SIZE as u32);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes.extend_from_slice(install_name);
        bytes.push(0);
        bytes.resize(HEADER_SIZE + id_command_size, 0);

        push_u32(&mut bytes, macho::LC_SYMTAB);
        push_u32(&mut bytes, SYMTAB_COMMAND_SIZE as u32);
        push_u32(&mut bytes, symoff as u32);
        push_u32(&mut bytes, 1);
        push_u32(&mut bytes, stroff as u32);
        push_u32(&mut bytes, string_table.len() as u32);

        push_u32(&mut bytes, 1);
        bytes.push(macho::N_SECT | macho::N_EXT);
        bytes.push(1);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&string_table);

        bytes
    }

    fn synthetic_dylib_with_export_trie(symbol_name: &[u8]) -> Vec<u8> {
        const HEADER_SIZE: usize = 32;
        const DYLIB_COMMAND_SIZE: usize = 24;
        const LINKEDIT_DATA_COMMAND_SIZE: usize = 16;

        let install_name = b"@rpath/libsld-test.dylib";
        let id_command_size = (DYLIB_COMMAND_SIZE + install_name.len() + 1)
            .next_multiple_of(crate::macho::MACHO_COMMAND_ALIGNMENT);
        let sizeofcmds = id_command_size + LINKEDIT_DATA_COMMAND_SIZE;
        let trie = synthetic_export_trie(symbol_name);
        let trieoff = HEADER_SIZE + sizeofcmds;

        let mut bytes = Vec::new();
        push_u32(&mut bytes, macho::MH_MAGIC_64);
        push_u32(&mut bytes, macho::CPU_TYPE_ARM64 as u32);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, macho::MH_DYLIB);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, sizeofcmds as u32);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);

        push_u32(&mut bytes, macho::LC_ID_DYLIB);
        push_u32(&mut bytes, id_command_size as u32);
        push_u32(&mut bytes, DYLIB_COMMAND_SIZE as u32);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes.extend_from_slice(install_name);
        bytes.push(0);
        bytes.resize(HEADER_SIZE + id_command_size, 0);

        push_u32(&mut bytes, macho::LC_DYLD_EXPORTS_TRIE);
        push_u32(&mut bytes, LINKEDIT_DATA_COMMAND_SIZE as u32);
        push_u32(&mut bytes, trieoff as u32);
        push_u32(&mut bytes, trie.len() as u32);
        bytes.extend_from_slice(&trie);

        bytes
    }

    fn synthetic_export_trie(symbol_name: &[u8]) -> Vec<u8> {
        let child_offset = 2 + symbol_name.len() + 1 + 1;
        assert!(child_offset < 128);

        let mut trie = Vec::new();
        trie.push(0);
        trie.push(1);
        trie.extend_from_slice(symbol_name);
        trie.push(0);
        trie.push(child_offset as u8);
        trie.push(2);
        trie.push(0);
        trie.push(0);
        trie.push(0);
        trie
    }

    fn synthetic_fat_with_arm64_slice(slice: &[u8]) -> Vec<u8> {
        const FAT_SLICE_OFFSET: usize = 0x1000;

        let mut bytes = Vec::new();
        push_be_u32(&mut bytes, macho::FAT_MAGIC);
        push_be_u32(&mut bytes, 1);
        push_be_u32(&mut bytes, macho::CPU_TYPE_ARM64 as u32);
        push_be_u32(&mut bytes, 0);
        push_be_u32(&mut bytes, FAT_SLICE_OFFSET as u32);
        push_be_u32(&mut bytes, slice.len() as u32);
        push_be_u32(&mut bytes, 12);

        bytes.resize(FAT_SLICE_OFFSET, 0);
        bytes.extend_from_slice(slice);
        bytes
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_be_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
}
