use crate::archive::ArchiveEntry;
use crate::archive::ArchiveIterator;
use crate::args::InputSpec;
use crate::error::Context as _;
use crate::error::Result;
use crate::input_data::FileLoader;
use crate::input_data::InputFile;
use crate::input_data::InputRef;
use crate::platform;
use crate::timing_phase;
use crate::verbose_timing_phase;
use hashbrown::HashMap;
use hashbrown::HashSet;
use linker_utils::aarch64;
use linker_utils::elf::RelocationKindInfo;
use linker_utils::elf::RelocationSize;
use linker_utils::loongarch64;
use linker_utils::riscv64;
use linker_utils::x86_64;
use memmap2::MmapOptions;
use object::Object as _;
use object::ObjectSection as _;
use object::ObjectSymbol as _;
use rayon::iter::IndexedParallelIterator as _;
use rayon::iter::IntoParallelIterator as _;
use rayon::iter::IntoParallelRefIterator as _;
use rayon::iter::ParallelIterator as _;
use std::ffi::OsString;
use std::fmt::Write as _;
#[cfg(unix)]
use std::fs::Metadata;
use std::fs::OpenOptions;
use std::hash::Hash as _;
use std::hash::Hasher as _;
use std::io::Read as _;
use std::io::Seek as _;
use std::io::SeekFrom;
use std::io::Write as _;
#[cfg(test)]
use std::num::NonZeroUsize;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const STATE_VERSION: &str = "sld-incremental-state-v38";
const STATE_VERSION_V37: &str = "sld-incremental-state-v37";
const STATE_VERSION_V36: &str = "sld-incremental-state-v36";
const STATE_VERSION_V35: &str = "sld-incremental-state-v35";
const STATE_VERSION_V34: &str = "sld-incremental-state-v34";
const STATE_VERSION_V33: &str = "sld-incremental-state-v33";
const STATE_VERSION_V32: &str = "sld-incremental-state-v32";
const STATE_VERSION_V31: &str = "sld-incremental-state-v31";
const STATE_VERSION_V30: &str = "sld-incremental-state-v30";
const STATE_VERSION_V29: &str = "sld-incremental-state-v29";
const STATE_VERSION_V28: &str = "sld-incremental-state-v28";
const STATE_VERSION_V27: &str = "sld-incremental-state-v27";
const STATE_VERSION_V26: &str = "sld-incremental-state-v26";
const STATE_VERSION_V25: &str = "sld-incremental-state-v25";
const STATE_VERSION_V24: &str = "sld-incremental-state-v24";
const STATE_VERSION_V23: &str = "sld-incremental-state-v23";
const STATE_VERSION_V22: &str = "sld-incremental-state-v22";
const STATE_VERSION_V21: &str = "sld-incremental-state-v21";
const STATE_VERSION_V20: &str = "sld-incremental-state-v20";
const STATE_VERSION_V19: &str = "sld-incremental-state-v19";
const STATE_VERSION_V18: &str = "sld-incremental-state-v18";
const STATE_VERSION_V17: &str = "sld-incremental-state-v17";
const STATE_VERSION_V16: &str = "sld-incremental-state-v16";
const STATE_VERSION_V15: &str = "sld-incremental-state-v15";
const STATE_VERSION_V14: &str = "sld-incremental-state-v14";
const STATE_VERSION_V13: &str = "sld-incremental-state-v13";
const STATE_VERSION_V12: &str = "sld-incremental-state-v12";
const STATE_VERSION_V11: &str = "sld-incremental-state-v11";
const STATE_VERSION_V10: &str = "sld-incremental-state-v10";
const STATE_VERSION_V9: &str = "sld-incremental-state-v9";
const STATE_VERSION_V8: &str = "sld-incremental-state-v8";
const STATE_VERSION_V7: &str = "sld-incremental-state-v7";
const STATE_VERSION_V6: &str = "sld-incremental-state-v6";
const STATE_VERSION_V5: &str = "sld-incremental-state-v5";
const STATE_VERSION_V4: &str = "sld-incremental-state-v4";
const STATE_VERSION_V3: &str = "sld-incremental-state-v3";
const STATE_VERSION_V2: &str = "sld-incremental-state-v2";
const STATE_VERSION_V1: &str = "sld-incremental-state-v1";
const INDEX_FILE: &str = "index";
const LOG_FILE: &str = "log";
const GLOBAL_LOG_FILE: &str = "incremental.log";
const METADATA_UPDATE_FILE: &str = "metadata-update";
const METADATA_UPDATE_VERSION: &str = "sld-incremental-metadata-update-v2";
const METADATA_UPDATE_VERSION_V1: &str = "sld-incremental-metadata-update-v1";
const USER_STATE_DIR_ENV: &str = "SLD_STATE_DIR";
const LOG_INCREMENTAL_LINK_OPTIONS_ENV: &str = "SLD_LOG_INCREMENTAL_LINK_OPTIONS";
const LOG_INCREMENTAL_EXACT_ARGS_ENV: &str = "SLD_LOG_INCREMENTAL_EXACT_ARGS";
const INPUT_SNAPSHOT_DIR: &str = "input-files";
const OUTPUT_SNAPSHOT_FILE: &str = "output";
const DIRECT_PATCH_STANDBY_FILE: &str = "direct-patch-standby";
const DIRECT_PATCH_STANDBY_STATE_FILE: &str = "direct-patch-standby-state";
const DIRECT_PATCH_STANDBY_STATE_VERSION: &str = "sld-direct-patch-standby-v1";
const DIRECT_PATCH_STANDBY_STATE_MAX_LEN: u64 = 1024 * 1024;
const DIRECT_PATCH_STANDBY_MAX_RANGES: usize = 4096;
const COMPRESSED_INPUT_SNAPSHOT_SUFFIX: &str = ".zstd";
const INPUT_SNAPSHOT_COMPRESSION_LEVEL: i32 = 1;
const STABLE_RUSTC_INPUT_DIR: &str = "stable-rustc-inputs";
const STABILIZE_RUSTC_TRANSIENT_INPUTS_ENV: &str = "SLD_STABILIZE_RUSTC_TRANSIENT_INPUTS";
const RUSTC_WORK_PRODUCT_PROVENANCE_ENV: &str = "SLD_RUSTC_WORK_PRODUCT_PROVENANCE";
const RUSTC_WORK_PRODUCT_PROVENANCE_FILE_ENV: &str = "SLD_RUSTC_WORK_PRODUCT_PROVENANCE_FILE";
const RUSTC_WORK_PRODUCT_PROVENANCE_VERSION: &str = "sld-rustc-work-product-provenance-v1";
const RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX: &[u8] = b"rustc-rlib-link-content-v1:";
const RUSTC_RLIB_RAW_OBJECT_DIGESTS_PREFIX: &[u8] = b"rustc-rlib-raw-object-digests-v1:";
const RUSTC_RLIB_LINK_METADATA_MEMBER: &[u8] = b"lib.rmeta-link";
const RUSTC_RLIB_LINK_METADATA_SECTION: &str = ".rmeta-link";
const RUSTC_RLIB_LINK_METADATA_WRAPPER_MAX_LEN: u64 = 16 * 1024 * 1024;
const RUSTC_SERIALIZED_METADATA_END: &[u8] = b"rust-end-file";
const BUILD_ID_HASH_FILE: &str = "build-id-hash";
const UPDATE_MARKER_FILE: &str = "update-in-progress";
const STATE_LOCK_FILE: &str = "state.lock";
const LINK_START_FILE: &str = "link-start";
const SECTIONS_FILE: &str = "sections";
const SECTIONS_FILE_PREFIX: &str = "sections-";
const COMPRESSED_SECTIONS_FILE_PREFIX: &str = "sections-zstd-";
const PUBLISHING_SECTIONS_FILE: &str = "sections-publishing";
const SECTIONS_COMPRESSION_LEVEL: i32 = 1;
const COMPRESSED_MACHO_RESOLUTIONS_FILE_PREFIX: &str = "macho-resolutions-zstd-";
const MACHO_RESOLUTIONS_COMPRESSION_LEVEL: i32 = 1;
const GENERATED_RELA_DYN_GENERAL: &str = "generated:.rela.dyn.general";
const BUILD_ID_HASH_GROUP_CHUNKS: usize = 64;
const BUILD_ID_HASH_GROUP_LEN: usize = blake3::CHUNK_LEN * BUILD_ID_HASH_GROUP_CHUNKS;
const BUILD_ID_HASH_PARALLEL_THRESHOLD: usize = BUILD_ID_HASH_GROUP_LEN * 8;
const PARALLEL_ARCHIVE_PATCH_FINGERPRINT_PREFIX: &str = "parallel-archive-members-v2:";
const ABSENT_FIELD: &str = "-";
const RECORD_TEXT_INTERNER_SHARDS: usize = 64;
const RECORD_BUFFER_SHARDS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SharedText(Arc<str>);

impl SharedText {
    fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl std::ops::Deref for SharedText {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<String> for SharedText {
    fn from(value: String) -> Self {
        Self(Arc::<str>::from(value))
    }
}

impl From<&str> for SharedText {
    fn from(value: &str) -> Self {
        Self(Arc::<str>::from(value))
    }
}

impl std::fmt::Display for SharedText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl PartialEq<String> for SharedText {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<&str> for SharedText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

type InternedInputKey = (usize, Option<(usize, usize)>);
type InternedInputTexts = (SharedText, SharedText);

struct RecordTextInterner {
    values: [Mutex<HashMap<String, SharedText>>; RECORD_TEXT_INTERNER_SHARDS],
    inputs: [Mutex<HashMap<InternedInputKey, InternedInputTexts>>; RECORD_TEXT_INTERNER_SHARDS],
    targets: [Mutex<HashMap<u32, RecordedRelocationTarget>>; RECORD_TEXT_INTERNER_SHARDS],
}

#[derive(Clone)]
struct RecordedRelocationTarget {
    target_name: Option<SharedText>,
    target: Option<(SharedText, SharedText, object::SectionIndex, u64)>,
}

impl Default for RecordTextInterner {
    fn default() -> Self {
        Self {
            values: std::array::from_fn(|_| Mutex::new(HashMap::new())),
            inputs: std::array::from_fn(|_| Mutex::new(HashMap::new())),
            targets: std::array::from_fn(|_| Mutex::new(HashMap::new())),
        }
    }
}

impl RecordTextInterner {
    fn intern(&self, value: String) -> SharedText {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        let shard = hasher.finish() as usize % RECORD_TEXT_INTERNER_SHARDS;
        let mut values = self.values[shard].lock().unwrap();
        if let Some(existing) = values.get(value.as_str()) {
            return existing.clone();
        }
        let shared = SharedText::from(value.clone());
        values.insert(value, shared.clone());
        shared
    }

    fn intern_input(&self, input: InputRef<'_>) -> (SharedText, SharedText) {
        let key = (
            std::ptr::from_ref(input.file) as usize,
            input
                .entry
                .map(|entry| (entry.start_offset, entry.end_offset)),
        );
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let shard = hasher.finish() as usize % RECORD_TEXT_INTERNER_SHARDS;
        if let Some(texts) = self.inputs[shard].lock().unwrap().get(&key).cloned() {
            return texts;
        }
        let texts = (
            self.intern(encode_path(&input.file.filename)),
            self.intern(encode_input_ref(input)),
        );
        self.inputs[shard]
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| texts.clone())
            .clone()
    }

    fn intern_relocation_target<'data>(
        &self,
        target_symbol_id: u32,
        metadata: impl FnOnce() -> Result<(
            Option<String>,
            Option<(InputRef<'data>, object::SectionIndex, u64)>,
        )>,
    ) -> Result<RecordedRelocationTarget> {
        let shard = target_symbol_id as usize % RECORD_TEXT_INTERNER_SHARDS;
        if let Some(existing) = self.targets[shard]
            .lock()
            .unwrap()
            .get(&target_symbol_id)
            .cloned()
        {
            return Ok(existing);
        }
        let (target_name, target) = metadata()?;
        let metadata = RecordedRelocationTarget {
            target_name: target_name.map(|name| self.intern(name)),
            target: target.map(|(target_input, target_section_index, section_offset)| {
                let (target_input_file, target_input_text) = self.intern_input(target_input);
                (
                    target_input_file,
                    target_input_text,
                    target_section_index,
                    section_offset,
                )
            }),
        };
        Ok(self.targets[shard]
            .lock()
            .unwrap()
            .entry(target_symbol_id)
            .or_insert_with(|| metadata.clone())
            .clone())
    }
}

#[derive(Clone, Copy)]
struct DeferredRecordedRelocationTarget<'data> {
    target_name: Option<&'data [u8]>,
    target: Option<(InputRef<'data>, object::SectionIndex, u64)>,
}

struct RecordBuffers<T> {
    values: [Mutex<Vec<T>>; RECORD_BUFFER_SHARDS],
}

impl<T> Default for RecordBuffers<T> {
    fn default() -> Self {
        Self {
            values: std::array::from_fn(|_| Mutex::new(Vec::new())),
        }
    }
}

impl<T> RecordBuffers<T> {
    fn push(&self, value: T) {
        let shard = rayon::current_thread_index().unwrap_or(0) % RECORD_BUFFER_SHARDS;
        self.values[shard].lock().unwrap().push(value);
    }

    fn extend(&self, values: Vec<T>) {
        if values.is_empty() {
            return;
        }
        let shard = rayon::current_thread_index().unwrap_or(0) % RECORD_BUFFER_SHARDS;
        self.values[shard].lock().unwrap().extend(values);
    }

    fn take_all(&self) -> Vec<T> {
        let mut shards = self.take_shards();
        let total_len = shards.iter().map(Vec::len).sum::<usize>();
        let mut records = Vec::with_capacity(total_len);
        for shard in &mut shards {
            records.append(shard);
        }
        records
    }

    fn take_shards(&self) -> Vec<Vec<T>> {
        self.values
            .iter()
            .map(|shard| std::mem::take(&mut *shard.lock().unwrap()))
            .collect()
    }
}

pub(crate) struct PreparedState<'data> {
    mode: IncrementalMode,
    current: CurrentState,
    retained_output_to_restore: Option<FileContentState>,
    reusable_inputs: HashSet<String>,
    previous_sections: HashSet<SectionRecord>,
    previous_relocations: Vec<RelocationRecord>,
    previous_macho_symbol_resolutions_file: Option<String>,
    previous_fdes: Vec<FdeRecord>,
    previous_dynamic_relocations: Vec<DynamicRelocationRecord>,
    current_sections: RecordBuffers<SectionRecord>,
    current_relocations: RecordBuffers<DeferredRelocationRecord<'data>>,
    current_macho_symbol_resolutions: RecordBuffers<MachOSymbolResolutionRecord>,
    current_fdes: RecordBuffers<FdeRecord>,
    current_dynamic_relocations: RecordBuffers<DynamicRelocationRecord>,
    record_texts: RecordTextInterner,
    reused_sections: AtomicUsize,
    prepared_fast_build_id_state: Mutex<Option<BuildIdHashStateAndTree>>,
}

pub(crate) struct PendingStateWrite<'data> {
    state_dir: PathBuf,
    state: PersistedState,
    relocation_shards: Vec<Vec<DeferredRelocationRecord<'data>>>,
    build_id_tree: Option<Vec<[u8; blake3::OUT_LEN]>>,
    deferred_hash_inputs: Option<Vec<&'data InputFile>>,
    reused_sections: usize,
    _lock: Option<IncrementalStateLock>,
}

pub(crate) struct IncrementalStateLock {
    _file: std::fs::File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IncrementalMode {
    Disabled,
    Reuse,
    Relink {
        reason: String,
        can_reuse_unchanged_sections: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentState {
    state_dir: PathBuf,
    args_hash: String,
    link_options_hash: String,
    input_order_hash: String,
    sld_version: String,
    link_start: Option<FileIdentity>,
    input_files: Vec<FileState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistedState {
    args_hash: String,
    link_options_hash: Option<String>,
    input_order_hash: Option<String>,
    sld_version: Option<String>,
    link_start: Option<FileIdentity>,
    output: FileContentState,
    build_id_hashes: Option<BuildIdHashState>,
    input_files: Vec<FileState>,
    sections: Vec<SectionRecord>,
    relocations: Vec<RelocationRecord>,
    macho_symbol_resolutions: Vec<MachOSymbolResolutionRecord>,
    fdes: Vec<FdeRecord>,
    dynamic_relocations: Vec<DynamicRelocationRecord>,
    sections_file: Option<String>,
    macho_symbol_resolutions_file: Option<String>,
    patch_records_file: Option<String>,
    patch_record_locations: Vec<PatchRecordLocation>,
    raw_patch_record_locations: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchRecordLocation {
    input_file: String,
    offset: u64,
    len: u64,
    hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildIdHashState {
    output_len: u64,
    nodes: usize,
    tree_hash: Option<String>,
}

type BuildIdHashStateAndTree = (Option<BuildIdHashState>, Option<Vec<[u8; blake3::OUT_LEN]>>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileState {
    path: String,
    content: FileContentState,
    snapshot_identity: Option<FileIdentity>,
    patch: Option<FilePatchState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePatchState {
    fingerprint: String,
    archive_member_set_proof: Option<ArchiveMemberSetProof>,
    archive_member_patch_fingerprints: Option<Vec<ArchiveMemberPatchFingerprint>>,
    sections: Vec<FilePatchSectionState>,
    raw_sections: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveMemberPatchFingerprint {
    archive_member_index: usize,
    identifier: Vec<u8>,
    data_len: usize,
    ranges_hash: String,
    rustc_object_digest: String,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveMemberSetProof {
    raw_ordered_hash: String,
    normalized_ordered_hash: String,
    member_count: usize,
    rustc_link_content_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RustcRlibRawObjectDigest {
    identifier: Vec<u8>,
    digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePatchSectionState {
    input: String,
    section_index: u32,
    section_name: Option<String>,
    input_size: u64,
    output_offset: u64,
    output_size: u64,
    data_hash: Option<String>,
    cstring_nul_boundaries_hash: Option<String>,
}

#[derive(Clone, Copy)]
enum PatchSectionReadMode {
    Parse,
    PreserveRaw,
}

#[derive(Debug, Clone, Eq)]
struct FileContentState {
    len: u64,
    hash: String,
    identity: Option<FileIdentity>,
}

#[derive(Debug, Clone)]
struct FileIdentity {
    len: u64,
    dev: u64,
    ino: u64,
    modified_sec: i64,
    modified_nsec: i64,
    changed_sec: i64,
    changed_nsec: i64,
}

impl PartialEq for FileIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self.dev == other.dev
            && self.ino == other.ino
            && self.modified_sec == other.modified_sec
            && self.modified_nsec == other.modified_nsec
            && self.changed_sec == other.changed_sec
            && self.changed_nsec == other.changed_nsec
    }
}

impl Eq for FileIdentity {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SectionRecord {
    input_file: SharedText,
    input: SharedText,
    section_index: u32,
    output_offset: u64,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct RelocationRecord {
    target_symbol_id: u32,
    written_value: Option<u64>,
    target_value: u64,
    applied_target_value: Option<u64>,
    target_name: Option<SharedText>,
    target: Option<RelocationTargetRecord>,
    input_file: SharedText,
    input: SharedText,
    section_index: u32,
    relocation_offset: u64,
    output_offset: u64,
    size: u64,
    kind: u32,
    addend: i64,
}

pub(crate) struct DeferredRelocationRecord<'data> {
    target_symbol_id: u32,
    written_value: u64,
    target_value: u64,
    applied_target_value: Option<u64>,
    target: DeferredRecordedRelocationTarget<'data>,
    input: InputRef<'data>,
    section_index: object::SectionIndex,
    relocation_offset: u64,
    output_offset: u64,
    size: u64,
    kind: u32,
    addend: i64,
}

pub(crate) struct DeferredMachOSymbolResolution<'data> {
    pub(crate) name: &'data [u8],
    pub(crate) direct_value: Option<u64>,
    pub(crate) got_address: Option<u64>,
    pub(crate) stub_address: Option<u64>,
    pub(crate) thunk_addresses: Vec<u64>,
    pub(crate) target: Option<(InputRef<'data>, object::SectionIndex, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct RelocationTargetRecord {
    input_file: SharedText,
    input: SharedText,
    section_index: u32,
    section_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct MachOSymbolResolutionRecord {
    name: SharedText,
    direct_value: Option<u64>,
    got_address: Option<u64>,
    stub_address: Option<u64>,
    thunk_addresses: Vec<u64>,
    target: Option<RelocationTargetRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct FdeRecord {
    input_file: SharedText,
    input: SharedText,
    section_index: u32,
    eh_frame_section_index: u32,
    input_offset: u64,
    output_offset: u64,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DynamicRelocationRecord {
    input_file: SharedText,
    input: SharedText,
    section_index: u32,
    relocation_offset: u64,
    output_offset: u64,
    size: u64,
    output_r_offset: Option<u64>,
    output_r_info: Option<u64>,
}

pub(crate) fn maybe_prepare<'data>(
    args: &impl platform::Args,
    file_loader: &FileLoader<'data>,
) -> Result<PreparedState<'data>> {
    if !args.common().incremental {
        return Ok(PreparedState {
            mode: IncrementalMode::Disabled,
            current: CurrentState {
                state_dir: state_dir_for_output(args.output()),
                args_hash: String::new(),
                link_options_hash: String::new(),
                input_order_hash: String::new(),
                sld_version: String::new(),
                link_start: None,
                input_files: Vec::new(),
            },
            retained_output_to_restore: None,
            reusable_inputs: HashSet::new(),
            previous_sections: HashSet::new(),
            previous_relocations: Vec::new(),
            previous_macho_symbol_resolutions_file: None,
            previous_fdes: Vec::new(),
            previous_dynamic_relocations: Vec::new(),
            current_sections: RecordBuffers::default(),
            current_relocations: RecordBuffers::default(),
            current_macho_symbol_resolutions: RecordBuffers::default(),
            current_fdes: RecordBuffers::default(),
            current_dynamic_relocations: RecordBuffers::default(),
            record_texts: RecordTextInterner::default(),
            reused_sections: AtomicUsize::new(0),
            prepared_fast_build_id_state: Mutex::new(None),
        });
    }

    timing_phase!("Prepare incremental link");

    let state_dir = state_dir_for_output(args.output());
    let previous_metadata = PersistedState::read_metadata(&state_dir);
    let retained_output_to_restore = if args.should_retain_output_snapshot()
        && let Ok(Some(previous)) = &previous_metadata
        && !args.output().try_exists().unwrap_or(false)
    {
        match retained_output_snapshot_matches_previous(&state_dir, &previous.output, args.output())
        {
            Ok(true) => Some(previous.output.clone()),
            Ok(false) => None,
            Err(error) => {
                append_log(
                    &state_dir,
                    &format!("retained output restoration unavailable: {error:?}"),
                )?;
                None
            }
        }
    } else {
        None
    };
    let current = CurrentState::new(
        args,
        file_loader,
        previous_metadata.as_ref().ok().and_then(|p| p.as_ref()),
    );
    log_incremental_link_options_if_requested(
        args,
        &state_dir,
        std::env::var(LOG_INCREMENTAL_LINK_OPTIONS_ENV).is_ok_and(|value| value == "1"),
        std::env::var(LOG_INCREMENTAL_EXACT_ARGS_ENV).is_ok_and(|value| value == "1"),
    )?;
    let (mut mode, previous_metadata) = match previous_metadata {
        Ok(Some(previous)) => (
            classify_incremental_mode_with_output_policy(
                args.output(),
                &current,
                &previous,
                args.should_trust_persistent_output_data_identity(),
                retained_output_to_restore.is_some(),
            ),
            Some(previous),
        ),
        Ok(None) => (
            IncrementalMode::Relink {
                reason: "no previous incremental state".to_owned(),
                can_reuse_unchanged_sections: false,
            },
            None,
        ),
        Err(error) => (
            IncrementalMode::Relink {
                reason: format!("could not read previous incremental state: {error:?}"),
                can_reuse_unchanged_sections: false,
            },
            None,
        ),
    };

    let mut previous_sections = HashSet::new();
    let mut previous_relocations = Vec::new();
    let mut previous_macho_symbol_resolutions_file = None;
    let mut previous_fdes = Vec::new();
    let mut previous_dynamic_relocations = Vec::new();
    if mode_needs_previous_sections(&mode) {
        match PersistedState::read(&state_dir) {
            Ok(Some(previous)) => {
                previous_sections = previous.sections.iter().cloned().collect();
                previous_relocations = previous.relocations;
                previous_macho_symbol_resolutions_file = previous.macho_symbol_resolutions_file;
                previous_fdes = previous.fdes;
                previous_dynamic_relocations = previous.dynamic_relocations;
            }
            Ok(None) => {
                mode = IncrementalMode::Relink {
                    reason: "no previous incremental state".to_owned(),
                    can_reuse_unchanged_sections: false,
                };
            }
            Err(error) => {
                mode = IncrementalMode::Relink {
                    reason: format!("could not read previous incremental state: {error:?}"),
                    can_reuse_unchanged_sections: false,
                };
            }
        }
    }

    current.log_mode(&mode)?;

    let reusable_inputs = previous_metadata
        .as_ref()
        .map(|previous| reusable_input_files(&current.input_files, &previous.input_files))
        .unwrap_or_default();
    let retained_output_to_restore = matches!(mode, IncrementalMode::Reuse)
        .then_some(retained_output_to_restore)
        .flatten();

    Ok(PreparedState {
        mode,
        current,
        retained_output_to_restore,
        reusable_inputs,
        previous_sections,
        previous_relocations,
        previous_macho_symbol_resolutions_file,
        previous_fdes,
        previous_dynamic_relocations,
        current_sections: RecordBuffers::default(),
        current_relocations: RecordBuffers::default(),
        current_macho_symbol_resolutions: RecordBuffers::default(),
        current_fdes: RecordBuffers::default(),
        current_dynamic_relocations: RecordBuffers::default(),
        record_texts: RecordTextInterner::default(),
        reused_sections: AtomicUsize::new(0),
        prepared_fast_build_id_state: Mutex::new(None),
    })
}

fn mode_needs_previous_sections(mode: &IncrementalMode) -> bool {
    matches!(
        mode,
        IncrementalMode::Reuse
            | IncrementalMode::Relink {
                can_reuse_unchanged_sections: true,
                ..
            }
    )
}

pub(crate) fn maybe_reuse_output_before_loading(args: &impl platform::Args) -> Result<bool> {
    if !args.common().incremental {
        return Ok(false);
    }

    let state_dir = state_dir_for_output(args.output());
    if maybe_reuse_output_during_publication(args, &state_dir)? {
        return Ok(true);
    }
    let _state_lock = acquire_incremental_state_lock(&state_dir)?;
    let current_link_start = write_link_start_marker(&state_dir)?;

    if args.should_write_trace_file() || args.common().save_dir.is_active() {
        return Ok(false);
    }
    if args
        .dependency_file()
        .is_some_and(|dependency_file| !dependency_file.exists())
    {
        return Ok(false);
    }

    timing_phase!("Check incremental fast path");

    if let Some(reason) = interrupted_update_relink_reason(&state_dir) {
        append_log(
            &state_dir,
            &format!("incremental fast path unavailable before loading inputs: {reason}"),
        )?;
        return Ok(false);
    }

    let Some(mut previous) = ({
        timing_phase!("Read incremental fast-path metadata");
        PersistedState::read_metadata(&state_dir).unwrap_or_default()
    }) else {
        return Ok(false);
    };

    if previous.args_hash != args_hash(args) {
        return Ok(false);
    }
    let current_sld_version = sld_version(args);
    if sld_version_relink_reason(previous.sld_version.as_deref(), &current_sld_version).is_some() {
        return Ok(false);
    }
    let retained_output_to_restore = if args.should_retain_output_snapshot()
        && !args.output().try_exists().unwrap_or(false)
    {
        match retained_output_snapshot_matches_previous(&state_dir, &previous.output, args.output())
        {
            Ok(true) => Some(previous.output.clone()),
            Ok(false) => return Ok(false),
            Err(error) => {
                append_log(
                    &state_dir,
                    &format!("retained output restoration unavailable: {error:?}"),
                )?;
                return Ok(false);
            }
        }
    } else {
        None
    };
    if retained_output_to_restore.is_none()
        && !{
            timing_phase!("Validate incremental fast-path output content");
            output_content_matches_previous(
                &previous.output,
                args.output(),
                args.should_trust_persistent_output_data_identity(),
            )?
        }
    {
        return Ok(false);
    }

    let mut changed_inputs = Vec::new();
    let mut rewritten_inputs = Vec::new();
    let mut checked_ambiguous_inputs = false;
    let input_checks = {
        timing_phase!("Check incremental fast-path input contents");
        previous
            .input_files
            .par_iter()
            .enumerate()
            .map(|(index, input)| {
                let path = decode_path(&input.path)?;
                if input.content.identity_matches_path(&path)? {
                    if input_content_is_anchored_before_link_start(
                        input,
                        previous.link_start.as_ref(),
                    ) || !input
                        .content
                        .identity_is_ambiguous_since(previous.link_start.as_ref())
                    {
                        return Ok((None, None, false));
                    }
                    if input_content_matches_previous(&state_dir, input, &path)? {
                        return Ok((None, None, true));
                    }
                    return Ok((Some((index, path)), None, true));
                }
                if args.should_patch_changed_inputs_before_loading() && input.patch.is_some() {
                    // The patcher must read a changed input under a stable identity anyway. Let it
                    // classify patchable identity replacements during that read instead of hashing
                    // the same large archive first.
                    return Ok((Some((index, path)), None, false));
                }
                if input_content_matches_previous(&state_dir, input, &path)? {
                    return Ok((None, Some((index, path)), false));
                }
                Ok((Some((index, path)), None, false))
            })
            .collect::<Result<Vec<_>>>()?
    };
    for (changed_input, rewritten_input, checked_ambiguous_input) in input_checks {
        if let Some(changed_input) = changed_input {
            changed_inputs.push(changed_input);
        }
        if let Some(rewritten_input) = rewritten_input {
            rewritten_inputs.push(rewritten_input);
        }
        checked_ambiguous_inputs |= checked_ambiguous_input;
    }

    if !rewritten_inputs.is_empty() {
        timing_phase!("Snapshot rewritten incremental inputs");
        snapshot_input_paths(
            &state_dir,
            rewritten_inputs
                .iter()
                .filter(|(input_index, _)| {
                    previous
                        .input_files
                        .get(*input_index)
                        .is_none_or(|input| input.snapshot_identity.is_none())
                })
                .map(|(_, path)| path.as_path()),
        )?;
        refresh_snapshotted_rewritten_input_metadata(&state_dir, &mut previous, &rewritten_inputs);
    }

    if !changed_inputs.is_empty() {
        if retained_output_to_restore.is_some() {
            append_log(
                &state_dir,
                "changed-input patch unavailable before loading inputs: missing retained output requires full relink",
            )?;
            return Ok(false);
        }
        if !args.should_patch_changed_inputs_before_loading() {
            append_log(
                &state_dir,
                "changed-input patch unavailable before loading inputs: signed Mach-O output requires full relink",
            )?;
            return Ok(false);
        }
        let changed_input_indices = changed_inputs
            .iter()
            .map(|(input_index, _)| *input_index)
            .collect::<HashSet<_>>();
        let metadata_update_input_indices =
            metadata_update_indices_for_inputs(&changed_inputs, &rewritten_inputs);
        if changed_input_indices.iter().any(|index| {
            previous.input_files[*index]
                .patch
                .as_ref()
                .is_some_and(|patch| patch.sections.is_empty() && patch.raw_sections.is_none())
        }) {
            timing_phase!("Read changed-input patch metadata");
            previous.read_patch_metadata_for_input_indices(&state_dir, &changed_input_indices)?;
        }
        let should_filter_records = previous.patch_records_file.is_some()
            || previous
                .sections_file
                .as_deref()
                .is_some_and(|sections_file| {
                    should_filter_sections_sidecar(&state_dir, sections_file)
                });
        let should_retry_with_full_state = should_filter_records;
        let result = if should_filter_records {
            let result = patch_changed_inputs(
                args,
                &state_dir,
                previous,
                current_link_start.clone(),
                false,
                &changed_inputs,
                &metadata_update_input_indices,
            )?;
            if let ChangedInputPatchResult::Unsupported(reason) = result {
                append_log(
                    &state_dir,
                    &format!(
                        "metadata-only changed-input patch unavailable before loading inputs: {reason}"
                    ),
                )?;
                let Some(mut previous) = PersistedState::read_metadata(&state_dir)? else {
                    return Ok(false);
                };
                refresh_snapshotted_rewritten_input_metadata(
                    &state_dir,
                    &mut previous,
                    &rewritten_inputs,
                );
                previous
                    .read_patch_metadata_for_input_indices(&state_dir, &changed_input_indices)?;
                let changed_input_files = changed_inputs
                    .iter()
                    .map(|(input_index, _)| previous.input_files[*input_index].path.clone())
                    .collect::<HashSet<_>>();
                previous.read_records_for_input_files(&state_dir, &changed_input_files)?;
                patch_changed_inputs(
                    args,
                    &state_dir,
                    previous,
                    current_link_start.clone(),
                    false,
                    &changed_inputs,
                    &metadata_update_input_indices,
                )?
            } else {
                result
            }
        } else {
            let mut records_complete = previous.sections_file.is_none();
            if previous.sections_file.is_some()
                && let Some(mut full_previous) = PersistedState::read(&state_dir)?
            {
                full_previous.input_files = previous.input_files;
                previous = full_previous;
                records_complete = true;
            }
            patch_changed_inputs(
                args,
                &state_dir,
                previous,
                current_link_start.clone(),
                records_complete,
                &changed_inputs,
                &metadata_update_input_indices,
            )?
        };
        let result = if let ChangedInputPatchResult::Unsupported(reason) = result {
            if should_retry_with_full_state {
                append_log(
                    &state_dir,
                    &format!(
                        "filtered-record changed-input patch unavailable before loading inputs: {reason}"
                    ),
                )?;
            }
            if should_retry_with_full_state
                && changed_input_patch_retry_may_benefit_from_complete_records(&reason)
                && let Some(mut full_previous) = PersistedState::read(&state_dir)?
            {
                refresh_snapshotted_rewritten_input_metadata(
                    &state_dir,
                    &mut full_previous,
                    &rewritten_inputs,
                );
                patch_changed_inputs(
                    args,
                    &state_dir,
                    full_previous,
                    current_link_start,
                    true,
                    &changed_inputs,
                    &metadata_update_input_indices,
                )?
            } else {
                ChangedInputPatchResult::Unsupported(reason)
            }
        } else {
            result
        };
        match result {
            ChangedInputPatchResult::Patched => return Ok(true),
            ChangedInputPatchResult::Unsupported(reason) => {
                append_log(
                    &state_dir,
                    &format!("changed-input patch unavailable before loading inputs: {reason}"),
                )?;
            }
            ChangedInputPatchResult::StartedUnsupported(reason) => {
                append_log(
                    &state_dir,
                    &format!(
                        "changed-input patch failed after starting update before loading inputs: {reason}"
                    ),
                )?;
            }
        }
        return Ok(false);
    }

    if let Some(reason) = input_identity_mismatch_reason(&previous.input_files)? {
        append_log(
            &state_dir,
            &format!("incremental fast path unavailable before loading inputs: {reason}"),
        )?;
        return Ok(false);
    }

    if (!rewritten_inputs.is_empty() || checked_ambiguous_inputs)
        && let Some(mut metadata) = PersistedState::read_metadata(&state_dir)?
    {
        refresh_snapshotted_rewritten_input_metadata(&state_dir, &mut metadata, &rewritten_inputs);
        metadata.link_start = current_link_start;
        metadata.write_metadata_update(&state_dir)?;
    }
    if !rewritten_inputs.is_empty() {
        append_log(
            &state_dir,
            &format!(
                "updated {} rewritten input file{} before loading inputs",
                rewritten_inputs.len(),
                if rewritten_inputs.len() == 1 { "" } else { "s" }
            ),
        )?;
    }
    if let Some(previous_output) = retained_output_to_restore.as_ref()
        && !restore_missing_output_snapshot(&state_dir, previous_output, args.output())?
    {
        return Ok(false);
    }
    append_log(&state_dir, "reused existing output before loading inputs")?;
    Ok(true)
}

fn refresh_snapshotted_rewritten_input_metadata(
    state_dir: &Path,
    previous: &mut PersistedState,
    rewritten_inputs: &[(usize, PathBuf)],
) {
    refresh_rewritten_input_identities(previous, rewritten_inputs);
    refresh_input_snapshot_identities_at_indices(
        state_dir,
        &mut previous.input_files,
        rewritten_inputs.iter().map(|(input_index, _)| *input_index),
    );
}

fn maybe_reuse_output_during_publication(
    args: &impl platform::Args,
    state_dir: &Path,
) -> Result<bool> {
    if !update_marker_path(state_dir).try_exists().unwrap_or(false)
        || args.should_write_trace_file()
        || args.common().save_dir.is_active()
        || args
            .dependency_file()
            .is_some_and(|dependency_file| !dependency_file.exists())
    {
        return Ok(false);
    }
    let Some(previous) = PersistedState::read_metadata(state_dir).unwrap_or_default() else {
        return Ok(false);
    };
    if previous.sections_file.as_deref() != Some(PUBLISHING_SECTIONS_FILE)
        || previous.patch_records_file.is_some()
        || previous.args_hash != args_hash(args)
        || sld_version_relink_reason(previous.sld_version.as_deref(), &sld_version(args)).is_some()
        || !output_content_matches_previous(
            &previous.output,
            args.output(),
            args.should_trust_persistent_output_data_identity(),
        )?
    {
        return Ok(false);
    }
    let inputs_match = previous
        .input_files
        .par_iter()
        .map(|input| -> Result<bool> {
            let path = decode_path(&input.path)?;
            if !input.content.identity_matches_path(&path)? {
                return Ok(false);
            }
            if input_content_is_anchored_before_link_start(input, previous.link_start.as_ref())
                || !input
                    .content
                    .identity_is_ambiguous_since(previous.link_start.as_ref())
            {
                return Ok(true);
            }
            input_content_matches_previous(state_dir, input, &path)
        })
        .try_reduce(|| true, |left, right| Ok(left && right))?;
    if !inputs_match {
        return Ok(false);
    }
    if input_identity_mismatch_reason(&previous.input_files)?.is_some() {
        return Ok(false);
    }
    append_log(
        state_dir,
        "reused existing output before loading inputs while incremental state publication was pending",
    )?;
    Ok(true)
}

fn input_identity_is_anchored_by_snapshot(input: &FileState) -> bool {
    input
        .snapshot_identity
        .as_ref()
        .zip(input.content.identity.as_ref())
        .is_some_and(|(snapshot, content)| snapshot == content)
}

// Creating a hardlink advances ctime without changing bytes. Only use that anchor when mtime
// proves the content predates this link; otherwise the stored hash must validate the input.
fn input_content_is_anchored_before_link_start(
    input: &FileState,
    link_start: Option<&FileIdentity>,
) -> bool {
    input_identity_is_anchored_by_snapshot(input)
        && input
            .content
            .identity
            .as_ref()
            .zip(link_start)
            .is_some_and(|(identity, link_start)| !identity.modified_on_or_after(link_start))
}

pub(crate) fn stabilize_rustc_transient_inputs(args: &mut crate::args::Args) -> Result<()> {
    remove_empty_rustc_raw_dylib_search_paths(args);

    let (common, output) = match args {
        crate::args::Args::Elf(args) if args.common.incremental => {
            (&mut args.common, args.output.clone())
        }
        crate::args::Args::MachO(args)
            if args.common.incremental
                && std::env::var_os(STABILIZE_RUSTC_TRANSIENT_INPUTS_ENV)
                    .is_some_and(|value| value == "1") =>
        {
            (&mut args.common, args.output.clone())
        }
        _ => return Ok(()),
    };

    let state_dir = state_dir_for_output(&output);
    timing_phase!("Stabilize rustc transient inputs");
    let stable_dir = state_dir.join(STABLE_RUSTC_INPUT_DIR);
    let (provenance, provenance_file) = rustc_work_product_provenance_from_env();
    let previous = provenance
        .as_ref()
        .and_then(|_| PersistedState::read_metadata(&state_dir).unwrap_or_default());
    let previous_inputs_by_path = previous_input_files_by_path(previous.as_ref());
    let mut stabilized = 0;
    let mut matched_producer_digests = 0;
    let mut reused_isolated = 0;
    for input in &mut common.inputs {
        let InputSpec::File(source) = &input.spec else {
            continue;
        };
        let Some(stable_name) = stable_rustc_input_name(source, &output) else {
            continue;
        };
        let source = source.to_path_buf();
        let target = stable_dir.join(stable_name);
        let producer_digest = provenance
            .as_ref()
            .and_then(|provenance| provenance.get(&source));
        if producer_digest.is_some() {
            matched_producer_digests += 1;
        }
        let reused_producer_digest = producer_digest.is_some_and(|digest| {
            stable_rustc_input_matches_previous_producer_digest(
                &previous_inputs_by_path,
                &target,
                digest,
            )
        });
        if reused_producer_digest {
            reused_isolated += 1;
        }

        let already_stable = if provenance.is_some() {
            reused_producer_digest
        } else {
            stable_rustc_input_already_names_source_data(&source, &target)
        };
        if !already_stable {
            std::fs::create_dir_all(&stable_dir).with_context(|| {
                format!(
                    "Failed to create stable rustc input directory `{}`",
                    stable_dir.display()
                )
            })?;
            let tmp = target.with_file_name(format!(
                "{}.{}.tmp",
                target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("input"),
                std::process::id()
            ));
            let _ = std::fs::remove_file(&tmp);
            if provenance.is_some() {
                copy_isolated_snapshot_bytes(&source, &tmp)?;
            } else {
                copy_snapshot_bytes(&source, &tmp)?;
            }
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&tmp, &target).with_context(|| {
                format!(
                    "Failed to install stable rustc input `{}`",
                    target.display()
                )
            })?;
        }
        input.spec = InputSpec::File(target.into_boxed_path());
        stabilized += 1;
    }

    if stabilized > 0 {
        append_log(
            &state_dir,
            &format!(
                "stabilized {stabilized} rustc transient input{} before loading inputs",
                if stabilized == 1 { "" } else { "s" }
            ),
        )?;
    }
    if reused_isolated > 0 {
        append_log(
            &state_dir,
            &format!(
                "reused {reused_isolated} isolated rustc work-product input{} by producer digest",
                if reused_isolated == 1 { "" } else { "s" }
            ),
        )?;
    }
    if let Some(provenance) = provenance {
        append_log(
            &state_dir,
            &format!(
                "loaded {} rustc work-product producer digest record{}; matched \
                 {matched_producer_digests} stabilized input{}; reused {reused_isolated} isolated \
                 input{}; manifest path present: {}; readable: {}; parsed: {}",
                provenance.len(),
                if provenance.len() == 1 { "" } else { "s" },
                if matched_producer_digests == 1 {
                    ""
                } else {
                    "s"
                },
                if reused_isolated == 1 { "" } else { "s" },
                provenance_file.path_present,
                provenance_file.readable,
                provenance_file.parsed,
            ),
        )?;
    }
    Ok(())
}

struct RustcWorkProductProvenanceFile {
    path_present: bool,
    readable: bool,
    parsed: bool,
}

fn rustc_work_product_provenance_from_env() -> (
    Option<HashMap<PathBuf, String>>,
    RustcWorkProductProvenanceFile,
) {
    let requested = rustc_work_product_provenance_enabled();
    let path = std::env::var_os(RUSTC_WORK_PRODUCT_PROVENANCE_FILE_ENV);
    let contents = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok());
    let parsed = contents
        .as_deref()
        .and_then(parse_rustc_work_product_provenance);
    (
        rustc_work_product_provenance(contents.as_deref(), requested || path.is_some()),
        RustcWorkProductProvenanceFile {
            path_present: path.is_some(),
            readable: contents.is_some(),
            parsed: parsed.is_some(),
        },
    )
}

fn rustc_work_product_provenance(
    contents: Option<&str>,
    requested: bool,
) -> Option<HashMap<PathBuf, String>> {
    if !requested {
        return None;
    }
    Some(
        contents
            .and_then(parse_rustc_work_product_provenance)
            .unwrap_or_default(),
    )
}

fn parse_rustc_work_product_provenance(contents: &str) -> Option<HashMap<PathBuf, String>> {
    let mut lines = contents.lines();
    if lines.next()? != RUSTC_WORK_PRODUCT_PROVENANCE_VERSION {
        return None;
    }
    let mut provenance = HashMap::new();
    for line in lines {
        let (digest, path) = line.split_once('\t')?;
        if !is_blake3_hex_digest(digest) || path.contains('\t') {
            return None;
        }
        if provenance
            .insert(decode_path(path).ok()?, digest.to_owned())
            .is_some()
        {
            return None;
        }
    }
    Some(provenance)
}

fn rustc_work_product_provenance_enabled() -> bool {
    std::env::var_os(RUSTC_WORK_PRODUCT_PROVENANCE_ENV).as_deref() == Some("1".as_ref())
}

fn is_blake3_hex_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_blake3_hex_digest(digest: &str) -> Option<[u8; blake3::OUT_LEN]> {
    if !is_blake3_hex_digest(digest) {
        return None;
    }
    let mut bytes = [0; blake3::OUT_LEN];
    hex::decode_to_slice(digest, &mut bytes).ok()?;
    Some(bytes)
}

fn previous_input_files_by_path(previous: Option<&PersistedState>) -> HashMap<&str, &FileState> {
    let Some(previous) = previous else {
        return HashMap::new();
    };
    let mut inputs_by_path = HashMap::with_capacity(previous.input_files.len());
    for input in &previous.input_files {
        inputs_by_path.entry(input.path.as_str()).or_insert(input);
    }
    inputs_by_path
}

fn stable_rustc_input_matches_previous_producer_digest(
    previous_inputs_by_path: &HashMap<&str, &FileState>,
    target: &Path,
    digest: &str,
) -> bool {
    let encoded_target = encode_path(target);
    previous_inputs_by_path
        .get(encoded_target.as_str())
        .is_some_and(|input| {
            input.content.hash == digest
                && input.content.identity_matches_path(target).unwrap_or(false)
        })
}

fn stable_rustc_input_already_names_source_data(source: &Path, target: &Path) -> bool {
    if !is_atomic_replacement_rust_input(source) {
        return false;
    }

    FileIdentity::from_path(source)
        .ok()
        .flatten()
        .zip(FileIdentity::from_path(target).ok().flatten())
        .is_some_and(|(source, target)| source.matches_same_data_ignoring_change_time(&target))
}

fn remove_empty_rustc_raw_dylib_search_paths(args: &mut crate::args::Args) {
    let crate::args::Args::Elf(args) = args else {
        return;
    };
    if !args.common.incremental {
        return;
    }

    args.lib_search_path
        .retain(|path| !is_empty_rustc_raw_dylib_search_path(path));
}

fn is_empty_rustc_raw_dylib_search_path(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("raw-dylibs")
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rustc"))
        && std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

fn stable_rustc_input_name(path: &Path, output: &Path) -> Option<PathBuf> {
    let output_dir = output.parent()?;
    let filename = path.file_name()?.to_str()?;
    if filename == "symbols.o"
        && path.parent()?.parent() == Some(output_dir)
        && path.parent()?.file_name()?.to_str()?.starts_with("rustc")
    {
        return Some(PathBuf::from("rustc-symbols.o"));
    }
    let output_name = output.file_name()?.to_str()?;
    if path.parent() != Some(output_dir)
        || !filename.starts_with(&format!("{output_name}."))
        || !filename.ends_with(".rcgu.o")
    {
        return None;
    }

    let parts = filename.split('.').collect::<Vec<_>>();
    let [crate_name, codegen_unit, invocation, "rcgu", "o"] = parts.as_slice() else {
        return None;
    };
    if crate_name.is_empty() || codegen_unit.is_empty() || invocation.is_empty() {
        return None;
    }
    Some(PathBuf::from(format!("{crate_name}.{codegen_unit}.rcgu.o")))
}

fn metadata_update_indices_for_inputs(
    changed_inputs: &[(usize, PathBuf)],
    rewritten_inputs: &[(usize, PathBuf)],
) -> Vec<usize> {
    let mut indices = changed_inputs
        .iter()
        .chain(rewritten_inputs)
        .map(|(input_index, _)| *input_index)
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices.dedup();
    indices
}

fn refresh_rewritten_input_identities(
    previous: &mut PersistedState,
    rewritten_inputs: &[(usize, PathBuf)],
) {
    refresh_input_file_identities_at_indices(
        &mut previous.input_files,
        rewritten_inputs.iter().map(|(input_index, _)| *input_index),
    );
}

enum ChangedInputPatchResult {
    Patched,
    Unsupported(String),
    StartedUnsupported(String),
}

fn changed_input_patch_retry_may_benefit_from_complete_records(reason: &str) -> bool {
    // The anonymous-section identity result is independent of global record coverage. Other
    // refusals remain conservative: complete generated-section state can enable dynamic changes.
    !reason.starts_with("could not match anonymous patch sections in `")
}

fn relocation_target_patches_for_input(
    relocations: &mut [RelocationRecord],
    input: &FileState,
    bytes: &[u8],
) -> Result<std::result::Result<RelocationTargetPatches, String>> {
    let mut input_ranges = Vec::new();
    let mut output_patches = Vec::new();
    let mut output_symbols = Vec::new();
    let mut macho_target_moves = Vec::new();
    for (relocation_index, relocation) in relocations.iter_mut().enumerate() {
        let Some(target) = relocation.target.as_mut() else {
            continue;
        };
        if target.input_file != input.path {
            continue;
        }
        let Some(target_name) = relocation.target_name.as_deref() else {
            continue;
        };
        let Some(input_bytes) = patch_input_bytes(bytes, input.path.as_str(), &target.input)?
        else {
            continue;
        };
        let file = object::File::parse(input_bytes.bytes)
            .context("Failed to parse changed relocation target input")?;
        let cross_input_macho_relocation = relocation.input_file != input.path
            && file.format() == object::BinaryFormat::MachO
            && file.architecture() == object::Architecture::Aarch64
            && decode_macho_aarch64_relocation_kind(relocation.kind).is_some();
        if relocation.input_file == input.path
            && relocation.input == target.input
            && file.format() == object::BinaryFormat::MachO
            && file.architecture() == object::Architecture::Aarch64
            && decode_macho_aarch64_relocation_kind(relocation.kind).is_some()
        {
            // Same-object Mach-O targets must be resolved together with the current relocation
            // site. Rust-generated local names can be reused for semantically different constants.
            continue;
        }
        let current = if cross_input_macho_relocation {
            match unique_symbol_position_by_name(
                input_bytes.bytes,
                input_bytes.file_offset,
                &file,
                target_name,
            )? {
                Ok(current) => current,
                Err(reason) => return Ok(Err(reason)),
            }
        } else {
            symbol_position_by_name(
                input_bytes.bytes,
                input_bytes.file_offset,
                &file,
                target_name,
            )?
        };
        let Some(current) = current else {
            continue;
        };
        if let Some(value_range) = current.value_range {
            input_ranges.push(value_range);
        }
        if current.section_index.0 as u32 == target.section_index
            && current.section_offset == target.section_offset
        {
            continue;
        }
        if current.section_index.0 as u32 != target.section_index {
            return Ok(Err(format!(
                "relocation target `{}` moved from section {} offset {:#x} to section {} offset {:#x} in {}",
                display_hex_text(target_name),
                target.section_index,
                target.section_offset,
                current.section_index.0,
                current.section_offset,
                display_hex_path(&input.path)
            )));
        }

        let Some(previous_written_value) = relocation.written_value else {
            return Ok(Err(format!(
                "missing written relocation value for target in {}",
                display_hex_path(&input.path)
            )));
        };
        let delta = i128::from(current.section_offset) - i128::from(target.section_offset);
        let previous_target_value = relocation.target_value;
        let previous_applied_target_value = relocation
            .applied_target_value
            .unwrap_or(previous_target_value);
        let Some(target_value) = add_signed_delta_u64(previous_target_value, delta) else {
            return Ok(Err(format!(
                "relocation target value overflowed in {}",
                display_hex_path(&input.path)
            )));
        };
        if file.format() == object::BinaryFormat::MachO
            && file.architecture() == object::Architecture::Aarch64
            && let Some(raw_relocation) = decode_macho_aarch64_relocation_kind(relocation.kind)
        {
            if relocation.input_file != input.path
                && !macho_aarch64_cross_input_relocation_is_supported(
                    raw_relocation,
                    relocation.size,
                )
            {
                return Ok(Err(format!(
                    "unsupported cross-input Mach-O relocation target patch in {}",
                    display_hex_path(&input.path)
                )));
            }
            let rel_info =
                <crate::macho_aarch64::MachOAArch64 as crate::platform::Arch>::relocation_from_raw(
                    raw_relocation,
                )
                .map_err(|error| {
                    crate::error!("Failed to decode recorded Mach-O target relocation: {error:#?}")
                })?;
            if relocation.input_file != input.path {
                if previous_applied_target_value != previous_target_value {
                    return Ok(Err(format!(
                        "moved Mach-O stub or thunk target in {}",
                        display_hex_path(&input.path)
                    )));
                }
                let Some(written_value) = macho_aarch64_relocated_value_after_target_move(
                    raw_relocation.r_type,
                    previous_written_value,
                    previous_target_value,
                    target_value,
                    relocation.addend,
                ) else {
                    return Ok(Err(format!(
                        "unsupported cross-input Mach-O relocation target patch in {}",
                        display_hex_path(&input.path)
                    )));
                };
                let applied_target_value = target_value;
                let Some(deferred_relocation) = deferred_instruction_relocation_patch(
                    rel_info,
                    previous_written_value,
                    written_value,
                ) else {
                    return Ok(Err(format!(
                        "unsupported cross-input Mach-O relocation instruction in {}",
                        display_hex_path(&input.path)
                    )));
                };
                let Ok(size) = usize::try_from(relocation.size) else {
                    return Ok(Err(format!(
                        "unsupported cross-input Mach-O relocation size in {}",
                        display_hex_path(&input.path)
                    )));
                };
                output_patches.push(SectionPatch {
                    output_offset: relocation.output_offset,
                    size: relocation.size,
                    data: vec![0; size],
                    deferred_relocation: Some(deferred_relocation),
                    preserve_ranges: Vec::new(),
                    adjustments: Vec::new(),
                });
                relocation.written_value = Some(written_value);
                relocation.target_value = target_value;
                relocation.applied_target_value = Some(applied_target_value);
                target.section_offset = current.section_offset;
                output_symbols.push(RelocationTargetSymbolPatch {
                    target_name: target_name.to_owned(),
                    previous_target_value,
                    target_value,
                    allow_missing: true,
                });
                continue;
            }
            macho_target_moves.push(MachORelocationTargetMove {
                relocation_index,
                current_section_offset: current.section_offset,
                current_target_value: target_value,
            });
            output_symbols.push(RelocationTargetSymbolPatch {
                target_name: target_name.to_owned(),
                previous_target_value,
                target_value,
                allow_missing: true,
            });
            continue;
        }

        let written_value = add_encoded_delta_u64(previous_written_value, delta);
        let Some(rel_info) = relocation_kind_info(&file, relocation.kind) else {
            return Ok(Err(format!(
                "unsupported relocation target patch kind in {}",
                display_hex_path(&input.path)
            )));
        };
        let deferred_relocation =
            deferred_instruction_relocation_patch(rel_info, previous_written_value, written_value);
        let data = if deferred_relocation.is_some() {
            let Ok(size) = usize::try_from(relocation.size) else {
                return Ok(Err(format!(
                    "unsupported relocation target patch size in {}",
                    display_hex_path(&input.path)
                )));
            };
            vec![0; size]
        } else {
            let Ok(size) = usize::try_from(relocation.size) else {
                return Ok(Err(format!(
                    "unsupported relocation target patch size in {}",
                    display_hex_path(&input.path)
                )));
            };
            let mut data = vec![0; size];
            if rel_info.write_to_buffer(written_value, &mut data).is_err() {
                return Ok(Err(format!(
                    "relocation target patch overflowed in {}",
                    display_hex_path(&input.path)
                )));
            }
            data
        };
        let previous_target_value = relocation.target_value;
        let Some(target_value) = add_signed_delta_u64(previous_target_value, delta) else {
            return Ok(Err(format!(
                "relocation target value overflowed in {}",
                display_hex_path(&input.path)
            )));
        };
        output_patches.push(SectionPatch {
            output_offset: relocation.output_offset,
            size: relocation.size,
            data,
            deferred_relocation,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        });
        relocation.written_value = Some(written_value);
        relocation.target_value = target_value;
        target.section_offset = current.section_offset;
        output_symbols.push(RelocationTargetSymbolPatch {
            target_name: target_name.to_owned(),
            previous_target_value,
            target_value,
            allow_missing: false,
        });
    }
    dedup_ranges(&mut input_ranges);
    Ok(Ok(RelocationTargetPatches {
        input_ranges,
        output_patches,
        output_symbols,
        macho_target_moves,
    }))
}

fn update_macho_symbol_resolutions_for_input(
    resolutions: &mut [MachOSymbolResolutionRecord],
    input: &FileState,
    bytes: &[u8],
) -> Result<std::result::Result<MachOSymbolResolutionUpdates, String>> {
    let mut input_ranges = Vec::new();
    let mut output_symbols = Vec::new();
    let mut changed = false;
    for resolution in resolutions {
        let Some(target) = resolution
            .target
            .as_mut()
            .filter(|target| target.input_file == input.path)
        else {
            continue;
        };
        let Some(input_bytes) = patch_input_bytes(bytes, input.path.as_str(), &target.input)?
        else {
            return Ok(Err(format!(
                "could not resolve Mach-O symbol catalog input in {}",
                display_hex_path(&input.path)
            )));
        };
        let file = object::File::parse(input_bytes.bytes)
            .context("Failed to parse changed Mach-O symbol catalog input")?;
        let current = match unique_symbol_position_by_name(
            input_bytes.bytes,
            input_bytes.file_offset,
            &file,
            resolution.name.as_str(),
        )? {
            Ok(Some(current)) => current,
            Ok(None) => {
                return Ok(Err(format!(
                    "missing Mach-O symbol catalog target in {}",
                    display_hex_path(&input.path)
                )));
            }
            Err(reason) => return Ok(Err(reason)),
        };
        if let Some(value_range) = current.value_range {
            input_ranges.push(value_range);
        }
        if current.section_index.0 as u32 != target.section_index {
            return Ok(Err(format!(
                "Mach-O symbol catalog target changed sections in {}",
                display_hex_path(&input.path)
            )));
        }
        if current.section_offset == target.section_offset {
            continue;
        }
        if resolution.got_address.is_some() || !resolution.thunk_addresses.is_empty() {
            return Ok(Err(format!(
                "moved Mach-O GOT or thunk target in {}",
                display_hex_path(&input.path)
            )));
        }
        let Some(previous_value) = resolution.direct_value else {
            return Ok(Err(format!(
                "missing direct Mach-O symbol catalog target in {}",
                display_hex_path(&input.path)
            )));
        };
        let delta = i128::from(current.section_offset) - i128::from(target.section_offset);
        let Some(current_value) = add_signed_delta_u64(previous_value, delta) else {
            return Ok(Err(format!(
                "Mach-O symbol catalog target value overflowed in {}",
                display_hex_path(&input.path)
            )));
        };
        output_symbols.push(RelocationTargetSymbolPatch {
            target_name: resolution.name.to_string(),
            previous_target_value: previous_value,
            target_value: current_value,
            allow_missing: true,
        });
        resolution.direct_value = Some(current_value);
        target.section_offset = current.section_offset;
        changed = true;
    }
    dedup_ranges(&mut input_ranges);
    Ok(Ok(MachOSymbolResolutionUpdates {
        input_ranges,
        output_symbols,
        changed,
    }))
}

fn deferred_instruction_relocation_patch(
    rel_info: RelocationKindInfo,
    previous_written_value: u64,
    written_value: u64,
) -> Option<DeferredRelocationPatch> {
    matches!(rel_info.size, RelocationSize::BitMasking(_)).then_some(DeferredRelocationPatch {
        rel_info,
        previous_written_value,
        written_value,
    })
}

fn relocation_kind_info(
    file: &object::File<'_>,
    relocation_kind: u32,
) -> Option<RelocationKindInfo> {
    Some(match file.architecture() {
        object::Architecture::X86_64 => x86_64::relocation_from_raw(relocation_kind)?,
        object::Architecture::Aarch64 => aarch64::relocation_type_from_raw(relocation_kind)?,
        object::Architecture::LoongArch64 => {
            loongarch64::relocation_type_from_raw(relocation_kind)?
        }
        object::Architecture::Riscv64 => riscv64::relocation_type_from_raw(relocation_kind)?,
        _ => return None,
    })
}

const MACHO_AARCH64_RELOCATION_KIND_TAG: u32 = 0xa000_0000;
const MACHO_RELOCATION_KIND_TAG_MASK: u32 = 0xf000_0000;

pub(crate) fn encode_macho_aarch64_relocation_kind(
    relocation: object::macho::RelocationInfo,
) -> u32 {
    MACHO_AARCH64_RELOCATION_KIND_TAG
        | u32::from(relocation.r_type & 0xf)
        | (u32::from(relocation.r_pcrel) << 4)
        | (u32::from(relocation.r_length & 0x3) << 5)
        | (u32::from(relocation.r_extern) << 7)
}

fn decode_macho_aarch64_relocation_kind(kind: u32) -> Option<object::macho::RelocationInfo> {
    (kind & MACHO_RELOCATION_KIND_TAG_MASK == MACHO_AARCH64_RELOCATION_KIND_TAG).then_some(
        object::macho::RelocationInfo {
            r_address: 0,
            r_symbolnum: 0,
            r_pcrel: kind & (1 << 4) != 0,
            r_length: ((kind >> 5) & 0x3) as u8,
            r_extern: kind & (1 << 7) != 0,
            r_type: (kind & 0xf) as u8,
        },
    )
}

fn macho_aarch64_relocated_value(
    r_type: u8,
    target_value: u64,
    addend: i64,
    place: u64,
) -> Option<u64> {
    let target_value = target_value.wrapping_add(addend as u64);
    Some(match r_type {
        object::macho::ARM64_RELOC_BRANCH26 => target_value.wrapping_sub(place),
        object::macho::ARM64_RELOC_PAGE21 => (target_value & !linker_utils::elf::PAGE_MASK_4KB)
            .wrapping_sub(place & !linker_utils::elf::PAGE_MASK_4KB),
        object::macho::ARM64_RELOC_PAGEOFF12 => target_value,
        _ => return None,
    })
}

fn macho_aarch64_cross_input_relocation_is_supported(
    relocation: object::macho::RelocationInfo,
    size: u64,
) -> bool {
    if size != 4 || relocation.r_length != 2 || !relocation.r_extern {
        return false;
    }
    match relocation.r_type {
        object::macho::ARM64_RELOC_BRANCH26 | object::macho::ARM64_RELOC_PAGE21 => {
            relocation.r_pcrel
        }
        object::macho::ARM64_RELOC_PAGEOFF12 => !relocation.r_pcrel,
        _ => false,
    }
}

fn macho_aarch64_relocated_value_after_target_move(
    r_type: u8,
    previous_written_value: u64,
    previous_target_value: u64,
    current_target_value: u64,
    addend: i64,
) -> Option<u64> {
    let previous_target_component =
        macho_aarch64_relocated_value(r_type, previous_target_value, addend, 0)?;
    let current_target_component =
        macho_aarch64_relocated_value(r_type, current_target_value, addend, 0)?;
    Some(
        previous_written_value
            .wrapping_add(current_target_component.wrapping_sub(previous_target_component)),
    )
}

fn dedup_ranges(ranges: &mut Vec<std::ops::Range<usize>>) {
    ranges.sort_by_key(|range| (range.start, range.end));
    ranges.dedup_by(|left, right| left.start == right.start && left.end == right.end);
}

fn add_signed_delta_u64(value: u64, delta: i128) -> Option<u64> {
    let adjusted = i128::from(value).checked_add(delta)?;
    u64::try_from(adjusted).ok()
}

fn add_encoded_delta_u64(value: u64, delta: i128) -> u64 {
    let modulus = i128::from(u64::MAX) + 1;
    let adjusted = (i128::from(value) + delta).rem_euclid(modulus);
    adjusted as u64
}

struct SymbolPosition {
    section_index: object::SectionIndex,
    section_offset: u64,
    value_range: Option<std::ops::Range<usize>>,
}

fn symbol_position_by_name(
    bytes: &[u8],
    file_offset: usize,
    file: &object::File<'_>,
    encoded_name: &str,
) -> Result<Option<SymbolPosition>> {
    let name = hex::decode(encoded_name).context("Malformed incremental relocation target name")?;
    for symbol in file.symbols() {
        if symbol.name_bytes()? != name {
            continue;
        }
        let Some(section_index) = symbol.section_index() else {
            return Ok(None);
        };
        let section_offset = if file.format() == object::BinaryFormat::MachO {
            let section_address = file.section_by_index(section_index)?.address();
            symbol
                .address()
                .checked_sub(section_address)
                .context("Mach-O symbol address precedes its section")?
        } else {
            symbol.address()
        };
        let value_range = symbol_value_field_range(bytes, symbol.index())
            .map(|range| file_offset + range.start..file_offset + range.end);
        return Ok(Some(SymbolPosition {
            section_index,
            section_offset,
            value_range,
        }));
    }
    Ok(None)
}

fn unique_symbol_position_by_name(
    bytes: &[u8],
    file_offset: usize,
    file: &object::File<'_>,
    encoded_name: &str,
) -> Result<std::result::Result<Option<SymbolPosition>, String>> {
    let name = hex::decode(encoded_name).context("Malformed incremental relocation target name")?;
    let mut matched = None;
    for symbol in file.symbols() {
        if symbol.name_bytes()? != name {
            continue;
        }
        let Some(section_index) = symbol.section_index() else {
            continue;
        };
        if matched.is_some() {
            return Ok(Err(
                "ambiguous cross-input Mach-O relocation target symbol".to_owned()
            ));
        }
        let section_address = file.section_by_index(section_index)?.address();
        let section_offset = symbol
            .address()
            .checked_sub(section_address)
            .context("Mach-O symbol address precedes its section")?;
        let value_range = symbol_value_field_range(bytes, symbol.index())
            .map(|range| file_offset + range.start..file_offset + range.end);
        matched = Some(SymbolPosition {
            section_index,
            section_offset,
            value_range,
        });
    }
    Ok(Ok(matched))
}

fn symbol_position_by_name_and_value(
    bytes: &[u8],
    file_offset: usize,
    file: &object::File<'_>,
    encoded_name: &str,
    value: u64,
) -> Result<std::result::Result<Option<SymbolPosition>, String>> {
    let name = hex::decode(encoded_name).context("Malformed incremental relocation target name")?;
    let mut matched_symbol = None;
    let mut name_was_present = false;
    for symbol in file.symbols() {
        if symbol.name_bytes()? != name {
            continue;
        }
        name_was_present = true;
        if symbol.address() != value {
            continue;
        }
        if matched_symbol.is_some() {
            return Ok(Err(
                "ambiguous output symbol for incremental value patch".to_owned()
            ));
        }
        let Some(section_index) = symbol.section_index() else {
            return Ok(Ok(None));
        };
        let value_range = symbol_value_field_range(bytes, symbol.index())
            .map(|range| file_offset + range.start..file_offset + range.end);
        matched_symbol = Some(SymbolPosition {
            section_index,
            section_offset: symbol.address(),
            value_range,
        });
    }
    if matched_symbol.is_none() && name_was_present {
        return Ok(Err(
            "output symbol value changed before incremental patch".to_owned()
        ));
    }
    Ok(Ok(matched_symbol))
}

fn output_symbol_value_patches(
    output: &[u8],
    symbols: &[RelocationTargetSymbolPatch],
) -> Result<std::result::Result<Vec<SectionPatch>, String>> {
    if symbols.is_empty() {
        return Ok(Ok(Vec::new()));
    }

    let file = object::File::parse(output)
        .context("Failed to parse output for incremental symbol value patching")?;
    let mut values_by_previous_name_and_value = HashMap::<(&str, u64), (u64, bool)>::new();
    for symbol in symbols {
        let key = (symbol.target_name.as_str(), symbol.previous_target_value);
        match values_by_previous_name_and_value.entry(key) {
            hashbrown::hash_map::Entry::Vacant(entry) => {
                entry.insert((symbol.target_value, symbol.allow_missing));
            }
            hashbrown::hash_map::Entry::Occupied(mut entry) => {
                if entry.get().0 != symbol.target_value {
                    return Ok(Err(
                        "conflicting incremental symbol value patches".to_owned()
                    ));
                }
                entry.get_mut().1 &= symbol.allow_missing;
            }
        }
    }

    let mut patches = Vec::with_capacity(values_by_previous_name_and_value.len());
    for ((target_name, previous_target_value), (target_value, allow_missing)) in
        values_by_previous_name_and_value
    {
        let symbol = symbol_position_by_name_and_value(
            output,
            0,
            &file,
            target_name,
            previous_target_value,
        )?;
        let symbol = match symbol {
            Ok(Some(symbol)) => symbol,
            Ok(None) if allow_missing => continue,
            Ok(None) => {
                return Ok(Err(
                    "missing output symbol for incremental value patch".to_owned()
                ));
            }
            Err(error) => return Ok(Err(error)),
        };
        let Some(value_range) = symbol.value_range else {
            return Ok(Err(
                "missing output symbol value range for incremental patch".to_owned(),
            ));
        };
        let data = match value_range.len() {
            4 => {
                let Ok(value) = u32::try_from(target_value) else {
                    return Ok(Err(
                        "incremental output symbol value patch overflowed".to_owned()
                    ));
                };
                value.to_le_bytes().to_vec()
            }
            8 => target_value.to_le_bytes().to_vec(),
            _ => {
                return Ok(Err(
                    "unsupported output symbol value size for incremental patch".to_owned(),
                ));
            }
        };
        patches.push(SectionPatch {
            output_offset: value_range.start as u64,
            size: value_range.len() as u64,
            data,
            deferred_relocation: None,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        });
    }

    Ok(Ok(patches))
}

fn patch_changed_inputs(
    args: &impl platform::Args,
    state_dir: &Path,
    previous: PersistedState,
    current_link_start: Option<FileIdentity>,
    records_complete: bool,
    changed_inputs: &[(usize, PathBuf)],
    metadata_update_input_indices: &[usize],
) -> Result<ChangedInputPatchResult> {
    timing_phase!("Patch changed incremental inputs");

    let mut previous = previous;
    previous.load_macho_symbol_resolutions(state_dir)?;
    let mut patches = Vec::new();
    let mut output_symbol_patches = Vec::new();
    let mut eh_frame_hdr_changes = Vec::new();
    let mut fde_add_candidates = Vec::new();
    let mut expected_changed_inputs = Vec::new();
    let mut rewritten_input_count = 0;
    let mut normalized_unchanged_input_count = 0;
    let mut rustc_link_content_digest_unchanged_input_count = 0;
    let mut rustc_link_content_digest_unchanged_input_indices = HashSet::new();
    let mut deferred_loaded_input_content_hashes = Vec::new();
    let mut hashless_atomic_replacement_snapshot_count = 0;
    let mut patched_input_count = 0;
    let mut patched_section_count = 0;
    let mut previous_output = LazyOutputBytes::new(|| read_output_bytes(args.output()));
    // The embedded digest is a rustc producer assertion. Trust it only inside the
    // Cargo-managed provenance lane; all other callers keep the byte classifier fallback.
    let trust_rustc_link_content_digests =
        std::env::var_os(RUSTC_WORK_PRODUCT_PROVENANCE_ENV).as_deref() == Some("1".as_ref());
    for (input_index, path) in changed_inputs {
        let mut loaded_input = None;
        let can_normalize_rust_archive_patch =
            can_normalize_rust_archive_patch(args, &previous, *input_index, path);
        if can_normalize_rust_archive_patch
            && trust_rustc_link_content_digests
            && previous_rustc_rlib_link_content_digest(&previous.input_files[*input_index])
                .is_some()
            && let Some(input_content) = ({
                timing_phase!("Read rustc rlib link-content digest");
                rustc_rlib_link_content_digest_matches_previous_path(
                    &previous.input_files[*input_index],
                    path,
                )
            })
        {
            expected_changed_inputs.push(ExpectedInputContent::from_content(path, &input_content));
            previous.input_files[*input_index].content = input_content;
            previous.input_files[*input_index].snapshot_identity = None;
            normalized_unchanged_input_count += 1;
            rustc_link_content_digest_unchanged_input_count += 1;
            rustc_link_content_digest_unchanged_input_indices.insert(*input_index);
            continue;
        }
        if previous.input_files[*input_index].patch.is_some() {
            let Some((bytes, input_content)) = ({
                timing_phase!("Read changed incremental input");
                read_file_with_stable_identity_and_hashing(path, !can_normalize_rust_archive_patch)
                    .with_context(|| {
                        format!(
                            "Failed to read changed incremental input `{}`",
                            path.display()
                        )
                    })?
            }) else {
                return Ok(ChangedInputPatchResult::Unsupported(format!(
                    "changed input changed while being read: {}",
                    path.display()
                )));
            };
            if content_state_matches_previous(
                &previous.input_files[*input_index].content,
                &input_content,
            ) {
                expected_changed_inputs
                    .push(ExpectedInputContent::from_content(path, &input_content));
                previous.input_files[*input_index].content = input_content;
                previous.input_files[*input_index].snapshot_identity = None;
                rewritten_input_count += 1;
                continue;
            }
            loaded_input = Some((bytes, input_content));
        }
        if previous.input_files[*input_index].patch.is_none()
            && previous
                .sections
                .iter()
                .any(|section| section.input_file == previous.input_files[*input_index].path)
        {
            let patch = current_patch_state_from_snapshot(
                state_dir,
                &previous.input_files[*input_index],
                previous_output.get()?,
                &previous.sections,
                &previous.relocations,
                &previous.fdes,
                &previous.dynamic_relocations,
                args.should_normalize_rust_archive_patch_inputs(),
            )?;
            previous.input_files[*input_index].patch = patch;
        }
        let previous_patch = {
            let input = &previous.input_files[*input_index];
            match patch_sections_from_previous_state(input, path) {
                Ok(previous_patch) => previous_patch,
                Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
            }
        };
        let (bytes, mut input_content) = if let Some(loaded_input) = loaded_input {
            loaded_input
        } else {
            let Some(loaded_input) = ({
                timing_phase!("Read changed incremental input");
                read_file_with_stable_identity(path).with_context(|| {
                    format!(
                        "Failed to read changed incremental input `{}`",
                        path.display()
                    )
                })?
            }) else {
                return Ok(ChangedInputPatchResult::Unsupported(format!(
                    "changed input changed while being read: {}",
                    path.display()
                )));
            };
            loaded_input
        };
        let input = &previous.input_files[*input_index];
        if can_normalize_rust_archive_patch
            && trust_rustc_link_content_digests
            && rustc_rlib_link_content_digest_matches_previous(input, &bytes)
        {
            expected_changed_inputs.push(ExpectedInputContent::from_content(path, &input_content));
            previous.input_files[*input_index].content = input_content;
            previous.input_files[*input_index].snapshot_identity = None;
            normalized_unchanged_input_count += 1;
            rustc_link_content_digest_unchanged_input_count += 1;
            rustc_link_content_digest_unchanged_input_indices.insert(*input_index);
            continue;
        }
        let normalized_rust_archive_patch_state = if can_normalize_rust_archive_patch {
            classify_normalized_rust_archive_patch_state(input, &bytes, &previous_patch)?
        } else {
            NormalizedRustArchivePatchState::Unknown
        };
        let normalized_rust_archive_matched_sections = match normalized_rust_archive_patch_state {
            NormalizedRustArchivePatchState::Unchanged(patch) => {
                expected_changed_inputs
                    .push(ExpectedInputContent::from_content(path, &input_content));
                previous.input_files[*input_index].content = input_content;
                previous.input_files[*input_index].snapshot_identity = None;
                previous.input_files[*input_index].patch = Some(patch);
                normalized_unchanged_input_count += 1;
                continue;
            }
            NormalizedRustArchivePatchState::MatchedButNotUnchanged(matched) => Some(matched),
            NormalizedRustArchivePatchState::Unknown => None,
        };
        let retain_hashless_atomic_replacement_snapshot =
            can_retain_hashless_atomic_replacement_snapshot(
                path,
                normalized_rust_archive_matched_sections.is_some(),
                trust_rustc_link_content_digests,
            ) && input_content.hash.is_empty();
        if retain_hashless_atomic_replacement_snapshot {
            hashless_atomic_replacement_snapshot_count += 1;
        }
        let defer_loaded_input_content_hash = normalized_rust_archive_matched_sections.is_some()
            && input_content.hash.is_empty()
            && !retain_hashless_atomic_replacement_snapshot;
        if !defer_loaded_input_content_hash && !retain_hashless_atomic_replacement_snapshot {
            ensure_loaded_input_content_hash(&bytes, &mut input_content);
        }
        let expected_changed_input_index = expected_changed_inputs.len();
        expected_changed_inputs.push(ExpectedInputContent::from_content(path, &input_content));
        patched_input_count += 1;

        let (
            fingerprint,
            archive_member_patch_fingerprints,
            archive_member_set_proof,
            matched_sections,
            current_sections,
            resolved_patches,
            fde_eh_frame_hdr_changes,
            input_fde_add_candidates,
            added_dynamic_relocations,
            removed_dynamic_relocations,
            removed_fdes,
            updated_fdes,
        ) = {
            let input = &previous.input_files[*input_index];
            let mut previous_snapshot_bytes =
                LazyInputSnapshotBytes::new(|| read_verified_input_snapshot(state_dir, input));
            timing_phase!("Resolve changed incremental patches");
            let normalize_rust_archive_patch_inputs =
                args.should_normalize_rust_archive_patch_inputs();
            let current_resolver = {
                timing_phase!("Parse changed patch input");
                PatchInputResolver::new(&bytes, normalize_rust_archive_patch_inputs)?
            };
            let archive_member_set_proof = archive_member_set_proof(&bytes)?;
            if !{
                timing_phase!("Compare changed archive members");
                if archive_member_set_proof_matches_current(
                    input,
                    &previous_patch,
                    archive_member_set_proof.as_ref(),
                    normalize_rust_archive_patch_inputs,
                ) == Some(true)
                {
                    true
                } else {
                    let previous_bytes = {
                        timing_phase!("Read previous incremental input snapshot");
                        previous_snapshot_bytes.get()?
                    };
                    archive_members_match_snapshot(
                        state_dir,
                        input,
                        previous_bytes,
                        &bytes,
                        normalize_rust_archive_patch_inputs,
                    )?
                }
            } {
                return Ok(ChangedInputPatchResult::Unsupported(format!(
                    "archive members changed in `{}`",
                    path.display()
                )));
            }
            let mut relocation_target_patches = {
                timing_phase!("Resolve changed relocation targets");
                match relocation_target_patches_for_input(&mut previous.relocations, input, &bytes)?
                {
                    Ok(patches) => patches,
                    Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
                }
            };
            let macho_resolution_updates = {
                timing_phase!("Refresh changed Mach-O symbol resolutions");
                match update_macho_symbol_resolutions_for_input(
                    &mut previous.macho_symbol_resolutions,
                    input,
                    &bytes,
                )? {
                    Ok(updates) => updates,
                    Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
                }
            };
            relocation_target_patches
                .input_ranges
                .extend(macho_resolution_updates.input_ranges);
            relocation_target_patches
                .output_symbols
                .extend(macho_resolution_updates.output_symbols);
            if macho_resolution_updates.changed {
                previous.macho_symbol_resolutions_file = None;
                previous.sections_file = None;
            }
            if input_has_records_requiring_previous_bytes(&previous, input) {
                timing_phase!("Read previous incremental input snapshot");
                previous_snapshot_bytes.get()?;
            }
            let relocation_addend_patches = {
                timing_phase!("Resolve changed relocation addends");
                match relocation_addend_patches_for_input(
                    &mut previous.relocations,
                    input,
                    &bytes,
                    previous_snapshot_bytes.get_if_loaded(),
                    &previous.dynamic_relocations,
                )? {
                    Ok(patches) => patches,
                    Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
                }
            };

            let matched_patch_sections = {
                timing_phase!("Match changed patch sections");
                if let Some(matched) = normalized_rust_archive_matched_sections {
                    Some(matched)
                } else if let Some(matched) =
                    match_patch_sections_from_current_hashes_with_resolver(
                        input.path.as_str(),
                        &previous_patch.sections,
                        &current_resolver,
                    )?
                {
                    Some(matched)
                } else {
                    let previous_bytes = {
                        timing_phase!("Read previous incremental input snapshot");
                        previous_snapshot_bytes.get()?
                    };
                    let previous_resolver = {
                        timing_phase!("Parse previous patch input");
                        previous_bytes
                            .map(|bytes| {
                                PatchInputResolver::new(bytes, normalize_rust_archive_patch_inputs)
                            })
                            .transpose()?
                    };
                    if let Some(previous_resolver) = previous_resolver.as_ref() {
                        match_patch_sections_with_resolvers(
                            input,
                            previous_resolver,
                            &current_resolver,
                            &previous_patch.sections,
                        )?
                    } else {
                        None
                    }
                }
            };

            let (mut matched_sections, matched_changed_sections) = match matched_patch_sections {
                Some(matched_sections) => (
                    matched_sections.sections,
                    Some(matched_sections.changed_sections),
                ),
                None if previous_patch
                    .sections
                    .iter()
                    .any(|section| section.section_name.is_none()) =>
                {
                    return Ok(ChangedInputPatchResult::Unsupported(format!(
                        "could not match anonymous patch sections in `{}`",
                        path.display()
                    )));
                }
                None => (
                    previous_patch
                        .sections
                        .iter()
                        .cloned()
                        .map(MatchedPatchSection::same)
                        .collect(),
                    None,
                ),
            };
            let matched_from_snapshot = matched_changed_sections.is_some();
            let mut current_sections = matched_sections
                .iter()
                .map(|section| section.current.clone())
                .collect::<Vec<_>>();
            if args.should_validate_macho_cstring_patches() {
                let mut boundaries_are_stable = matched_cstring_literal_boundaries_are_stable(
                    input.path.as_str(),
                    &matched_sections,
                    None,
                    &current_resolver,
                )?;
                if !boundaries_are_stable {
                    let previous_bytes = {
                        timing_phase!("Read previous incremental input snapshot");
                        previous_snapshot_bytes.get()?
                    };
                    let previous_resolver = previous_bytes
                        .map(|bytes| {
                            PatchInputResolver::new(bytes, normalize_rust_archive_patch_inputs)
                        })
                        .transpose()?;
                    boundaries_are_stable = matched_cstring_literal_boundaries_are_stable(
                        input.path.as_str(),
                        &matched_sections,
                        previous_resolver.as_ref(),
                        &current_resolver,
                    )?;
                }
                if !boundaries_are_stable {
                    return Ok(ChangedInputPatchResult::Unsupported(format!(
                        "changed Mach-O cstring literal boundaries in `{}`",
                        path.display()
                    )));
                }
            }
            let macho_text_relocation_replays = {
                let previous_bytes = {
                    timing_phase!("Read previous incremental input snapshot");
                    previous_snapshot_bytes.get()?
                };
                let previous_resolver = previous_bytes
                    .map(|bytes| {
                        PatchInputResolver::new(bytes, normalize_rust_archive_patch_inputs)
                    })
                    .transpose()?;
                if let Some(previous_resolver) = previous_resolver.as_ref() {
                    match macho_text_relocation_replays_for_input(
                        &previous.relocations,
                        &mut previous.macho_symbol_resolutions,
                        input,
                        &matched_sections,
                        previous_resolver,
                        &current_resolver,
                        previous_output.get()?,
                        &mut relocation_target_patches,
                    )? {
                        Ok(replays) => replays,
                        Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
                    }
                } else {
                    MachOTextRelocationReplays {
                        replays: Vec::new(),
                        rematerialized_sections: Vec::new(),
                        symbol_resolutions_changed: false,
                    }
                }
            };
            if macho_text_relocation_replays.symbol_resolutions_changed {
                previous.macho_symbol_resolutions_file = None;
                previous.sections_file = None;
            }
            output_symbol_patches.extend(relocation_target_patches.output_symbols.iter().cloned());
            if args.should_validate_x86_64_elf_got_relaxation_contexts() {
                let mut got_contexts_are_stable = {
                    timing_phase!("Validate changed ELF GOT contexts");
                    matched_x86_64_elf_got_relaxation_contexts_are_stable(
                        previous_snapshot_bytes.get_if_loaded(),
                        &bytes,
                        input.path.as_str(),
                        &matched_sections,
                    )?
                };
                if !got_contexts_are_stable {
                    let previous_bytes = {
                        timing_phase!("Read previous incremental input snapshot");
                        previous_snapshot_bytes.get()?
                    };
                    got_contexts_are_stable =
                        matched_x86_64_elf_got_relaxation_contexts_are_stable(
                            previous_bytes,
                            &bytes,
                            input.path.as_str(),
                            &matched_sections,
                        )?;
                }
                if !got_contexts_are_stable {
                    return Ok(ChangedInputPatchResult::Unsupported(format!(
                        "changed x86-64 ELF GOT relaxation context in `{}`",
                        path.display()
                    )));
                }
            }

            let mut dynamic_relocation_patches = dynamic_relocation_patches_for_input(
                &bytes,
                input.path.as_str(),
                previous
                    .dynamic_relocations
                    .iter()
                    .filter(|record| record.input_file == input.path),
            )?;
            if let Some(previous_bytes) = previous_snapshot_bytes.get_if_loaded() {
                dynamic_relocation_patches.extend(added_dynamic_relocation_patches_for_input(
                    &bytes,
                    previous_bytes,
                    input.path.as_str(),
                    &matched_sections,
                    &previous.dynamic_relocations,
                    &previous.sections,
                ));
            }
            let eh_frame_patches =
                if let Some(previous_bytes) = previous_snapshot_bytes.get_if_loaded() {
                    fde_relocation_patches_for_input(
                        &bytes,
                        previous_bytes,
                        input.path.as_str(),
                        previous
                            .fdes
                            .iter()
                            .filter(|record| record.input_file == input.path),
                    )?
                } else {
                    Vec::new()
                };
            let input_fde_add_candidates =
                if let Some(previous_bytes) = previous_snapshot_bytes.get_if_loaded() {
                    added_fde_candidates_for_input(
                        &bytes,
                        previous_bytes,
                        input.path.as_str(),
                        &matched_sections,
                        previous
                            .fdes
                            .iter()
                            .filter(|record| record.input_file == input.path),
                    )?
                } else {
                    Vec::new()
                };
            let fingerprint_extra_ranges = dynamic_relocation_patches
                .iter()
                .filter_map(|patch| patch.input_range.clone())
                .chain(relocation_addend_patches.input_ranges.iter().cloned())
                .chain(relocation_target_patches.input_ranges.iter().cloned())
                .chain(
                    eh_frame_patches
                        .iter()
                        .flat_map(|patch| patch.input_ranges.iter().map(Clone::clone)),
                )
                .chain(
                    input_fde_add_candidates
                        .iter()
                        .flat_map(|candidate| candidate.input_ranges.iter().cloned()),
                )
                .collect::<Vec<_>>();
            let Some((fingerprint, archive_member_patch_fingerprints)) = ({
                timing_phase!("Fingerprint changed patch input");
                patch_fingerprint_with_resolver_and_archive_member_patch_fingerprints(
                    &bytes,
                    input.path.as_str(),
                    current_sections.iter().cloned(),
                    fingerprint_extra_ranges.iter().cloned(),
                    &current_resolver,
                    PatchInputLookup::MatchArchiveMember,
                    normalize_rust_archive_patch_inputs,
                    previous_patch.archive_member_patch_fingerprints.as_deref(),
                )?
            }) else {
                return Ok(ChangedInputPatchResult::Unsupported(format!(
                    "could not resolve patchable sections in `{}`",
                    path.display()
                )));
            };
            if fingerprint != previous_patch.fingerprint {
                let previous_snapshot_bytes = {
                    timing_phase!("Read previous incremental input snapshot");
                    previous_snapshot_bytes.get()?
                };
                let dynamic_relocation_removed = dynamic_relocation_patches
                    .iter()
                    .any(|patch| patch.input_range.is_none());
                let allows_dynamic_relocation_removal = if dynamic_relocation_removed {
                    if let Some(previous_bytes) = previous_snapshot_bytes {
                        object_diff_allows_dynamic_relocation_removal(
                            previous_bytes,
                            &bytes,
                            input.path.as_str(),
                            &matched_sections,
                            &dynamic_relocation_patches,
                        )?
                    } else {
                        false
                    }
                } else {
                    false
                };
                let dynamic_relocation_added = dynamic_relocation_patches
                    .iter()
                    .any(|patch| patch.input_range.is_some());
                let allows_dynamic_relocation_addition = if dynamic_relocation_added {
                    if let Some(previous_bytes) = previous_snapshot_bytes {
                        object_diff_allows_dynamic_relocation_addition(
                            previous_bytes,
                            &bytes,
                            input.path.as_str(),
                            &matched_sections,
                            &dynamic_relocation_patches,
                        )?
                    } else {
                        false
                    }
                } else {
                    false
                };
                let fde_removed = eh_frame_patches.iter().any(|patch| {
                    matches!(patch.eh_frame_hdr_change, Some(EhFrameHdrChange::Remove(_)))
                });
                let allows_fde_removal = if fde_removed {
                    if let Some(previous_bytes) = previous_snapshot_bytes {
                        object_diff_allows_fde_removal(
                            previous_bytes,
                            &bytes,
                            input.path.as_str(),
                            &matched_sections,
                            &eh_frame_patches,
                        )?
                    } else {
                        false
                    }
                } else {
                    false
                };
                let allows_fde_addition = if input_fde_add_candidates.is_empty() {
                    false
                } else {
                    if let Some(previous_bytes) = previous_snapshot_bytes {
                        object_diff_allows_fde_addition(
                            previous_bytes,
                            &bytes,
                            input.path.as_str(),
                            &matched_sections,
                            &input_fde_add_candidates,
                        )?
                    } else {
                        false
                    }
                };
                let metadata_only_fingerprint_matches = !records_complete
                    && previous.sections.is_empty()
                    && previous.relocations.is_empty()
                    && previous.fdes.is_empty()
                    && previous.dynamic_relocations.is_empty()
                    && if let Some(previous_bytes) = previous_snapshot_bytes {
                        patch_fingerprint_matches_previous_without_extra_ranges(
                            previous_bytes,
                            fingerprint.as_str(),
                            input.path.as_str(),
                            &matched_sections,
                            normalize_rust_archive_patch_inputs,
                        )?
                    } else {
                        false
                    };
                if !allows_dynamic_relocation_removal
                    && !allows_dynamic_relocation_addition
                    && !allows_fde_removal
                    && !allows_fde_addition
                    && !metadata_only_fingerprint_matches
                {
                    return Ok(ChangedInputPatchResult::Unsupported(format!(
                        "changed bytes outside patchable sections in `{}`",
                        path.display()
                    )));
                }
            }

            let mut patch_sections = if let Some(changed_sections) = matched_changed_sections {
                changed_sections
            } else {
                let previous_bytes = {
                    timing_phase!("Read previous incremental input snapshot");
                    previous_snapshot_bytes.get()?
                };
                let previous_resolver = previous_bytes
                    .map(|bytes| {
                        PatchInputResolver::new(bytes, normalize_rust_archive_patch_inputs)
                    })
                    .transpose()?;
                if let Some(previous_resolver) = previous_resolver.as_ref() {
                    changed_patch_sections_with_resolvers(
                        input,
                        previous_resolver,
                        &current_resolver,
                        &matched_sections,
                    )?
                    .unwrap_or_else(|| current_sections.clone())
                } else {
                    current_sections.clone()
                }
            };
            let moved_macho_relocations = relocation_target_patches
                .macho_target_moves
                .iter()
                .map(|target_move| target_move.relocation_index)
                .collect::<HashSet<_>>();
            for replay in &macho_text_relocation_replays.replays {
                if !replay
                    .relocation_index
                    .is_some_and(|index| moved_macho_relocations.contains(&index))
                    || patch_sections.iter().any(|section| {
                        section.input == replay.input
                            && section.section_index == replay.section_index
                    })
                {
                    continue;
                }
                let Some(section) = matched_sections.iter().find(|section| {
                    section.current.input == replay.input
                        && section.current.section_index == replay.section_index
                }) else {
                    return Ok(ChangedInputPatchResult::Unsupported(format!(
                        "missing Mach-O text section for moved target in `{}`",
                        path.display()
                    )));
                };
                patch_sections.push(section.current.clone());
            }
            patched_section_count += patch_sections.len();

            let Some(mut resolved_patches) = ({
                timing_phase!("Materialize changed section patches");
                resolved_patch_sections_for_input_with_resolver(
                    input.path.as_str(),
                    patch_sections,
                    dynamic_relocation_patches.iter().map(|patch| &patch.record),
                    previous
                        .relocations
                        .iter()
                        .filter(|record| record.input_file == input.path),
                    &current_resolver,
                )?
            }) else {
                return Ok(ChangedInputPatchResult::Unsupported(format!(
                    "changed patchable section size in `{}`",
                    path.display()
                )));
            };
            if !macho_text_relocation_replays
                .rematerialized_sections
                .is_empty()
                && !records_complete
            {
                return Ok(ChangedInputPatchResult::Unsupported(
                    "changed input needs complete Mach-O text relocation records".to_owned(),
                ));
            }
            if (!macho_text_relocation_replays.replays.is_empty()
                || !macho_text_relocation_replays
                    .rematerialized_sections
                    .is_empty())
                && let Err(reason) = apply_macho_text_relocation_replays(
                    &mut resolved_patches,
                    &macho_text_relocation_replays,
                    previous_output.get()?,
                    &mut previous.relocations,
                )
            {
                return Ok(ChangedInputPatchResult::Unsupported(reason));
            }
            if !macho_text_relocation_replays
                .rematerialized_sections
                .is_empty()
            {
                previous.sections_file = None;
            }
            if !matched_from_snapshot {
                if resolved_patches.len() == current_sections.len() {
                    current_sections = resolved_patches
                        .iter()
                        .map(|resolved| resolved.section.clone())
                        .collect();
                } else {
                    let Some(resolved_sections) = resolve_current_patch_sections_with_resolver(
                        input.path.as_str(),
                        current_sections.iter().cloned(),
                        dynamic_relocation_patches.iter().map(|patch| &patch.record),
                        previous
                            .relocations
                            .iter()
                            .filter(|record| record.input_file == input.path),
                        &current_resolver,
                    )?
                    else {
                        return Ok(ChangedInputPatchResult::Unsupported(format!(
                            "changed patchable section size in `{}`",
                            path.display()
                        )));
                    };
                    current_sections = resolved_sections;
                }
            }
            update_matched_patch_current_sections(&mut matched_sections, &current_sections);
            patched_section_count += dynamic_relocation_patches.len();
            patched_section_count += relocation_addend_patches.output_patches.len();
            patched_section_count += eh_frame_patches
                .iter()
                .filter(|patch| patch.patch.is_some())
                .count();
            let eh_frame_hdr_changes = eh_frame_patches
                .iter()
                .filter_map(|patch| patch.eh_frame_hdr_change.clone())
                .collect::<Vec<_>>();
            let removed_dynamic_relocations = dynamic_relocation_patches
                .iter()
                .filter_map(|patch| patch.input_range.is_none().then_some(patch.record.clone()))
                .collect::<HashSet<_>>();
            let added_dynamic_relocations = dynamic_relocation_patches
                .iter()
                .filter_map(|patch| {
                    (patch.input_range.is_some() && patch.is_new).then_some(patch.record.clone())
                })
                .collect::<HashSet<_>>();
            let removed_fdes = eh_frame_hdr_changes
                .iter()
                .filter_map(|change| match change {
                    EhFrameHdrChange::Remove(fde) => Some(fde.clone()),
                    EhFrameHdrChange::Adjust(_) | EhFrameHdrChange::Add(_) => None,
                })
                .collect::<HashSet<_>>();
            let updated_fdes = eh_frame_patches
                .iter()
                .filter_map(|patch| patch.record_update.clone())
                .collect::<Vec<_>>();

            (
                fingerprint,
                archive_member_patch_fingerprints,
                archive_member_set_proof,
                matched_sections,
                current_sections,
                resolved_patches
                    .into_iter()
                    .map(|resolved| resolved.patch)
                    .chain(
                        dynamic_relocation_patches
                            .into_iter()
                            .map(|relocation| relocation.patch),
                    )
                    .chain(relocation_addend_patches.output_patches)
                    .chain(relocation_target_patches.output_patches)
                    .chain(eh_frame_patches.into_iter().filter_map(|fde| fde.patch))
                    .collect::<Vec<_>>(),
                eh_frame_hdr_changes,
                input_fde_add_candidates,
                added_dynamic_relocations,
                removed_dynamic_relocations,
                removed_fdes,
                updated_fdes,
            )
        };

        let sections_changed = update_section_records_for_matched_patches(
            previous.input_files[*input_index].path.as_str(),
            &matched_sections,
            &mut previous.sections,
        );
        if sections_changed {
            if !records_complete {
                return Ok(ChangedInputPatchResult::Unsupported(
                    "changed input needs complete section records".to_owned(),
                ));
            }
            previous.sections_file = None;
        }
        if !added_dynamic_relocations.is_empty() {
            if !records_complete {
                return Ok(ChangedInputPatchResult::Unsupported(
                    "changed input needs complete dynamic relocation records".to_owned(),
                ));
            }
            previous
                .dynamic_relocations
                .extend(added_dynamic_relocations);
            previous.dynamic_relocations.sort();
            previous.dynamic_relocations.dedup();
            previous.sections_file = None;
        }
        if !removed_dynamic_relocations.is_empty() {
            if !records_complete {
                return Ok(ChangedInputPatchResult::Unsupported(
                    "changed input needs complete dynamic relocation records".to_owned(),
                ));
            }
            previous.dynamic_relocations.retain(|record| {
                !removed_dynamic_relocations.contains(record)
                    || record.has_restorable_rela_output_info()
            });
            previous.sections_file = None;
        }
        if !removed_fdes.is_empty() {
            if !records_complete {
                return Ok(ChangedInputPatchResult::Unsupported(
                    "changed input needs complete FDE records".to_owned(),
                ));
            }
            previous
                .fdes
                .retain(|record| !removed_fdes.contains(record));
            previous.sections_file = None;
        }
        if !updated_fdes.is_empty() {
            if !records_complete {
                return Ok(ChangedInputPatchResult::Unsupported(
                    "changed input needs complete FDE records".to_owned(),
                ));
            }
            update_fde_records(&mut previous.fdes, updated_fdes);
            previous.sections_file = None;
        }
        previous.input_files[*input_index].content = input_content;
        previous.input_files[*input_index].snapshot_identity = None;
        previous.input_files[*input_index].patch = Some(FilePatchState {
            fingerprint: fingerprint.clone(),
            archive_member_set_proof,
            archive_member_patch_fingerprints,
            sections: current_sections
                .iter()
                .map(|section| FilePatchSectionState {
                    input: section.input.clone(),
                    section_index: section.section_index,
                    section_name: section.section_name.clone(),
                    input_size: section.input_size,
                    output_offset: section.output_offset,
                    output_size: section.output_size,
                    data_hash: section.data_hash.clone(),
                    cstring_nul_boundaries_hash: section.cstring_nul_boundaries_hash.clone(),
                })
                .collect(),
            raw_sections: None,
        });
        patches.extend(resolved_patches);
        eh_frame_hdr_changes.extend(fde_eh_frame_hdr_changes);
        fde_add_candidates.extend(input_fde_add_candidates);
        if defer_loaded_input_content_hash {
            deferred_loaded_input_content_hashes.push((
                *input_index,
                expected_changed_input_index,
                bytes,
            ));
        }
    }

    if patched_input_count == 0 {
        if let Some(reason) = input_content_mismatch_reason(&expected_changed_inputs, None) {
            return Ok(ChangedInputPatchResult::Unsupported(reason));
        }
        if let Some(reason) = input_identity_mismatch_reason(&previous.input_files)? {
            return Ok(ChangedInputPatchResult::Unsupported(reason));
        }
        snapshot_input_paths(
            state_dir,
            changed_inputs_requiring_snapshot(
                changed_inputs,
                &rustc_link_content_digest_unchanged_input_indices,
            )
            .map(|(_, path)| path.as_path()),
        )?;
        refresh_input_snapshot_identities_at_indices(
            state_dir,
            &mut previous.input_files,
            changed_inputs_requiring_snapshot(
                changed_inputs,
                &rustc_link_content_digest_unchanged_input_indices,
            )
            .map(|(input_index, _)| *input_index),
        );
        refresh_input_file_identities_at_indices(
            &mut previous.input_files,
            changed_inputs.iter().map(|(input_index, _)| *input_index),
        );
        if let Some(reason) =
            input_content_mismatch_reason(&expected_changed_inputs, Some(state_dir))
        {
            return Ok(ChangedInputPatchResult::Unsupported(reason));
        }
        if let Some(reason) = input_identity_mismatch_reason(&previous.input_files)? {
            return Ok(ChangedInputPatchResult::Unsupported(reason));
        }
        previous.link_start = current_link_start;
        previous.write_metadata_update_for_inputs(state_dir, metadata_update_input_indices)?;
        if rewritten_input_count > 0 {
            append_log(
                state_dir,
                &format!(
                    "updated {rewritten_input_count} rewritten input file{} before loading inputs",
                    if rewritten_input_count == 1 { "" } else { "s" }
                ),
            )?;
        }
        if normalized_unchanged_input_count > 0 {
            append_log(
                state_dir,
                &format!(
                    "updated {normalized_unchanged_input_count} unchanged normalized Rust archive input file{} before loading inputs",
                    if normalized_unchanged_input_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
            )?;
        }
        if rustc_link_content_digest_unchanged_input_count > 0 {
            append_log(
                state_dir,
                &format!(
                    "reused {rustc_link_content_digest_unchanged_input_count} unchanged Rust archive input file{} by rustc link-content digest before loading inputs",
                    if rustc_link_content_digest_unchanged_input_count == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
            )?;
        }
        append_log(state_dir, "reused existing output before loading inputs")?;
        return Ok(ChangedInputPatchResult::Patched);
    }

    if let Some(reason) = input_content_mismatch_reason(&expected_changed_inputs, None) {
        return Ok(ChangedInputPatchResult::Unsupported(reason));
    }

    if let Some(reason) = input_identity_mismatch_reason(&previous.input_files)? {
        return Ok(ChangedInputPatchResult::Unsupported(reason));
    }

    if let Some(reason) = patch_output_range_rejection_reason(&patches) {
        return Ok(ChangedInputPatchResult::Unsupported(reason));
    }

    let Some(mut directly_patched_output) = DirectlyPatchedOutput::new(
        args.output(),
        state_dir,
        args.should_replace_directly_patched_output(),
    ) else {
        return Ok(ChangedInputPatchResult::Unsupported(
            "could not clone directly patched output generation".to_owned(),
        ));
    };
    let should_invalidate_code_signature_cache =
        directly_patched_output.should_invalidate_code_signature_cache();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directly_patched_output.path())
        .with_context(|| {
            format!(
                "Failed to open output `{}` for incremental patching",
                directly_patched_output.path().display()
            )
        })?;
    let mut output = if directly_patched_output.is_generation() {
        // A shared writable mapping can leave the kernel's code-signing cache stale even after an
        // atomic rename. Patch private pages, then publish them with ordinary file writes.
        unsafe { MmapOptions::new().map_copy(&file) }
    } else {
        unsafe { MmapOptions::new().map_mut(&file) }
    }
    .with_context(|| {
        format!(
            "Failed to mmap output `{}` for incremental patching",
            directly_patched_output.path().display()
        )
    })?;
    match output_symbol_value_patches(&output, &output_symbol_patches)? {
        Ok(symbol_patches) => patches.extend(symbol_patches),
        Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
    }
    match fde_add_patches_for_output(&output, &fde_add_candidates, &previous.fdes)? {
        Ok(resolved_fdes) => {
            if !records_complete && !resolved_fdes.is_empty() {
                return Ok(ChangedInputPatchResult::Unsupported(
                    "changed input needs complete FDE records".to_owned(),
                ));
            }
            patched_section_count += resolved_fdes.len();
            for resolved in resolved_fdes {
                previous.fdes.push(resolved.record);
                patches.push(resolved.patch);
                if let Some(change) = resolved.eh_frame_hdr_change {
                    eh_frame_hdr_changes.push(change);
                }
                previous.sections_file = None;
            }
        }
        Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
    }
    match eh_frame_hdr_patches_for_fde_changes(&output, &eh_frame_hdr_changes)? {
        Ok(header_patches) => patches.extend(header_patches),
        Err(reason) => return Ok(ChangedInputPatchResult::Unsupported(reason)),
    }
    if let Some(reason) = patch_output_range_rejection_reason(&patches) {
        return Ok(ChangedInputPatchResult::Unsupported(reason));
    }

    let build_id_range = build_id_note_range(&output)?;
    if build_id_range.is_some() && !args.has_incremental_fast_build_id() {
        return Ok(ChangedInputPatchResult::Unsupported(
            "output has a build ID that cannot be updated incrementally".to_owned(),
        ));
    }
    let mut build_id_tree = None;
    let mut build_id_hashes = None;
    if build_id_range.is_some() {
        let Some(previous_hashes) = previous.build_id_hashes.as_ref() else {
            return Ok(ChangedInputPatchResult::Unsupported(
                "missing build ID hash state".to_owned(),
            ));
        };
        let Ok(tree) = read_build_id_hash_tree(state_dir, previous_hashes) else {
            return Ok(ChangedInputPatchResult::Unsupported(
                "could not read build ID hash state".to_owned(),
            ));
        };
        build_id_tree = Some(tree);
        build_id_hashes = Some(previous_hashes.clone());
    }

    mark_incremental_update_started(state_dir, "patch changed inputs")?;

    let mut patched_ranges = Vec::new();
    {
        timing_phase!("Write changed incremental output ranges");
        for mut patch in patches {
            let start = patch.output_offset as usize;
            let end = start
                .checked_add(patch.size as usize)
                .context("Incremental patch output range overflow")?;
            let Some(output_range) = output.get_mut(start..end) else {
                return Ok(ChangedInputPatchResult::StartedUnsupported(
                    "changed patch output range is out of bounds".to_owned(),
                ));
            };
            if patch.data.len() > output_range.len() {
                return Ok(ChangedInputPatchResult::StartedUnsupported(
                    "changed patch data does not fit in the previous output range".to_owned(),
                ));
            }
            if let Some(deferred_relocation) = patch.deferred_relocation
                && let Err(reason) = materialize_deferred_relocation_patch(
                    &mut patch.data,
                    output_range,
                    deferred_relocation,
                )
            {
                return Ok(ChangedInputPatchResult::StartedUnsupported(reason));
            }
            for preserve_range in &patch.preserve_ranges {
                let Some(data_range) = patch.data.get_mut(preserve_range.clone()) else {
                    return Ok(ChangedInputPatchResult::StartedUnsupported(
                        "changed patch preserve range is out of bounds".to_owned(),
                    ));
                };
                let Some(previous_range) = output_range.get(preserve_range.clone()) else {
                    return Ok(ChangedInputPatchResult::StartedUnsupported(
                        "changed patch preserve range is out of bounds".to_owned(),
                    ));
                };
                data_range.copy_from_slice(previous_range);
            }
            for adjustment in &patch.adjustments {
                let Some(data_range) = patch.data.get_mut(adjustment.range.clone()) else {
                    return Ok(ChangedInputPatchResult::StartedUnsupported(
                        "changed patch adjustment range is out of bounds".to_owned(),
                    ));
                };
                if let Err(reason) = apply_addend_delta(data_range, adjustment.addend_delta) {
                    return Ok(ChangedInputPatchResult::StartedUnsupported(reason));
                }
            }
            let (data_out, padding) = output_range.split_at_mut(patch.data.len());
            data_out.copy_from_slice(&patch.data);
            padding.fill(0);
            patched_ranges.push(start..end);
        }
    }

    let mut flush_ranges = patched_ranges.clone();
    if let Some(range) = build_id_range {
        let previous_hashes = build_id_hashes
            .as_mut()
            .context("Missing incremental build ID hash state")?;
        let tree = build_id_tree
            .as_mut()
            .context("Missing incremental build ID hash tree")?;
        flush_ranges.push(range.clone());
        write_fast_build_id_from_state(&mut output, range, previous_hashes, tree, &patched_ranges)?;
    }
    let (changed_input_snapshot_result, deferred_loaded_input_content_hashes) =
        if args.should_snapshot_changed_inputs_while_finalizing_direct_patches() {
            let (deferred_loaded_input_content_hashes, snapshot_result) =
                std::thread::scope(|scope| -> Result<_> {
                    let background = std::thread::Builder::new()
                        .name("sld-input-snapshot".to_owned())
                        .spawn_scoped(scope, || {
                            let snapshot_result = {
                                timing_phase!("Snapshot changed incremental inputs");
                                snapshot_input_paths(
                                    state_dir,
                                    changed_inputs_requiring_snapshot(
                                        changed_inputs,
                                        &rustc_link_content_digest_unchanged_input_indices,
                                    )
                                    .map(|(_, path)| path.as_path()),
                                )
                            };
                            let deferred_loaded_input_content_hashes =
                                hash_deferred_loaded_input_contents(
                                    &deferred_loaded_input_content_hashes,
                                    false,
                                );
                            (deferred_loaded_input_content_hashes, snapshot_result)
                        })
                        .context("Failed to spawn incremental input snapshot thread")?;
                    let finalization_result = args.finalize_directly_patched_output(
                        &mut output,
                        &mut flush_ranges,
                        should_invalidate_code_signature_cache,
                    );
                    let background_result = background
                        .join()
                        .map_err(|_| crate::error!("Incremental input snapshot thread panicked"))?;
                    finalization_result?;
                    Ok(background_result)
                })?;
            (Some(snapshot_result), deferred_loaded_input_content_hashes)
        } else {
            args.finalize_directly_patched_output(
                &mut output,
                &mut flush_ranges,
                should_invalidate_code_signature_cache,
            )?;
            (
                None,
                hash_deferred_loaded_input_contents(&deferred_loaded_input_content_hashes, true),
            )
        };
    install_deferred_loaded_input_content_hashes(
        &mut previous.input_files,
        &mut expected_changed_inputs,
        deferred_loaded_input_content_hashes,
    )?;

    {
        timing_phase!("Flush changed incremental output ranges");
        if directly_patched_output.is_generation() {
            write_output_ranges(
                &output,
                &flush_ranges,
                &mut file,
                directly_patched_output.path(),
            )?;
        } else {
            flush_output_ranges(&output, &flush_ranges, directly_patched_output.path())?;
        }
    }
    drop(output);
    drop(file);
    directly_patched_output.install(&flush_ranges)?;

    let output = directly_patched_output_content_state(args, args.output()).with_context(|| {
        format!(
            "Failed to record patched output `{}` for incremental state",
            args.output().display()
        )
    })?;
    if args.should_retain_output_snapshot() {
        timing_phase!("Update incremental output snapshot");
        update_output_snapshot_from_ranges(state_dir, args.output(), &flush_ranges)?;
    }
    write_build_id_hash_tree(state_dir, build_id_tree.as_deref())?;
    {
        changed_input_snapshot_result.unwrap_or_else(|| {
            timing_phase!("Snapshot changed incremental inputs");
            snapshot_input_paths(
                state_dir,
                changed_inputs_requiring_snapshot(
                    changed_inputs,
                    &rustc_link_content_digest_unchanged_input_indices,
                )
                .map(|(_, path)| path.as_path()),
            )
        })?;
        refresh_input_snapshot_identities_at_indices(
            state_dir,
            &mut previous.input_files,
            changed_inputs_requiring_snapshot(
                changed_inputs,
                &rustc_link_content_digest_unchanged_input_indices,
            )
            .map(|(input_index, _)| *input_index),
        );
        refresh_input_file_identities_at_indices(
            &mut previous.input_files,
            changed_inputs.iter().map(|(input_index, _)| *input_index),
        );
    }
    if let Some(reason) = input_content_mismatch_reason(&expected_changed_inputs, Some(state_dir)) {
        return Ok(ChangedInputPatchResult::StartedUnsupported(reason));
    }
    if let Some(reason) = input_identity_mismatch_reason(&previous.input_files)? {
        return Ok(ChangedInputPatchResult::StartedUnsupported(reason));
    }
    {
        timing_phase!("Persist changed incremental patch metadata");
        PersistedState {
            args_hash: previous.args_hash,
            link_options_hash: previous.link_options_hash,
            input_order_hash: previous.input_order_hash,
            sld_version: previous.sld_version,
            link_start: current_link_start,
            output,
            build_id_hashes,
            input_files: previous.input_files,
            sections: previous.sections,
            relocations: previous.relocations,
            macho_symbol_resolutions: previous.macho_symbol_resolutions,
            fdes: previous.fdes,
            dynamic_relocations: previous.dynamic_relocations,
            sections_file: previous.sections_file,
            macho_symbol_resolutions_file: previous.macho_symbol_resolutions_file,
            patch_records_file: previous.patch_records_file,
            patch_record_locations: previous.patch_record_locations,
            raw_patch_record_locations: previous.raw_patch_record_locations,
        }
        .write_metadata_update_for_inputs(state_dir, metadata_update_input_indices)?;
    }
    clear_incremental_update_marker(state_dir)?;

    append_log(
        state_dir,
        &format!(
            "patched {} changed input file{} before loading inputs",
            patched_input_count,
            if patched_input_count == 1 { "" } else { "s" }
        ),
    )?;
    if rewritten_input_count > 0 {
        append_log(
            state_dir,
            &format!(
                "updated {rewritten_input_count} rewritten input file{} before loading inputs",
                if rewritten_input_count == 1 { "" } else { "s" }
            ),
        )?;
    }
    if normalized_unchanged_input_count > 0 {
        append_log(
            state_dir,
            &format!(
                "updated {normalized_unchanged_input_count} unchanged normalized Rust archive input file{} before loading inputs",
                if normalized_unchanged_input_count == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        )?;
    }
    if rustc_link_content_digest_unchanged_input_count > 0 {
        append_log(
            state_dir,
            &format!(
                "reused {rustc_link_content_digest_unchanged_input_count} unchanged Rust archive input file{} by rustc link-content digest before loading inputs",
                if rustc_link_content_digest_unchanged_input_count == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        )?;
    }
    if hashless_atomic_replacement_snapshot_count > 0 {
        append_log(
            state_dir,
            &format!(
                "retained {hashless_atomic_replacement_snapshot_count} hashless atomic-replacement input snapshot{} before loading inputs",
                if hashless_atomic_replacement_snapshot_count == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        )?;
    }
    append_log(
        state_dir,
        &format!("patched {patched_section_count} changed input sections before loading inputs"),
    )?;
    Ok(ChangedInputPatchResult::Patched)
}

fn directly_patched_output_content_state(
    args: &impl platform::Args,
    output: &Path,
) -> Result<FileContentState> {
    if args.should_hash_directly_patched_output() || args.should_retain_output_snapshot() {
        FileContentState::from_path(output)
    } else {
        FileContentState::from_path_identity_only(output)
    }
}

struct PreviousPatchState {
    fingerprint: String,
    archive_member_patch_fingerprints: Option<Vec<ArchiveMemberPatchFingerprint>>,
    sections: Vec<PatchSection>,
}

fn can_normalize_rust_archive_patch(
    args: &impl platform::Args,
    previous: &PersistedState,
    input_index: usize,
    path: &Path,
) -> bool {
    args.should_normalize_rust_archive_patch_inputs()
        && path
            .extension()
            .is_some_and(|extension| extension == "rlib")
        && previous
            .input_files
            .get(input_index)
            .is_some_and(|input| !input_has_records_requiring_previous_bytes(previous, input))
}

fn can_retain_hashless_atomic_replacement_snapshot(
    path: &Path,
    normalized_rust_archive_matched_sections: bool,
    trust_rustc_link_content_digests: bool,
) -> bool {
    normalized_rust_archive_matched_sections
        && trust_rustc_link_content_digests
        && is_atomic_replacement_rust_input(path)
}

fn input_has_records_requiring_previous_bytes(
    previous: &PersistedState,
    input: &FileState,
) -> bool {
    previous.relocations.iter().any(|relocation| {
        relocation.input_file == input.path
            || relocation
                .target
                .as_ref()
                .is_some_and(|target| target.input_file == input.path)
    }) || previous
        .dynamic_relocations
        .iter()
        .any(|relocation| relocation.input_file == input.path)
        || previous
            .fdes
            .iter()
            .any(|record| record.input_file == input.path)
}

fn classify_normalized_rust_archive_patch_state(
    input: &FileState,
    bytes: &[u8],
    previous_patch: &PreviousPatchState,
) -> Result<NormalizedRustArchivePatchState> {
    verbose_timing_phase!("Classify normalized unchanged archive input");
    let resolver = PatchInputResolver::new(bytes, true)?;
    let Some(matched) = match_patch_sections_from_current_hashes_with_resolver(
        input.path.as_str(),
        &previous_patch.sections,
        &resolver,
    )?
    else {
        return Ok(NormalizedRustArchivePatchState::Unknown);
    };
    if !matched.changed_sections.is_empty() {
        return Ok(NormalizedRustArchivePatchState::MatchedButNotUnchanged(
            matched,
        ));
    }
    let current_sections = matched
        .sections
        .into_iter()
        .map(|section| section.current)
        .collect::<Vec<_>>();
    let Some((fingerprint, archive_member_patch_fingerprints)) =
        patch_fingerprint_with_resolver_and_archive_member_patch_fingerprints(
            bytes,
            input.path.as_str(),
            current_sections.iter().cloned(),
            std::iter::empty(),
            &resolver,
            PatchInputLookup::MatchArchiveMember,
            true,
            previous_patch.archive_member_patch_fingerprints.as_deref(),
        )?
    else {
        return Ok(NormalizedRustArchivePatchState::Unknown);
    };
    if fingerprint != previous_patch.fingerprint {
        return Ok(NormalizedRustArchivePatchState::Unknown);
    }
    Ok(NormalizedRustArchivePatchState::Unchanged(FilePatchState {
        fingerprint,
        archive_member_set_proof: archive_member_set_proof(bytes)?,
        archive_member_patch_fingerprints,
        sections: current_sections
            .iter()
            .map(|section| FilePatchSectionState {
                input: section.input.clone(),
                section_index: section.section_index,
                section_name: section.section_name.clone(),
                input_size: section.input_size,
                output_offset: section.output_offset,
                output_size: section.output_size,
                data_hash: section.data_hash.clone(),
                cstring_nul_boundaries_hash: section.cstring_nul_boundaries_hash.clone(),
            })
            .collect(),
        raw_sections: None,
    }))
}

fn patch_sections_from_previous_state(
    input: &FileState,
    path: &Path,
) -> std::result::Result<PreviousPatchState, String> {
    let Some(previous_patch) = input.patch.as_ref() else {
        return Err(format!("missing patch metadata for `{}`", path.display()));
    };
    let sections = if previous_patch.sections.is_empty() {
        previous_patch
            .raw_sections
            .as_ref()
            .map(|raw| parse_patch_sections(input.path.as_str(), raw))
            .transpose()
            .map_err(|error| format!("{error:?}"))?
            .unwrap_or_default()
    } else {
        previous_patch.sections.clone()
    };
    if sections.is_empty() {
        return Err(format!(
            "no patchable sections recorded for `{}`",
            path.display()
        ));
    }
    let patch_section_keys = sections
        .iter()
        .map(|section| (section.input.as_str(), section.section_index))
        .collect::<HashSet<_>>();
    if patch_section_keys.len() != sections.len() {
        return Err(format!(
            "duplicate patchable section metadata for `{}`",
            path.display()
        ));
    }
    Ok(PreviousPatchState {
        fingerprint: previous_patch.fingerprint.clone(),
        archive_member_patch_fingerprints: previous_patch.archive_member_patch_fingerprints.clone(),
        sections: sections
            .iter()
            .map(|section| PatchSection {
                input: section.input.clone(),
                section_index: section.section_index,
                section_name: section.section_name.clone(),
                input_size: section.input_size,
                output_offset: section.output_offset,
                output_size: section.output_size,
                data_hash: section.data_hash.clone(),
                cstring_nul_boundaries_hash: section.cstring_nul_boundaries_hash.clone(),
            })
            .collect(),
    })
}

struct SectionPatch {
    output_offset: u64,
    size: u64,
    data: Vec<u8>,
    deferred_relocation: Option<DeferredRelocationPatch>,
    preserve_ranges: Vec<std::ops::Range<usize>>,
    adjustments: Vec<PatchAdjustment>,
}

#[derive(Clone, Copy)]
struct DeferredRelocationPatch {
    rel_info: RelocationKindInfo,
    previous_written_value: u64,
    written_value: u64,
}

struct PatchAdjustment {
    range: std::ops::Range<usize>,
    addend_delta: i64,
}

struct ResolvedSectionPatch {
    section: PatchSection,
    patch: SectionPatch,
}

struct DynamicRelocationPatch {
    record: DynamicRelocationRecord,
    input_range: Option<std::ops::Range<usize>>,
    patch: SectionPatch,
    is_new: bool,
}

struct RelocationAddendPatches {
    input_ranges: Vec<std::ops::Range<usize>>,
    output_patches: Vec<SectionPatch>,
}

struct RelocationTargetPatches {
    input_ranges: Vec<std::ops::Range<usize>>,
    output_patches: Vec<SectionPatch>,
    output_symbols: Vec<RelocationTargetSymbolPatch>,
    macho_target_moves: Vec<MachORelocationTargetMove>,
}

struct MachOSymbolResolutionUpdates {
    input_ranges: Vec<std::ops::Range<usize>>,
    output_symbols: Vec<RelocationTargetSymbolPatch>,
    changed: bool,
}

#[derive(Clone, Copy)]
struct MachORelocationTargetMove {
    relocation_index: usize,
    current_section_offset: u64,
    current_target_value: u64,
}

#[derive(Clone)]
struct RelocationTargetSymbolPatch {
    target_name: String,
    previous_target_value: u64,
    target_value: u64,
    allow_missing: bool,
}

struct FdeRelocationPatch {
    input_ranges: Vec<std::ops::Range<usize>>,
    patch: Option<SectionPatch>,
    eh_frame_hdr_change: Option<EhFrameHdrChange>,
    record_update: Option<FdeRecordUpdate>,
}

#[derive(Clone)]
struct FdeRecordUpdate {
    previous: FdeRecord,
    current: FdeRecord,
}

struct FdeAddCandidate {
    input_ranges: Vec<std::ops::Range<usize>>,
    input_file: String,
    input: String,
    target_section_index: u32,
    eh_frame_section_index: u32,
    input_offset: u64,
    target_section_offset: u64,
    target_output_offset: u64,
    fde_data: Vec<u8>,
    pc_begin_range: std::ops::Range<usize>,
    cie_input_offset: u64,
    cie_reference_fde_output_offset: u64,
}

struct ResolvedFdeAdd {
    patch: SectionPatch,
    record: FdeRecord,
    eh_frame_hdr_change: Option<EhFrameHdrChange>,
}

#[derive(Clone)]
enum EhFrameHdrChange {
    Adjust(EhFrameHdrDelta),
    Remove(FdeRecord),
    Add(EhFrameHdrEntryPatch),
}

#[derive(Clone)]
struct EhFrameHdrDelta {
    fde_output_offset: u64,
    frame_ptr_delta: i64,
}

const GENERATED_SECTION_INPUT_FILE: &str = "generated";
const GENERATED_SECTION_INDEX: u32 = 0;

struct ExpectedInputContent {
    path: PathBuf,
    len: u64,
    hash: String,
    identity: Option<FileIdentity>,
}

impl ExpectedInputContent {
    #[cfg(test)]
    fn from_bytes(path: &Path, bytes: &[u8]) -> Self {
        let content = FileContentState::from_bytes(bytes);
        Self::from_content(path, &content)
    }

    fn from_content(path: &Path, content: &FileContentState) -> Self {
        Self {
            path: path.to_owned(),
            len: content.len,
            hash: content.hash.clone(),
            identity: content.identity.clone(),
        }
    }

    fn matches_unchanged_atomic_replacement_input(&self) -> bool {
        if !is_atomic_replacement_rust_input(&self.path) {
            return false;
        }
        let Some(expected_identity) = self.identity.as_ref() else {
            return false;
        };
        FileIdentity::from_path(&self.path)
            .ok()
            .flatten()
            .as_ref()
            .is_some_and(|identity| identity == expected_identity)
    }

    fn matches_installed_atomic_replacement_snapshot(&self, state_dir: &Path) -> bool {
        if !is_atomic_replacement_rust_input(&self.path) {
            return false;
        }
        let Some(expected_identity) = self.identity.as_ref() else {
            return false;
        };
        let Ok(Some(current_identity)) = FileIdentity::from_path(&self.path) else {
            return false;
        };
        let Ok(Some(snapshot_identity)) =
            FileIdentity::from_path(&input_snapshot_path(state_dir, &self.path))
        else {
            return false;
        };
        expected_identity.matches_same_data_ignoring_change_time(&current_identity)
            && current_identity == snapshot_identity
    }
}

fn patch_output_range_rejection_reason(patches: &[SectionPatch]) -> Option<String> {
    let mut ranges = Vec::with_capacity(patches.len());
    for patch in patches {
        let Ok(start) = usize::try_from(patch.output_offset) else {
            return Some("changed patch output range is out of bounds".to_owned());
        };
        let Some(end) = start.checked_add(patch.size as usize) else {
            return Some("changed patch output range overflow".to_owned());
        };
        ranges.push(start..end);
    }
    ranges.sort_by_key(|range| range.start);

    let mut previous_end = 0;
    for range in ranges {
        if !range.is_empty() && range.start < previous_end {
            return Some("changed patch output ranges overlap".to_owned());
        }
        previous_end = previous_end.max(range.end);
    }
    None
}

fn apply_addend_delta(data: &mut [u8], addend_delta: i64) -> std::result::Result<(), String> {
    match data.len() {
        4 => {
            let value = i64::from(i32::from_le_bytes(data.try_into().unwrap()));
            let adjusted = value
                .checked_add(addend_delta)
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| "changed .eh_frame relocation adjustment overflowed".to_owned())?;
            data.copy_from_slice(&adjusted.to_le_bytes());
            Ok(())
        }
        8 => {
            let value = i64::from_le_bytes(data.try_into().unwrap());
            let adjusted = value
                .checked_add(addend_delta)
                .ok_or_else(|| "changed .eh_frame relocation adjustment overflowed".to_owned())?;
            data.copy_from_slice(&adjusted.to_le_bytes());
            Ok(())
        }
        _ => Err("unsupported .eh_frame relocation field size for incremental patch".to_owned()),
    }
}

fn materialize_deferred_relocation_patch(
    data: &mut [u8],
    previous_output: &[u8],
    deferred_relocation: DeferredRelocationPatch,
) -> std::result::Result<(), String> {
    if data.len() != previous_output.len() {
        return Err("deferred relocation patch output size changed".to_owned());
    }
    let RelocationSize::BitMasking(mask) = deferred_relocation.rel_info.size else {
        return Err("deferred relocation patch is not instruction-shaped".to_owned());
    };
    if mask.instruction.write_windows_size() != data.len() {
        return Err("deferred relocation patch output size changed".to_owned());
    }

    let mut replayed_previous_output = previous_output.to_vec();
    clear_deferred_relocation_bits(&mut replayed_previous_output, mask);
    deferred_relocation
        .rel_info
        .write_to_buffer(
            deferred_relocation.previous_written_value,
            &mut replayed_previous_output,
        )
        .map_err(|error| format!("failed to validate deferred relocation patch: {error:#}"))?;
    if replayed_previous_output != previous_output {
        return Err("deferred relocation patch encoding changed".to_owned());
    }

    data.copy_from_slice(previous_output);
    clear_deferred_relocation_bits(data, mask);
    deferred_relocation
        .rel_info
        .write_to_buffer(deferred_relocation.written_value, data)
        .map_err(|error| format!("failed to encode deferred relocation patch: {error:#}"))
}

fn clear_deferred_relocation_bits(data: &mut [u8], mask: linker_utils::elf::BitMask) {
    if data.len() != 4 {
        return;
    }
    let preserved_bits = mask.instruction.bit_mask(mask.range);
    for (byte, preserved) in data.iter_mut().zip(preserved_bits) {
        *byte &= preserved;
    }
}

struct DirectlyPatchedOutput {
    path: PathBuf,
    published_path: Option<PathBuf>,
    published_identity: Option<FileIdentity>,
    standby_state_path: Option<PathBuf>,
}

impl DirectlyPatchedOutput {
    fn new(output: &Path, state_dir: &Path, should_replace: bool) -> Option<Self> {
        if !should_replace {
            return Some(Self {
                path: output.to_path_buf(),
                published_path: None,
                published_identity: None,
                standby_state_path: None,
            });
        }

        let published_identity = FileIdentity::from_path(output).ok().flatten()?;
        let generation = directly_patched_output_standby_path(state_dir);
        let standby_state_path = directly_patched_output_standby_state_path(state_dir);
        let reused_standby =
            read_directly_patched_output_standby_ranges(output, &generation, &standby_state_path)
                .is_some_and(|ranges| {
                    verbose_timing_phase!(
                        "Catch up directly patched output generation",
                        range_count = ranges.len(),
                        byte_count = ranges.iter().map(std::ops::Range::len).sum::<usize>()
                    );
                    copy_output_ranges(output, &generation, &ranges).is_ok()
                });
        if !reused_standby {
            let _ = std::fs::remove_file(&generation);
            verbose_timing_phase!("Clone directly patched output generation");
            if !clone_snapshot_bytes(output, &generation) {
                return None;
            }
        }
        if !FileIdentity::from_path(output)
            .ok()
            .flatten()
            .as_ref()
            .is_some_and(|identity| {
                identity.matches_same_data_ignoring_change_time(&published_identity)
            })
        {
            let _ = std::fs::remove_file(&generation);
            return None;
        }
        Some(Self {
            path: generation,
            published_path: Some(output.to_path_buf()),
            published_identity: Some(published_identity),
            standby_state_path: Some(standby_state_path),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn is_generation(&self) -> bool {
        self.published_path.is_some()
    }

    fn should_invalidate_code_signature_cache(&self) -> bool {
        !self.is_generation()
    }

    fn install(&mut self, ranges: &[std::ops::Range<usize>]) -> Result {
        let Some(published_path) = self.published_path.as_ref() else {
            return Ok(());
        };
        verbose_timing_phase!("Install directly patched output generation");
        install_directly_patched_output_generation(&self.path, published_path)?;
        let standby_is_predecessor = self
            .published_identity
            .as_ref()
            .zip(FileIdentity::from_path(&self.path).ok().flatten().as_ref())
            .is_some_and(|(expected, actual)| {
                expected.matches_same_data_ignoring_change_time(actual)
            });
        let standby_state_installed = standby_is_predecessor
            && self
                .standby_state_path
                .as_ref()
                .is_some_and(|standby_state_path| {
                    write_directly_patched_output_standby_state(
                        standby_state_path,
                        published_path,
                        &self.path,
                        ranges,
                    )
                    .is_ok()
                });
        if !standby_state_installed {
            let _ = std::fs::remove_file(&self.path);
        }
        if !standby_state_installed
            && let Some(standby_state_path) = self.standby_state_path.as_ref()
        {
            let _ = std::fs::remove_file(standby_state_path);
        }
        self.published_path = None;
        Ok(())
    }
}

impl Drop for DirectlyPatchedOutput {
    fn drop(&mut self) {
        if self.published_path.is_some() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn directly_patched_output_standby_path(state_dir: &Path) -> PathBuf {
    state_dir.join(DIRECT_PATCH_STANDBY_FILE)
}

fn directly_patched_output_standby_state_path(state_dir: &Path) -> PathBuf {
    state_dir.join(DIRECT_PATCH_STANDBY_STATE_FILE)
}

fn invalidate_directly_patched_output_standby(state_dir: &Path) -> Result {
    for path in [
        directly_patched_output_standby_state_path(state_dir),
        directly_patched_output_standby_path(state_dir),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to remove directly patched output standby `{}`",
                        path.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn read_directly_patched_output_standby_ranges(
    output: &Path,
    standby: &Path,
    state_path: &Path,
) -> Option<Vec<std::ops::Range<usize>>> {
    if std::fs::metadata(state_path).ok()?.len() > DIRECT_PATCH_STANDBY_STATE_MAX_LEN {
        return None;
    }
    let contents = std::fs::read_to_string(state_path).ok()?;
    let mut lines = contents.lines();
    if lines.next()? != DIRECT_PATCH_STANDBY_STATE_VERSION {
        return None;
    }
    let expected_output = FileIdentity::parse(lines.next()?).ok().flatten()?;
    let expected_standby = FileIdentity::parse(lines.next()?).ok().flatten()?;
    let output_identity = FileIdentity::from_path(output).ok().flatten()?;
    let standby_identity = FileIdentity::from_path(standby).ok().flatten()?;
    if expected_output != output_identity || expected_standby != standby_identity {
        return None;
    }

    let range_count = lines.next()?.parse().ok()?;
    if range_count > DIRECT_PATCH_STANDBY_MAX_RANGES {
        return None;
    }
    let mut ranges = Vec::with_capacity(range_count);
    for line in lines.by_ref().take(range_count) {
        let (start, end) = line.split_once(':')?;
        let start = start.parse().ok()?;
        let end = end.parse().ok()?;
        if start > end || end > output_identity.len.try_into().ok()? {
            return None;
        }
        ranges.push(start..end);
    }
    if ranges.len() != range_count || lines.next().is_some() {
        return None;
    }
    Some(merged_output_ranges(&ranges))
}

fn write_directly_patched_output_standby_state(
    state_path: &Path,
    output: &Path,
    standby: &Path,
    ranges: &[std::ops::Range<usize>],
) -> Result {
    let ranges = merged_output_ranges(ranges);
    if ranges.len() > DIRECT_PATCH_STANDBY_MAX_RANGES {
        let _ = std::fs::remove_file(state_path);
        let _ = std::fs::remove_file(standby);
        return Ok(());
    }
    let output_identity = FileIdentity::from_path(output)?
        .context("Missing directly patched output generation identity")?;
    let standby_identity = FileIdentity::from_path(standby)?
        .context("Missing directly patched output standby identity")?;
    let mut contents = format!(
        "{DIRECT_PATCH_STANDBY_STATE_VERSION}\n{}\n{}\n{}\n",
        output_identity.render(),
        standby_identity.render(),
        ranges.len(),
    );
    for range in ranges {
        writeln!(&mut contents, "{}:{}", range.start, range.end)?;
    }
    let tmp_path = append_suffix(state_path, ".tmp");
    std::fs::write(&tmp_path, contents).with_context(|| {
        format!(
            "Failed to write directly patched output standby state `{}`",
            tmp_path.display()
        )
    })?;
    std::fs::rename(&tmp_path, state_path).with_context(|| {
        format!(
            "Failed to install directly patched output standby state `{}`",
            state_path.display()
        )
    })
}

fn copy_output_ranges(
    source_path: &Path,
    target_path: &Path,
    ranges: &[std::ops::Range<usize>],
) -> Result {
    let mut source = std::fs::File::open(source_path).with_context(|| {
        format!(
            "Failed to open directly patched output source `{}`",
            source_path.display()
        )
    })?;
    let mut target = OpenOptions::new()
        .write(true)
        .open(target_path)
        .with_context(|| {
            format!(
                "Failed to open directly patched output standby `{}`",
                target_path.display()
            )
        })?;
    let mut buffer = [0; 64 * 1024];
    for range in merged_output_ranges(ranges) {
        source.seek(SeekFrom::Start(range.start as u64))?;
        target.seek(SeekFrom::Start(range.start as u64))?;
        let mut remaining = range.len();
        while remaining > 0 {
            let len = remaining.min(buffer.len());
            source.read_exact(&mut buffer[..len])?;
            target.write_all(&buffer[..len])?;
            remaining -= len;
        }
    }
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn install_directly_patched_output_generation(generation: &Path, output: &Path) -> Result {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let generation_cstring = CString::new(generation.as_os_str().as_bytes())
        .context("Directly patched output generation path contains a NUL byte")?;
    let output_cstring = CString::new(output.as_os_str().as_bytes())
        .context("Directly patched output path contains a NUL byte")?;
    // SAFETY: Both paths are valid NUL-terminated strings for the duration of the call.
    if unsafe {
        libc::renamex_np(
            generation_cstring.as_ptr(),
            output_cstring.as_ptr(),
            libc::RENAME_SWAP,
        )
    } == 0
    {
        return Ok(());
    }
    // Some Apple filesystems may support clones without swap publication. Retain the previous
    // atomic-replacement behavior there; the absent standby disables reuse on the next link.
    std::fs::rename(generation, output)
        .context("Failed to install directly patched output generation")
}

#[cfg(not(target_vendor = "apple"))]
fn install_directly_patched_output_generation(generation: &Path, output: &Path) -> Result {
    std::fs::rename(generation, output).with_context(|| {
        format!(
            "Failed to install directly patched output generation `{}` as `{}`",
            generation.display(),
            output.display()
        )
    })
}

fn flush_output_ranges(
    output: &memmap2::MmapMut,
    ranges: &[std::ops::Range<usize>],
    output_path: &Path,
) -> Result {
    for range in merged_output_ranges(ranges) {
        output
            .flush_range(range.start, range.end - range.start)
            .with_context(|| {
                format!(
                    "Failed to flush incrementally patched output `{}`",
                    output_path.display()
                )
            })?;
    }
    Ok(())
}

fn write_output_ranges(
    output: &[u8],
    ranges: &[std::ops::Range<usize>],
    file: &mut std::fs::File,
    output_path: &Path,
) -> Result {
    let ranges = merged_output_ranges(ranges);
    verbose_timing_phase!(
        "Write output generation ranges",
        range_count = ranges.len(),
        byte_count = ranges.iter().map(std::ops::Range::len).sum::<usize>()
    );
    for range in ranges {
        verbose_timing_phase!(
            "Write output generation range",
            start = range.start,
            byte_count = range.len()
        );
        let Some(output_range) = output.get(range.clone()) else {
            return Err(crate::error!(
                "Incrementally patched output range {range:?} is out of bounds for `{}`",
                output_path.display()
            ));
        };
        file.seek(SeekFrom::Start(range.start as u64))
            .with_context(|| {
                format!(
                    "Failed to seek incrementally patched output `{}`",
                    output_path.display()
                )
            })?;
        file.write_all(output_range).with_context(|| {
            format!(
                "Failed to write incrementally patched output `{}`",
                output_path.display()
            )
        })?;
    }
    Ok(())
}

fn merged_output_ranges(ranges: &[std::ops::Range<usize>]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = ranges
        .iter()
        .filter(|range| !range.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    let mut merged = Vec::<std::ops::Range<usize>>::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    merged
}

#[derive(Clone)]
struct PatchSection {
    input: String,
    section_index: u32,
    section_name: Option<String>,
    input_size: u64,
    output_offset: u64,
    output_size: u64,
    data_hash: Option<String>,
    cstring_nul_boundaries_hash: Option<String>,
}

#[derive(Clone)]
struct MatchedPatchSection {
    previous: PatchSection,
    current: PatchSection,
}

struct MatchedPatchSections {
    sections: Vec<MatchedPatchSection>,
    changed_sections: Vec<PatchSection>,
}

enum NormalizedRustArchivePatchState {
    Unchanged(FilePatchState),
    MatchedButNotUnchanged(MatchedPatchSections),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SectionReference {
    source_section_name: String,
    relocation_offset: u64,
    relocation_kind: String,
    relocation_encoding: String,
    relocation_size: u8,
    relocation_addend: i64,
}

#[derive(Clone, Copy)]
struct PatchInputBytes<'data> {
    bytes: &'data [u8],
    file_offset: usize,
}

struct ParsedPatchInputRef {
    identifier: Vec<u8>,
    range: std::ops::Range<usize>,
}

#[derive(Clone, Copy)]
enum PatchInputLookup {
    MatchArchiveMember,
    CurrentRecordedRange,
}

#[derive(Clone, Copy)]
enum ArchiveMemberMatch<'data> {
    Unique(PatchInputBytes<'data>),
    Ambiguous,
    Unavailable,
}

struct PatchInputResolver<'data> {
    bytes: &'data [u8],
    archive_members: Option<HashMap<Vec<u8>, ArchiveMemberMatch<'data>>>,
    normalize_rust_archive_patch_inputs: bool,
}

impl<'data> PatchInputResolver<'data> {
    fn new(bytes: &'data [u8], normalize_rust_archive_patch_inputs: bool) -> Result<Self> {
        let Ok(archive) = ArchiveIterator::from_archive_bytes(bytes) else {
            return Ok(Self {
                bytes,
                archive_members: None,
                normalize_rust_archive_patch_inputs,
            });
        };
        let mut archive_members = HashMap::new();
        for entry in archive {
            let ArchiveEntry::Regular(content) = entry? else {
                continue;
            };
            let identifier = if normalize_rust_archive_patch_inputs {
                archive_member_patch_identifier(content.ident.as_slice())
            } else {
                content.ident.as_slice().to_vec()
            };
            let member = PatchInputBytes {
                bytes: content.entry_data,
                file_offset: content.data_offset,
            };
            archive_members
                .entry(identifier)
                .and_modify(|existing| *existing = ArchiveMemberMatch::Ambiguous)
                .or_insert(ArchiveMemberMatch::Unique(member));
        }
        Ok(Self {
            bytes,
            archive_members: Some(archive_members),
            normalize_rust_archive_patch_inputs,
        })
    }

    fn resolve(
        &self,
        input_file_path: &str,
        input_ref: &str,
        lookup: PatchInputLookup,
    ) -> Result<Option<PatchInputBytes<'data>>> {
        let Some(parsed) = parse_patch_input_ref(input_file_path, input_ref)? else {
            return Ok(Some(PatchInputBytes {
                bytes: self.bytes,
                file_offset: 0,
            }));
        };
        if parsed.range.is_empty() {
            return Ok(None);
        }

        if matches!(lookup, PatchInputLookup::CurrentRecordedRange) {
            let Some(input_bytes) = self.bytes.get(parsed.range.clone()) else {
                return Ok(None);
            };
            return Ok(Some(PatchInputBytes {
                bytes: input_bytes,
                file_offset: parsed.range.start,
            }));
        }

        let archive_member = if self.normalize_rust_archive_patch_inputs {
            self.archive_members.as_ref().and_then(|members| {
                members.get(&archive_member_patch_identifier(&parsed.identifier))
            })
        } else {
            self.archive_members
                .as_ref()
                .and_then(|members| members.get(&parsed.identifier))
        };
        if !parsed.identifier.is_empty()
            && let Some(member) = archive_member
        {
            match member {
                ArchiveMemberMatch::Unique(member) => return Ok(Some(*member)),
                ArchiveMemberMatch::Ambiguous => return Ok(None),
                ArchiveMemberMatch::Unavailable => {}
            }
        }

        let Some(input_bytes) = self.bytes.get(parsed.range.clone()) else {
            return Ok(None);
        };
        Ok(Some(PatchInputBytes {
            bytes: input_bytes,
            file_offset: parsed.range.start,
        }))
    }
}

impl MatchedPatchSection {
    fn same(section: PatchSection) -> Self {
        Self {
            previous: section.clone(),
            current: section,
        }
    }
}

fn update_section_records_for_matched_patches(
    input_file: &str,
    matched_sections: &[MatchedPatchSection],
    records: &mut [SectionRecord],
) -> bool {
    if matched_sections.len() == 1 {
        return update_section_record_for_matched_patch(input_file, &matched_sections[0], records);
    }

    let updates = matched_sections
        .iter()
        .map(|matched| {
            (
                section_record_update_key(input_file, &matched.previous),
                &matched.current,
            )
        })
        .collect::<HashMap<_, _>>();

    let mut changed = false;
    for record in records {
        let Some(current) = updates.get(&(
            record.input_file.as_str(),
            record.input.as_str(),
            record.section_index,
            record.output_offset,
            record.size,
        )) else {
            continue;
        };

        if update_section_record(record, current) {
            changed = true;
        }
    }
    changed
}

fn update_section_record_for_matched_patch(
    input_file: &str,
    matched: &MatchedPatchSection,
    records: &mut [SectionRecord],
) -> bool {
    let Some(record) = records.iter_mut().find(|record| {
        record.input_file == input_file
            && record.input == matched.previous.input
            && record.section_index == matched.previous.section_index
            && record.output_offset == matched.previous.output_offset
            && record.size == matched.previous.output_size
    }) else {
        return false;
    };

    update_section_record(record, &matched.current)
}

fn section_record_update_key<'a>(
    input_file: &'a str,
    section: &'a PatchSection,
) -> (&'a str, &'a str, u32, u64, u64) {
    (
        input_file,
        section.input.as_str(),
        section.section_index,
        section.output_offset,
        section.output_size,
    )
}

fn update_section_record(record: &mut SectionRecord, current: &PatchSection) -> bool {
    if record.input == current.input
        && record.section_index == current.section_index
        && record.output_offset == current.output_offset
        && record.size == current.output_size
    {
        return false;
    }

    record.input = current.input.clone().into();
    record.section_index = current.section_index;
    record.output_offset = current.output_offset;
    record.size = current.output_size;
    true
}

fn update_matched_patch_current_sections(
    matched_sections: &mut [MatchedPatchSection],
    current_sections: &[PatchSection],
) {
    for (matched, current) in matched_sections.iter_mut().zip(current_sections) {
        matched.current = current.clone();
    }
}

impl<'data> PreparedState<'data> {
    pub(crate) fn compute_fast_build_id_and_prepare_state(
        &self,
        output: &[u8],
    ) -> Result<Option<blake3::Hash>> {
        if self.mode == IncrementalMode::Disabled {
            return Ok(None);
        }
        let Some(range) = build_id_note_range(output)? else {
            return Ok(None);
        };
        validate_fast_build_id_range(&range)?;
        let Some(tree) = build_id_hash_tree(output, &range) else {
            return Ok(None);
        };
        let state = BuildIdHashState {
            output_len: output.len() as u64,
            nodes: tree.len(),
            tree_hash: Some(build_id_hash_tree_hash(&tree)),
        };
        let build_id = build_id_from_hash_tree(&state, &tree)?;
        *self.prepared_fast_build_id_state.lock().unwrap() = Some((Some(state), Some(tree)));
        Ok(Some(build_id))
    }

    pub(crate) fn begin_update(&self) -> Result<Option<IncrementalStateLock>> {
        if self.mode == IncrementalMode::Disabled {
            return Ok(None);
        }
        let lock = acquire_incremental_state_lock(&self.current.state_dir)?;
        if matches!(self.mode, IncrementalMode::Relink { .. }) {
            invalidate_directly_patched_output_standby(&self.current.state_dir)?;
        }
        if !self.can_publish_in_background() {
            remove_incremental_index(&self.current.state_dir)?;
        }
        mark_incremental_update_started(&self.current.state_dir, "link output")?;
        Ok(Some(lock))
    }

    pub(crate) fn can_reuse_output(&self) -> bool {
        self.mode == IncrementalMode::Reuse
    }

    pub(crate) fn can_reuse_unchanged_sections(&self) -> bool {
        matches!(
            self.mode,
            IncrementalMode::Relink {
                can_reuse_unchanged_sections: true,
                ..
            }
        )
    }

    pub(crate) fn can_publish_in_background(&self) -> bool {
        !matches!(
            &self.mode,
            IncrementalMode::Relink { reason, .. }
                if reason == "previous incremental update did not complete"
                    || reason.starts_with("previous incremental update status could not be checked:")
        )
    }

    fn intern_input_texts(&self, input: InputRef<'_>) -> (SharedText, SharedText) {
        self.record_texts.intern_input(input)
    }

    pub(crate) fn records_relocations(&self) -> bool {
        self.mode != IncrementalMode::Disabled
    }

    pub(crate) fn try_reuse_section(
        &self,
        input: InputRef<'_>,
        section_index: object::SectionIndex,
        output_offset: u64,
        size: u64,
        record_for_reuse: bool,
        allow_reuse: bool,
    ) -> bool {
        if self.mode == IncrementalMode::Disabled {
            return false;
        }
        if !record_for_reuse {
            return false;
        }

        let (input_file, input_text) = self.intern_input_texts(input);
        let record = SectionRecord::new_with_texts(
            input_file,
            input_text,
            section_index,
            output_offset,
            size,
        );
        self.current_sections.push(record.clone());

        if !allow_reuse {
            return false;
        }
        if !self.can_reuse_unchanged_sections() {
            return false;
        }
        if !self.reusable_inputs.contains(record.input_file.as_str()) {
            return false;
        }
        if !self.previous_sections.contains(&record) {
            return false;
        }

        self.reused_sections.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub(crate) fn record_generated_section(&self, name: &str, output_offset: u64, size: u64) {
        if self.mode == IncrementalMode::Disabled || size == 0 {
            return;
        }
        self.current_sections
            .push(generated_section_record(name, output_offset, size));
    }

    pub(crate) fn record_eh_frame_fde(
        &self,
        input: InputRef<'_>,
        section_index: object::SectionIndex,
        eh_frame_section_index: object::SectionIndex,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) {
        if self.mode == IncrementalMode::Disabled || size == 0 {
            return;
        }
        let (input_file, input_text) = self.intern_input_texts(input);
        self.current_fdes.push(FdeRecord::new_with_texts(
            input_file,
            input_text,
            section_index,
            eh_frame_section_index,
            input_offset,
            output_offset,
            size,
        ));
    }

    pub(crate) fn deferred_relocation_record(
        input: InputRef<'data>,
        section_index: object::SectionIndex,
        target_symbol_id: u32,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        kind: u32,
        addend: i64,
        written_value: u64,
        target_value: u64,
        target_metadata: impl FnOnce() -> Result<(
            Option<&'data [u8]>,
            Option<(InputRef<'data>, object::SectionIndex, u64)>,
        )>,
    ) -> Result<Option<DeferredRelocationRecord<'data>>> {
        Self::deferred_relocation_record_with_applied_target(
            input,
            section_index,
            target_symbol_id,
            relocation_offset,
            output_offset,
            size,
            kind,
            addend,
            written_value,
            target_value,
            None,
            target_metadata,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn deferred_relocation_record_with_applied_target(
        input: InputRef<'data>,
        section_index: object::SectionIndex,
        target_symbol_id: u32,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        kind: u32,
        addend: i64,
        written_value: u64,
        target_value: u64,
        applied_target_value: Option<u64>,
        target_metadata: impl FnOnce() -> Result<(
            Option<&'data [u8]>,
            Option<(InputRef<'data>, object::SectionIndex, u64)>,
        )>,
    ) -> Result<Option<DeferredRelocationRecord<'data>>> {
        if size == 0 {
            return Ok(None);
        }
        let (target_name, target) = target_metadata()?;
        let target = DeferredRecordedRelocationTarget {
            target_name,
            target,
        };
        Ok(Some(DeferredRelocationRecord {
            target_symbol_id,
            written_value,
            target_value,
            applied_target_value,
            target,
            input,
            section_index,
            relocation_offset,
            output_offset,
            size,
            kind,
            addend,
        }))
    }

    pub(crate) fn record_relocations(&self, records: Vec<DeferredRelocationRecord<'data>>) {
        self.current_relocations.extend(records);
    }

    pub(crate) fn record_macho_symbol_resolutions(
        &self,
        resolutions: Vec<DeferredMachOSymbolResolution<'data>>,
    ) {
        if self.mode == IncrementalMode::Disabled {
            return;
        }
        let records = resolutions
            .into_iter()
            .filter(|resolution| !resolution.name.is_empty())
            .map(|resolution| {
                let target = resolution
                    .target
                    .map(|(input, section_index, section_offset)| {
                        let (input_file, input) = self.intern_input_texts(input);
                        RelocationTargetRecord::new(
                            input_file,
                            input,
                            section_index,
                            section_offset,
                        )
                    });
                MachOSymbolResolutionRecord {
                    name: self.record_texts.intern(hex::encode(resolution.name)),
                    direct_value: resolution.direct_value,
                    got_address: resolution.got_address,
                    stub_address: resolution.stub_address,
                    thunk_addresses: resolution.thunk_addresses,
                    target,
                }
            })
            .collect();
        self.current_macho_symbol_resolutions.extend(records);
    }

    pub(crate) fn record_dynamic_relocation_with_output_info(
        &self,
        input: InputRef<'_>,
        section_index: object::SectionIndex,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        output_info: Option<(u64, u64)>,
    ) {
        if self.mode == IncrementalMode::Disabled || size == 0 {
            return;
        }
        let (input_file, input_text) = self.intern_input_texts(input);
        self.current_dynamic_relocations
            .push(DynamicRelocationRecord::new_with_texts(
                input_file,
                input_text,
                section_index,
                relocation_offset,
                output_offset,
                size,
                output_info,
            ));
    }

    pub(crate) fn finish(
        &self,
        args: &impl platform::Args,
        file_loader: &FileLoader<'data>,
        lock: Option<IncrementalStateLock>,
    ) -> Result {
        timing_phase!("Write incremental state");
        if let Some(pending) = self.prepare_finish(args, file_loader, lock, false)? {
            pending.publish()?;
        }
        Ok(())
    }

    pub(crate) fn prepare_finish(
        &self,
        args: &impl platform::Args,
        file_loader: &FileLoader<'data>,
        lock: Option<IncrementalStateLock>,
        defer_input_hashing: bool,
    ) -> Result<Option<PendingStateWrite<'data>>> {
        if self.mode == IncrementalMode::Disabled {
            return Ok(None);
        }

        if let Some(previous_output) = self.retained_output_to_restore.as_ref()
            && !restore_missing_output_snapshot(
                &self.current.state_dir,
                previous_output,
                args.output(),
            )?
        {
            return Err(crate::error!(
                "Retained incremental output snapshot could not be restored"
            ));
        }

        let output = if args.should_retain_output_snapshot() {
            FileContentState::from_path(args.output())
        } else {
            FileContentState::from_path_identity_only(args.output())
        }
        .with_context(|| {
            format!(
                "Failed to record output file `{}` for incremental state",
                args.output().display()
            )
        })?;
        if args.should_retain_output_snapshot() {
            install_output_snapshot(&self.current.state_dir, args.output())?;
        }
        let output_path = args.output().to_owned();
        let mut output_bytes = LazyOutputBytes::new(|| read_output_bytes(&output_path));
        let (build_id_hashes, build_id_tree) = {
            timing_phase!("Compute incremental build ID state");
            if args.has_incremental_fast_build_id() {
                if let Some(prepared) = self.prepared_fast_build_id_state.lock().unwrap().take() {
                    prepared
                } else {
                    build_id_hash_state_from_output(output_bytes.get()?)?
                }
            } else {
                (None, None)
            }
        };

        let (
            sections,
            relocations,
            relocation_shards,
            macho_symbol_resolutions,
            macho_symbol_resolutions_file,
            fdes,
            dynamic_relocations,
        ) = {
            timing_phase!("Collect incremental records");

            let mut sections = self.current_sections.take_all();
            if sections.is_empty() && self.mode == IncrementalMode::Reuse {
                sections.extend(self.previous_sections.iter().cloned());
            }

            let relocation_shards = self.current_relocations.take_shards();
            let relocations = if relocation_shards.iter().all(Vec::is_empty)
                && self.mode == IncrementalMode::Reuse
            {
                self.previous_relocations.clone()
            } else {
                Vec::new()
            };

            let mut macho_symbol_resolutions = self.current_macho_symbol_resolutions.take_all();
            macho_symbol_resolutions.sort_unstable();
            macho_symbol_resolutions.dedup();
            let macho_symbol_resolutions_file =
                if macho_symbol_resolutions.is_empty() && self.mode == IncrementalMode::Reuse {
                    self.previous_macho_symbol_resolutions_file.clone()
                } else {
                    None
                };

            let mut fdes = self.current_fdes.take_all();
            if fdes.is_empty() && self.mode == IncrementalMode::Reuse {
                fdes.extend(self.previous_fdes.iter().cloned());
            }

            let mut dynamic_relocations = self.current_dynamic_relocations.take_all();
            if dynamic_relocations.is_empty() && self.mode == IncrementalMode::Reuse {
                dynamic_relocations.extend(self.previous_dynamic_relocations.iter().cloned());
            }

            (
                sections,
                relocations,
                relocation_shards,
                macho_symbol_resolutions,
                macho_symbol_resolutions_file,
                fdes,
                dynamic_relocations,
            )
        };

        let mut input_files = self.current.input_files.clone();
        let deferred_hash_inputs = if defer_input_hashing {
            // Install snapshots while they still name the inputs that produced this output.
            // Hashing uses the retained mapped bytes and can be deferred into publication.
            timing_phase!("Snapshot incremental inputs");
            snapshot_loaded_input_files(
                &self.current.state_dir,
                &file_loader.loaded_files,
                &mut input_files,
                &sections,
                false,
            )?;
            timing_phase!("Refresh incremental input identities");
            refresh_input_file_identities(&mut input_files);
            Some(file_loader.loaded_files.clone())
        } else {
            {
                timing_phase!("Snapshot incremental inputs");
                snapshot_loaded_files(
                    &self.current.state_dir,
                    file_loader,
                    &mut input_files,
                    &sections,
                )?;
            }
            {
                timing_phase!("Refresh incremental input identities");
                refresh_input_file_identities(&mut input_files);
            }
            None
        };

        let state = PersistedState {
            args_hash: self.current.args_hash.clone(),
            link_options_hash: Some(self.current.link_options_hash.clone()),
            input_order_hash: Some(self.current.input_order_hash.clone()),
            sld_version: Some(self.current.sld_version.clone()),
            link_start: self.current.link_start.clone(),
            output,
            build_id_hashes,
            input_files,
            sections,
            relocations,
            macho_symbol_resolutions,
            fdes,
            dynamic_relocations,
            sections_file: None,
            macho_symbol_resolutions_file,
            patch_records_file: None,
            patch_record_locations: Vec::new(),
            raw_patch_record_locations: None,
        };

        Ok(Some(PendingStateWrite {
            state_dir: self.current.state_dir.clone(),
            state,
            relocation_shards,
            build_id_tree,
            deferred_hash_inputs,
            reused_sections: self.reused_sections.load(Ordering::Relaxed),
            _lock: lock,
        }))
    }
}

impl PendingStateWrite<'_> {
    pub(crate) fn publish_reuse_metadata_in_background(&mut self) {
        if let Some(loaded_files) = self.deferred_hash_inputs.as_ref() {
            hash_pending_reuse_input_files(
                loaded_files,
                &mut self.state.input_files,
                self.state.link_start.as_ref(),
            );
        }
        if let Err(error) = self.state.write_publishing_index(&self.state_dir) {
            let _ = append_log(
                &self.state_dir,
                &format!("background incremental reuse metadata publication failed: {error:?}"),
            );
        }
    }

    pub(crate) fn publish(mut self) -> Result {
        timing_phase!("Persist prepared incremental state");
        if let Some(loaded_files) = self.deferred_hash_inputs.take() {
            timing_phase!("Hash incremental inputs");
            hash_loaded_input_files(&loaded_files, &mut self.state.input_files);
        }
        let total_relocations = self.relocation_shards.iter().map(Vec::len).sum::<usize>();
        self.state.relocations.reserve(total_relocations);
        let record_texts = RecordTextInterner::default();
        let mut relocation_shards = std::mem::take(&mut self.relocation_shards)
            .into_par_iter()
            .map(|shard| {
                shard
                    .into_iter()
                    .map(|record| record.materialize(&record_texts))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        for shard in &mut relocation_shards {
            self.state.relocations.append(shard);
        }
        // Release publication-only allocations before advertising the state as ready. Otherwise
        // an immediate incremental reuse contends with the background publisher tearing them down.
        drop(relocation_shards);
        drop(record_texts);
        {
            timing_phase!("Persist incremental build ID state");
            write_build_id_hash_tree(&self.state_dir, self.build_id_tree.as_deref())?;
        }
        {
            timing_phase!("Persist incremental index and sections");
            self.state.write(&self.state_dir)?;
        }
        clear_incremental_update_marker(&self.state_dir)?;
        if self.reused_sections > 0 {
            append_log(
                &self.state_dir,
                &format!("reused {} unchanged input sections", self.reused_sections),
            )?;
        }
        Ok(())
    }

    pub(crate) fn publish_in_background(self) {
        let state_dir = self.state_dir.clone();
        if let Err(error) = self.publish() {
            let _ = append_log(
                &state_dir,
                &format!("background incremental state publication failed: {error:?}"),
            );
        }
    }
}

#[cfg(test)]
fn classify_incremental_mode(
    output: &Path,
    current: &CurrentState,
    previous: &PersistedState,
) -> IncrementalMode {
    classify_incremental_mode_with_output_policy(output, current, previous, false, false)
}

fn classify_incremental_mode_with_output_policy(
    output: &Path,
    current: &CurrentState,
    previous: &PersistedState,
    trust_persistent_output_data_identity: bool,
    retained_output_snapshot_matches: bool,
) -> IncrementalMode {
    if let Some(reason) = interrupted_update_relink_reason(&current.state_dir) {
        return IncrementalMode::Relink {
            reason,
            can_reuse_unchanged_sections: false,
        };
    }

    if let Some(reason) =
        sld_version_relink_reason(previous.sld_version.as_deref(), &current.sld_version)
    {
        return IncrementalMode::Relink {
            reason: reason.to_owned(),
            can_reuse_unchanged_sections: false,
        };
    }

    let previous_link_options_hash = previous
        .link_options_hash
        .as_deref()
        .unwrap_or(&previous.args_hash);
    if current.link_options_hash != previous_link_options_hash {
        return IncrementalMode::Relink {
            reason: "linker arguments changed".to_owned(),
            can_reuse_unchanged_sections: false,
        };
    }

    if !retained_output_snapshot_matches {
        match output_content_matches_previous(
            &previous.output,
            output,
            trust_persistent_output_data_identity,
        ) {
            Ok(true) => {}
            Ok(false) => {
                return IncrementalMode::Relink {
                    reason: "output file changed since previous link".to_owned(),
                    can_reuse_unchanged_sections: false,
                };
            }
            Err(error) => {
                return IncrementalMode::Relink {
                    reason: format!("output file could not be reused: {error:?}"),
                    can_reuse_unchanged_sections: false,
                };
            }
        }
    }

    if current.input_files != previous.input_files {
        return IncrementalMode::Relink {
            reason: describe_input_difference(&current.input_files, &previous.input_files),
            can_reuse_unchanged_sections: true,
        };
    }

    match previous.input_order_hash.as_deref() {
        Some(previous_order) if current.input_order_hash == previous_order => {}
        Some(_) => {
            return IncrementalMode::Relink {
                reason: "input file order changed".to_owned(),
                can_reuse_unchanged_sections: true,
            };
        }
        None => {
            return IncrementalMode::Relink {
                reason: "input file order missing from previous state".to_owned(),
                can_reuse_unchanged_sections: false,
            };
        }
    }

    IncrementalMode::Reuse
}

fn describe_input_difference(current: &[FileState], previous: &[FileState]) -> String {
    let previous_by_path = previous
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<HashMap<_, _>>();

    for file in current {
        match previous_by_path.get(file.path.as_str()) {
            None => return format!("input file added: {}", display_hex_path(&file.path)),
            Some(previous) if previous.content != file.content => {
                return format!("input file changed: {}", display_hex_path(&file.path));
            }
            Some(_) => {}
        }
    }

    let current_paths = current
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();
    for file in previous {
        if !current_paths.contains(file.path.as_str()) {
            return format!("input file removed: {}", display_hex_path(&file.path));
        }
    }

    "input file set changed".to_owned()
}

fn reusable_input_files(current: &[FileState], previous: &[FileState]) -> HashSet<String> {
    let previous_by_path = previous
        .iter()
        .map(|file| (file.path.as_str(), &file.content))
        .collect::<HashMap<_, _>>();

    current
        .iter()
        .filter(|file| previous_by_path.get(file.path.as_str()) == Some(&&file.content))
        .map(|file| file.path.clone())
        .collect()
}

impl CurrentState {
    fn new(
        args: &impl platform::Args,
        file_loader: &FileLoader<'_>,
        previous: Option<&PersistedState>,
    ) -> Self {
        Self {
            state_dir: state_dir_for_output(args.output()),
            args_hash: args_hash(args),
            link_options_hash: link_options_hash(args),
            input_order_hash: input_order_hash(file_loader),
            sld_version: sld_version(args),
            link_start: link_start_marker_identity(&state_dir_for_output(args.output())),
            input_files: fingerprint_loaded_files(file_loader, previous),
        }
    }

    fn log_mode(&self, mode: &IncrementalMode) -> Result {
        match mode {
            IncrementalMode::Disabled => Ok(()),
            IncrementalMode::Reuse => append_log(&self.state_dir, "reused existing output"),
            IncrementalMode::Relink { reason, .. } => {
                append_log(&self.state_dir, &format!("full relink: {reason}"))
            }
        }
    }
}

impl PersistedState {
    fn read(state_dir: &Path) -> Result<Option<Self>> {
        Self::read_impl(state_dir, true, PatchSectionReadMode::Parse, true)
    }

    fn read_metadata(state_dir: &Path) -> Result<Option<Self>> {
        Self::read_impl(state_dir, false, PatchSectionReadMode::PreserveRaw, false)
    }

    fn load_macho_symbol_resolutions(&mut self, state_dir: &Path) -> Result {
        if !self.macho_symbol_resolutions.is_empty() {
            return Ok(());
        }
        let Some(file_name) = self.macho_symbol_resolutions_file.as_deref() else {
            return Ok(());
        };
        timing_phase!("Read incremental Mach-O resolutions");
        self.macho_symbol_resolutions = read_macho_resolutions_sidecar(state_dir, file_name)?;
        Ok(())
    }

    fn read_impl(
        state_dir: &Path,
        load_sections: bool,
        patch_section_mode: PatchSectionReadMode,
        parse_patch_record_locations: bool,
    ) -> Result<Option<Self>> {
        let path = state_dir.join(INDEX_FILE);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let mut state = Self::parse_with_section_loader(
            &contents,
            patch_section_mode,
            parse_patch_record_locations,
            |sections_file| {
                if !load_sections {
                    return Ok(None);
                }
                read_sections_sidecar(state_dir, sections_file).map(Some)
            },
        )?;
        if load_sections
            && state.sections_file == state.patch_records_file
            && state.patch_records_file.is_some()
        {
            let records = state.read_all_indexed_records(state_dir)?;
            state.sections = records.sections;
            state.relocations = records.relocations;
            state.fdes = records.fdes;
            state.dynamic_relocations = records.dynamic_relocations;
        }
        state.apply_metadata_update(state_dir, patch_section_mode)?;
        Ok(Some(state))
    }

    fn read_records_for_input_files(
        &mut self,
        state_dir: &Path,
        input_files: &HashSet<String>,
    ) -> Result {
        self.materialize_patch_record_locations()?;
        if let Some(patch_records_file) = self.patch_records_file.as_deref() {
            let canonical_index = self.sections_file.as_deref() == Some(patch_records_file);
            if let Some(records) =
                self.read_indexed_patch_records(state_dir, patch_records_file, input_files)?
            {
                self.sections = records.sections;
                self.relocations = records.relocations;
                self.fdes = records.fdes;
                self.dynamic_relocations = records.dynamic_relocations;
                return Ok(());
            }
            if canonical_index {
                self.sections.clear();
                self.relocations.clear();
                self.fdes.clear();
                self.dynamic_relocations.clear();
                return Ok(());
            }
            if self.patch_record_locations.is_empty() {
                timing_phase!("Read incremental patch-record sidecar");
                let contents = read_sections_sidecar(state_dir, patch_records_file)?;
                let records =
                    parse_compact_records_block_for_input_files(contents.lines(), input_files)?;
                if input_files.iter().all(|input_file| {
                    records
                        .sections
                        .iter()
                        .any(|record| record.input_file.as_str() == input_file)
                }) {
                    self.sections = records.sections;
                    self.relocations = records.relocations;
                    self.fdes = records.fdes;
                    self.dynamic_relocations = records.dynamic_relocations;
                    return Ok(());
                }
            }
        }
        let Some(sections_file) = self.sections_file.as_deref() else {
            return Ok(());
        };
        timing_phase!("Read incremental sidecar records");
        let contents = read_sections_sidecar(state_dir, sections_file)?;
        let records = parse_compact_records_block_for_input_files(contents.lines(), input_files)?;
        self.sections = records.sections;
        self.relocations = records.relocations;
        self.fdes = records.fdes;
        self.dynamic_relocations = records.dynamic_relocations;
        Ok(())
    }

    fn materialize_patch_record_locations(&mut self) -> Result {
        let Some(raw) = self.raw_patch_record_locations.as_deref() else {
            return Ok(());
        };
        let mut lines = raw.lines();
        let (locations, deferred) = parse_patch_record_location_table(&mut lines, true)?;
        if deferred.is_some() || lines.next().is_some() {
            return Err(crate::error!(
                "Unexpected trailing incremental patch record location data"
            ));
        }
        self.patch_record_locations = locations;
        self.raw_patch_record_locations = None;
        Ok(())
    }

    fn read_indexed_patch_records(
        &self,
        state_dir: &Path,
        patch_records_file: &str,
        input_files: &HashSet<String>,
    ) -> Result<Option<CompactRecords>> {
        if self.patch_record_locations.is_empty() {
            return if self.sections_file.as_deref() == Some(patch_records_file) {
                Ok(Some(CompactRecords::default()))
            } else {
                Ok(None)
            };
        }
        let canonical_index = self.sections_file.as_deref() == Some(patch_records_file);
        let mut locations = self
            .patch_record_locations
            .iter()
            .filter(|location| input_files.contains(&location.input_file))
            .collect::<Vec<_>>();
        if !canonical_index
            && input_files.iter().any(|input_file| {
                !locations
                    .iter()
                    .any(|location| location.input_file == *input_file)
            })
        {
            return Ok(None);
        }
        locations.sort_by_key(|location| (location.offset, location.len, location.hash.as_str()));
        locations.dedup_by(|left, right| {
            left.offset == right.offset && left.len == right.len && left.hash == right.hash
        });
        let mut records =
            Self::read_indexed_records_at_locations(state_dir, patch_records_file, locations)?;
        records
            .sections
            .retain(|record| input_files.contains(record.input_file.as_str()));
        records.relocations.retain(|record| {
            input_files.contains(record.input_file.as_str())
                || record
                    .target
                    .as_ref()
                    .is_some_and(|target| input_files.contains(target.input_file.as_str()))
        });
        records
            .fdes
            .retain(|record| input_files.contains(record.input_file.as_str()));
        records
            .dynamic_relocations
            .retain(|record| input_files.contains(record.input_file.as_str()));
        Ok(Some(records))
    }

    fn read_all_indexed_records(&self, state_dir: &Path) -> Result<CompactRecords> {
        let Some(patch_records_file) = self.patch_records_file.as_deref() else {
            return Ok(CompactRecords::default());
        };
        let mut locations = self.patch_record_locations.iter().collect::<Vec<_>>();
        locations.sort_by_key(|location| (location.offset, location.len, location.hash.as_str()));
        locations.dedup_by(|left, right| {
            left.offset == right.offset && left.len == right.len && left.hash == right.hash
        });
        Self::read_indexed_records_at_locations(state_dir, patch_records_file, locations)
    }

    fn read_indexed_records_at_locations(
        state_dir: &Path,
        patch_records_file: &str,
        locations: Vec<&PatchRecordLocation>,
    ) -> Result<CompactRecords> {
        timing_phase!("Read indexed incremental patch records");
        validate_sections_file_name(patch_records_file)?;
        let path = state_dir.join(patch_records_file);
        let mut file = OpenOptions::new()
            .read(true)
            .open(&path)
            .with_context(|| format!("Failed to read incremental sections `{}`", path.display()))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("Failed to stat incremental sections `{}`", path.display()))?
            .len();
        let mut records = CompactRecords::default();
        for location in locations {
            let end = location
                .offset
                .checked_add(location.len)
                .context("Incremental patch record range overflowed")?;
            if end > file_len {
                return Err(crate::error!(
                    "Incremental patch record range is outside `{}`",
                    path.display()
                ));
            }
            file.seek(SeekFrom::Start(location.offset))
                .with_context(|| {
                    format!("Failed to seek incremental sections `{}`", path.display())
                })?;
            let len = usize::try_from(location.len)
                .context("Incremental patch record range is too large")?;
            let mut bytes = vec![0; len];
            file.read_exact(&mut bytes).with_context(|| {
                format!("Failed to read incremental sections `{}`", path.display())
            })?;
            if hash_bytes(&bytes) != location.hash {
                return Err(crate::error!(
                    "Incremental patch records `{}` do not match their content hash",
                    path.display()
                ));
            }
            let bytes = if patch_records_file.starts_with(COMPRESSED_SECTIONS_FILE_PREFIX) {
                zstd::stream::decode_all(bytes.as_slice()).with_context(|| {
                    format!(
                        "Failed to decompress incremental patch records `{}`",
                        path.display()
                    )
                })?
            } else {
                bytes
            };
            let contents = std::str::from_utf8(&bytes)
                .context("Invalid UTF-8 in incremental patch record sidecar")?;
            let block = parse_compact_records_block(contents.lines())?;
            records.sections.extend(block.sections);
            records.relocations.extend(block.relocations);
            records.fdes.extend(block.fdes);
            records
                .dynamic_relocations
                .extend(block.dynamic_relocations);
        }
        records.sections.sort();
        records.sections.dedup();
        records.relocations.sort();
        records.relocations.dedup();
        records.fdes.sort();
        records.fdes.dedup();
        records.dynamic_relocations.sort();
        records.dynamic_relocations.dedup();
        Ok(records)
    }

    fn read_patch_metadata_for_input_indices(
        &mut self,
        state_dir: &Path,
        input_indices: &HashSet<usize>,
    ) -> Result {
        if input_indices.is_empty() {
            return Ok(());
        }
        let mut remaining = input_indices.clone();
        self.read_patch_metadata_from_update(state_dir, &mut remaining)?;
        if !remaining.is_empty() {
            self.read_patch_metadata_from_index(state_dir, &mut remaining)?;
        }
        Ok(())
    }

    fn read_patch_metadata_from_update(
        &mut self,
        state_dir: &Path,
        input_indices: &mut HashSet<usize>,
    ) -> Result {
        let path = metadata_update_path(state_dir);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut lines = contents.lines();
        let version = lines
            .next()
            .context("Missing incremental metadata update header")?;
        if version != METADATA_UPDATE_VERSION && version != METADATA_UPDATE_VERSION_V1 {
            return Err(crate::error!(
                "Unsupported incremental metadata update version `{version}`"
            ));
        }
        let _ = parse_link_start_line(lines.next())?;
        let _ = parse_content_line(lines.next(), "output")?;
        let _ = parse_build_id_hash_line(lines.next())?;
        if version == METADATA_UPDATE_VERSION {
            let _ = parse_prefixed_line(lines.next(), "macho-resolutions-file")?;
        }
        let input_count: usize = parse_prefixed_line(lines.next(), "inputs")?
            .parse()
            .context("Invalid incremental metadata update input count")?;
        for _ in 0..input_count {
            let rest = parse_prefixed_line(lines.next(), "input")?;
            let (index, input_line) = rest
                .split_once('\t')
                .context("Malformed incremental metadata update input")?;
            let index: usize = index
                .parse()
                .context("Invalid incremental metadata update input index")?;
            if input_indices.remove(&index) {
                let Some(input) = self.input_files.get_mut(index) else {
                    return Err(crate::error!(
                        "Incremental metadata update input index out of bounds"
                    ));
                };
                input.patch = parse_input_line(
                    &format!("input\t{input_line}"),
                    PatchSectionReadMode::PreserveRaw,
                )?
                .patch;
            }
        }
        if lines.next().is_some() {
            return Err(crate::error!(
                "Unexpected trailing incremental metadata update data"
            ));
        }
        Ok(())
    }

    fn read_patch_metadata_from_index(
        &mut self,
        state_dir: &Path,
        input_indices: &mut HashSet<usize>,
    ) -> Result {
        let path = state_dir.join(INDEX_FILE);
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read incremental state `{}`", path.display()))?;
        let mut lines = contents.lines();
        for line in lines.by_ref() {
            if line.starts_with("inputs\t") {
                let input_count: usize = parse_prefixed_line(Some(line), "inputs")?
                    .parse()
                    .context("Invalid incremental input count")?;
                for index in 0..input_count {
                    let line = lines.next().context("Missing incremental input record")?;
                    if input_indices.remove(&index) {
                        let Some(input) = self.input_files.get_mut(index) else {
                            return Err(crate::error!("Incremental input index out of bounds"));
                        };
                        input.patch =
                            parse_input_line(line, PatchSectionReadMode::PreserveRaw)?.patch;
                    }
                    if input_indices.is_empty() {
                        return Ok(());
                    }
                }
                return Ok(());
            }
        }
        Err(crate::error!("Missing incremental input count"))
    }

    #[cfg(test)]
    fn parse(contents: &str) -> Result<Self> {
        Self::parse_with_section_loader(contents, PatchSectionReadMode::Parse, true, |_| Ok(None))
    }

    fn parse_with_section_loader(
        contents: &str,
        patch_section_mode: PatchSectionReadMode,
        parse_patch_record_locations: bool,
        mut load_sections: impl FnMut(&str) -> Result<Option<String>>,
    ) -> Result<Self> {
        let mut lines = contents.lines().peekable();
        let version = lines.next().context("Missing incremental state header")?;
        if version != STATE_VERSION
            && version != STATE_VERSION_V37
            && version != STATE_VERSION_V36
            && version != STATE_VERSION_V35
            && version != STATE_VERSION_V34
            && version != STATE_VERSION_V33
            && version != STATE_VERSION_V32
            && version != STATE_VERSION_V31
            && version != STATE_VERSION_V30
            && version != STATE_VERSION_V29
            && version != STATE_VERSION_V28
            && version != STATE_VERSION_V27
            && version != STATE_VERSION_V26
            && version != STATE_VERSION_V25
            && version != STATE_VERSION_V24
            && version != STATE_VERSION_V23
            && version != STATE_VERSION_V22
            && version != STATE_VERSION_V21
            && version != STATE_VERSION_V20
            && version != STATE_VERSION_V19
            && version != STATE_VERSION_V18
            && version != STATE_VERSION_V17
            && version != STATE_VERSION_V16
            && version != STATE_VERSION_V15
            && version != STATE_VERSION_V14
            && version != STATE_VERSION_V13
            && version != STATE_VERSION_V12
            && version != STATE_VERSION_V11
            && version != STATE_VERSION_V10
            && version != STATE_VERSION_V9
            && version != STATE_VERSION_V8
            && version != STATE_VERSION_V7
            && version != STATE_VERSION_V6
            && version != STATE_VERSION_V5
            && version != STATE_VERSION_V4
            && version != STATE_VERSION_V3
            && version != STATE_VERSION_V2
            && version != STATE_VERSION_V1
        {
            return Err(crate::error!(
                "Unsupported incremental state version `{version}`"
            ));
        }

        let args_hash = parse_prefixed_line(lines.next(), "args")?.to_owned();
        let link_options_hash = if lines
            .peek()
            .is_some_and(|line| line.starts_with("link-options\t"))
        {
            Some(parse_prefixed_line(lines.next(), "link-options")?.to_owned())
        } else {
            None
        };
        if version == STATE_VERSION && link_options_hash.is_none() {
            return Err(crate::error!(
                "Missing incremental link-options hash in incremental state"
            ));
        }
        let input_order_hash = if lines
            .peek()
            .is_some_and(|line| line.starts_with("input-order\t"))
        {
            Some(parse_prefixed_line(lines.next(), "input-order")?.to_owned())
        } else {
            None
        };
        if version == STATE_VERSION && input_order_hash.is_none() {
            return Err(crate::error!(
                "Missing incremental input-order hash in incremental state"
            ));
        }
        let sld_version = if lines
            .peek()
            .is_some_and(|line| line.starts_with("sld-version\t"))
        {
            Some(parse_prefixed_line(lines.next(), "sld-version")?.to_owned())
        } else {
            None
        };
        if version == STATE_VERSION && sld_version.is_none() {
            return Err(crate::error!("Missing sld version in incremental state"));
        }
        let link_start = if lines
            .peek()
            .is_some_and(|line| line.starts_with("link-start\t"))
        {
            parse_link_start_line(lines.next())?
        } else {
            None
        };
        let output = parse_content_line(lines.next(), "output")?;
        let build_id_hashes = if lines
            .peek()
            .is_some_and(|line| line.starts_with("build-id-hash\t"))
        {
            parse_build_id_hash_line(lines.next())?
        } else {
            None
        };

        let input_count: usize = parse_prefixed_line(lines.next(), "inputs")?
            .parse()
            .context("Invalid incremental input count")?;

        let mut input_files = Vec::with_capacity(input_count);
        for _ in 0..input_count {
            let line = lines.next().context("Missing incremental input record")?;
            input_files.push(parse_input_line(line, patch_section_mode)?);
        }

        let macho_symbol_resolutions_file = if lines
            .peek()
            .is_some_and(|line| line.starts_with("macho-resolutions-file\t"))
        {
            let file = parse_prefixed_line(lines.next(), "macho-resolutions-file")?;
            if file == ABSENT_FIELD {
                None
            } else {
                validate_macho_resolutions_file_name(file)?;
                Some(file.to_owned())
            }
        } else if version == STATE_VERSION || version == STATE_VERSION_V37 {
            return Err(crate::error!(
                "Missing Mach-O resolution sidecar in incremental state"
            ));
        } else {
            None
        };

        let mut sections_file = None;
        let mut patch_records_file = None;
        let mut patch_record_locations = Vec::new();
        let mut raw_patch_record_locations = None;
        let mut relocations = Vec::new();
        let mut fdes = Vec::new();
        let mut dynamic_relocations = Vec::new();
        let sections = if version == STATE_VERSION
            || version == STATE_VERSION_V37
            || version == STATE_VERSION_V36
            || version == STATE_VERSION_V35
            || version == STATE_VERSION_V34
            || version == STATE_VERSION_V33
            || version == STATE_VERSION_V32
            || version == STATE_VERSION_V31
            || version == STATE_VERSION_V30
            || version == STATE_VERSION_V29
            || version == STATE_VERSION_V28
            || version == STATE_VERSION_V27
            || version == STATE_VERSION_V26
            || version == STATE_VERSION_V25
            || version == STATE_VERSION_V24
            || version == STATE_VERSION_V23
            || version == STATE_VERSION_V22
            || version == STATE_VERSION_V21
            || version == STATE_VERSION_V20
            || version == STATE_VERSION_V19
            || version == STATE_VERSION_V18
            || version == STATE_VERSION_V17
            || version == STATE_VERSION_V16
            || version == STATE_VERSION_V15
            || version == STATE_VERSION_V14
            || version == STATE_VERSION_V13
            || version == STATE_VERSION_V12
            || version == STATE_VERSION_V11
            || version == STATE_VERSION_V10
            || version == STATE_VERSION_V9
            || version == STATE_VERSION_V8
            || version == STATE_VERSION_V7
            || version == STATE_VERSION_V6
            || version == STATE_VERSION_V5
        {
            let first_line = lines
                .next()
                .context("Missing incremental section input count")?;
            if first_line.starts_with("indexed-sections-file\t") {
                if version != STATE_VERSION
                    && version != STATE_VERSION_V37
                    && version != STATE_VERSION_V36
                    && version != STATE_VERSION_V35
                    && version != STATE_VERSION_V34
                    && version != STATE_VERSION_V33
                    && version != STATE_VERSION_V32
                    && version != STATE_VERSION_V31
                    && version != STATE_VERSION_V30
                {
                    return Err(crate::error!(
                        "Indexed incremental sections require incremental state version `{STATE_VERSION}`, `{STATE_VERSION_V37}`, `{STATE_VERSION_V36}`, `{STATE_VERSION_V35}`, `{STATE_VERSION_V34}`, `{STATE_VERSION_V33}`, `{STATE_VERSION_V32}`, `{STATE_VERSION_V31}`, or `{STATE_VERSION_V30}`"
                    ));
                }
                let file =
                    parse_prefixed_line(Some(first_line), "indexed-sections-file")?.to_owned();
                validate_sections_file_name(&file)?;
                sections_file = Some(file.clone());
                patch_records_file = Some(file);
                (patch_record_locations, raw_patch_record_locations) =
                    parse_patch_record_location_table(&mut lines, parse_patch_record_locations)?;
                Vec::new()
            } else if first_line.starts_with("sections-file\t") {
                let file = parse_prefixed_line(Some(first_line), "sections-file")?.to_owned();
                validate_sections_file_name(&file)?;
                let records = load_sections(&file)?
                    .map(|contents| parse_compact_records_block(contents.lines()))
                    .transpose()?
                    .unwrap_or_default();
                sections_file = Some(file);
                if lines
                    .peek()
                    .is_some_and(|line| line.starts_with("patch-records-file\t"))
                {
                    let file = parse_prefixed_line(lines.next(), "patch-records-file")?.to_owned();
                    validate_sections_file_name(&file)?;
                    patch_records_file = Some(file);
                    if lines
                        .peek()
                        .is_some_and(|line| line.starts_with("patch-records\t"))
                    {
                        (patch_record_locations, raw_patch_record_locations) =
                            parse_patch_record_location_table(
                                &mut lines,
                                parse_patch_record_locations,
                            )?;
                    }
                }
                relocations = records.relocations;
                fdes = records.fdes;
                dynamic_relocations = records.dynamic_relocations;
                records.sections
            } else {
                let records =
                    parse_compact_records_block(std::iter::once(first_line).chain(&mut lines))?;
                relocations = records.relocations;
                fdes = records.fdes;
                dynamic_relocations = records.dynamic_relocations;
                records.sections
            }
        } else if version == STATE_VERSION_V4
            || version == STATE_VERSION_V3
            || version == STATE_VERSION_V2
        {
            let section_count: usize = parse_prefixed_line(lines.next(), "sections")?
                .parse()
                .context("Invalid incremental section count")?;
            let mut sections = Vec::with_capacity(section_count);
            for _ in 0..section_count {
                let line = lines.next().context("Missing incremental section record")?;
                sections.push(parse_section_line(line)?);
            }
            sections
        } else {
            Vec::new()
        };

        if lines.next().is_some() {
            return Err(crate::error!("Unexpected trailing incremental state data"));
        }

        Ok(Self {
            args_hash,
            link_options_hash,
            input_order_hash,
            sld_version,
            link_start,
            output,
            build_id_hashes,
            input_files,
            sections,
            relocations,
            macho_symbol_resolutions: Vec::new(),
            fdes,
            dynamic_relocations,
            sections_file,
            macho_symbol_resolutions_file,
            patch_records_file,
            patch_record_locations,
            raw_patch_record_locations,
        })
    }

    fn write(&self, state_dir: &Path) -> Result {
        let macho_symbol_resolutions_file = if self.macho_symbol_resolutions.is_empty() {
            self.macho_symbol_resolutions_file.clone()
        } else {
            Some(write_macho_resolutions_sidecar(
                state_dir,
                &self.macho_symbol_resolutions,
            )?)
        };
        let (sections_file, locations) = write_indexed_records_streaming(
            state_dir,
            &self.sections,
            &self.relocations,
            &self.fdes,
            &self.dynamic_relocations,
        )?;
        self.write_index_with_sections_files(
            state_dir,
            &sections_file,
            Some(&sections_file),
            &locations,
            None,
            macho_symbol_resolutions_file.as_deref(),
        )
    }

    fn write_publishing_index(&self, state_dir: &Path) -> Result {
        self.write_index_with_sections_files(
            state_dir,
            PUBLISHING_SECTIONS_FILE,
            None,
            &[],
            None,
            self.macho_symbol_resolutions_file.as_deref(),
        )
    }

    fn write_metadata_update(&self, state_dir: &Path) -> Result {
        if self.sections_file.is_some() {
            self.write_index(state_dir)
        } else {
            self.write(state_dir)
        }
    }

    fn write_metadata_update_for_inputs(
        &self,
        state_dir: &Path,
        input_indices: &[usize],
    ) -> Result {
        if self.sections_file.is_none() {
            return self.write(state_dir);
        }
        std::fs::create_dir_all(state_dir).with_context(|| {
            format!(
                "Failed to create incremental state directory `{}`",
                state_dir.display()
            )
        })?;

        let path = metadata_update_path(state_dir);
        let tmp_path = state_dir.join(format!("{METADATA_UPDATE_FILE}.tmp"));
        std::fs::write(&tmp_path, self.render_metadata_update(input_indices)).with_context(
            || {
                format!(
                    "Failed to write incremental metadata update `{}`",
                    tmp_path.display()
                )
            },
        )?;
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp_path, &path).with_context(|| {
            format!(
                "Failed to install incremental metadata update `{}`",
                path.display()
            )
        })?;
        Ok(())
    }

    fn write_index(&self, state_dir: &Path) -> Result {
        let sections_file = self.sections_file.as_deref().unwrap_or(SECTIONS_FILE);
        self.write_index_with_sections_files(
            state_dir,
            sections_file,
            self.patch_records_file.as_deref(),
            &self.patch_record_locations,
            self.raw_patch_record_locations.as_deref(),
            self.macho_symbol_resolutions_file.as_deref(),
        )
    }

    fn write_index_with_sections_files(
        &self,
        state_dir: &Path,
        sections_file: &str,
        patch_records_file: Option<&str>,
        patch_record_locations: &[PatchRecordLocation],
        raw_patch_record_locations: Option<&str>,
        macho_symbol_resolutions_file: Option<&str>,
    ) -> Result {
        std::fs::create_dir_all(state_dir).with_context(|| {
            format!(
                "Failed to create incremental state directory `{}`",
                state_dir.display()
            )
        })?;

        let path = state_dir.join(INDEX_FILE);
        let tmp_path = state_dir.join(format!("{INDEX_FILE}.tmp"));
        std::fs::write(
            &tmp_path,
            self.render_index(
                sections_file,
                patch_records_file,
                patch_record_locations,
                raw_patch_record_locations,
                macho_symbol_resolutions_file,
            ),
        )
        .with_context(|| format!("Failed to write incremental state `{}`", tmp_path.display()))?;
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp_path, &path)
            .with_context(|| format!("Failed to install incremental state `{}`", path.display()))?;
        let _ = std::fs::remove_file(metadata_update_path(state_dir));
        Ok(())
    }

    #[cfg(test)]
    fn write_sections(&self, state_dir: &Path, file_name: &str, contents: &str) -> Result {
        std::fs::create_dir_all(state_dir).with_context(|| {
            format!(
                "Failed to create incremental state directory `{}`",
                state_dir.display()
            )
        })?;

        let path = state_dir.join(file_name);
        let tmp_path = state_dir.join(format!("{file_name}.tmp"));
        std::fs::write(&tmp_path, contents).with_context(|| {
            format!(
                "Failed to write incremental sections `{}`",
                tmp_path.display()
            )
        })?;
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp_path, &path).with_context(|| {
            format!(
                "Failed to install incremental sections `{}`",
                path.display()
            )
        })?;
        Ok(())
    }

    fn render_index(
        &self,
        sections_file: &str,
        patch_records_file: Option<&str>,
        patch_record_locations: &[PatchRecordLocation],
        raw_patch_record_locations: Option<&str>,
        macho_symbol_resolutions_file: Option<&str>,
    ) -> String {
        let mut out = self
            .render_header_and_inputs_with_macho_resolutions_file(macho_symbol_resolutions_file);
        if patch_records_file == Some(sections_file) {
            writeln!(&mut out, "indexed-sections-file\t{sections_file}").unwrap();
            render_patch_record_location_table(
                &mut out,
                patch_record_locations,
                raw_patch_record_locations,
            );
        } else {
            writeln!(&mut out, "sections-file\t{sections_file}").unwrap();
        }
        if let Some(patch_records_file) = patch_records_file
            && patch_records_file != sections_file
        {
            writeln!(&mut out, "patch-records-file\t{patch_records_file}").unwrap();
            render_patch_record_location_table(
                &mut out,
                patch_record_locations,
                raw_patch_record_locations,
            );
        }
        out
    }

    fn render_metadata_update(&self, input_indices: &[usize]) -> String {
        let mut out = String::new();
        writeln!(&mut out, "{METADATA_UPDATE_VERSION}").unwrap();
        writeln!(
            &mut out,
            "link-start\t{}",
            self.link_start
                .as_ref()
                .map_or_else(|| ABSENT_FIELD.to_owned(), FileIdentity::render)
        )
        .unwrap();
        writeln!(
            &mut out,
            "output\t{}\t{}\t{}",
            self.output.len,
            self.output.hash,
            self.output.render_identity()
        )
        .unwrap();
        writeln!(
            &mut out,
            "build-id-hash\t{}",
            self.build_id_hashes
                .as_ref()
                .map_or_else(|| ABSENT_FIELD.to_owned(), render_build_id_hash_state)
        )
        .unwrap();
        writeln!(
            &mut out,
            "macho-resolutions-file\t{}",
            self.macho_symbol_resolutions_file
                .as_deref()
                .unwrap_or(ABSENT_FIELD)
        )
        .unwrap();
        let mut input_indices = input_indices.to_vec();
        input_indices.sort_unstable();
        input_indices.dedup();
        writeln!(&mut out, "inputs\t{}", input_indices.len()).unwrap();
        for index in input_indices {
            writeln!(
                &mut out,
                "input\t{}\t{}",
                index,
                render_input_line_rest(&self.input_files[index])
            )
            .unwrap();
        }
        out
    }

    #[cfg(test)]
    fn render(&self) -> String {
        let mut out = self.render_header_and_inputs();
        out.push_str(&self.render_sections());
        out
    }

    #[cfg(test)]
    fn render_header_and_inputs(&self) -> String {
        self.render_header_and_inputs_with_macho_resolutions_file(
            self.macho_symbol_resolutions_file.as_deref(),
        )
    }

    fn render_header_and_inputs_with_macho_resolutions_file(
        &self,
        macho_symbol_resolutions_file: Option<&str>,
    ) -> String {
        let mut out = String::new();
        writeln!(&mut out, "{STATE_VERSION}").unwrap();
        writeln!(&mut out, "args\t{}", self.args_hash).unwrap();
        if let Some(hash) = &self.link_options_hash {
            writeln!(&mut out, "link-options\t{hash}").unwrap();
        }
        if let Some(hash) = &self.input_order_hash {
            writeln!(&mut out, "input-order\t{hash}").unwrap();
        }
        if let Some(version) = &self.sld_version {
            writeln!(&mut out, "sld-version\t{version}").unwrap();
        }
        writeln!(
            &mut out,
            "link-start\t{}",
            self.link_start
                .as_ref()
                .map_or_else(|| ABSENT_FIELD.to_owned(), FileIdentity::render)
        )
        .unwrap();
        writeln!(
            &mut out,
            "output\t{}\t{}\t{}",
            self.output.len,
            self.output.hash,
            self.output.render_identity()
        )
        .unwrap();
        writeln!(
            &mut out,
            "build-id-hash\t{}",
            self.build_id_hashes
                .as_ref()
                .map_or_else(|| ABSENT_FIELD.to_owned(), render_build_id_hash_state)
        )
        .unwrap();
        writeln!(&mut out, "inputs\t{}", self.input_files.len()).unwrap();
        for input in &self.input_files {
            writeln!(&mut out, "input\t{}", render_input_line_rest(input)).unwrap();
        }
        writeln!(
            &mut out,
            "macho-resolutions-file\t{}",
            macho_symbol_resolutions_file.unwrap_or(ABSENT_FIELD)
        )
        .unwrap();
        out
    }

    fn apply_metadata_update(
        &mut self,
        state_dir: &Path,
        patch_section_mode: PatchSectionReadMode,
    ) -> Result {
        let path = metadata_update_path(state_dir);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut lines = contents.lines();
        let version = lines
            .next()
            .context("Missing incremental metadata update header")?;
        if version != METADATA_UPDATE_VERSION && version != METADATA_UPDATE_VERSION_V1 {
            return Err(crate::error!(
                "Unsupported incremental metadata update version `{version}`"
            ));
        }
        self.link_start = parse_link_start_line(lines.next())?;
        self.output = parse_content_line(lines.next(), "output")?;
        self.build_id_hashes = parse_build_id_hash_line(lines.next())?;
        if version == METADATA_UPDATE_VERSION {
            let file = parse_prefixed_line(lines.next(), "macho-resolutions-file")?;
            self.macho_symbol_resolutions_file = if file == ABSENT_FIELD {
                None
            } else {
                validate_macho_resolutions_file_name(file)?;
                Some(file.to_owned())
            };
            self.macho_symbol_resolutions.clear();
        }
        let input_count: usize = parse_prefixed_line(lines.next(), "inputs")?
            .parse()
            .context("Invalid incremental metadata update input count")?;
        for _ in 0..input_count {
            let rest = parse_prefixed_line(lines.next(), "input")?;
            let (index, input_line) = rest
                .split_once('\t')
                .context("Malformed incremental metadata update input")?;
            let index: usize = index
                .parse()
                .context("Invalid incremental metadata update input index")?;
            let Some(input) = self.input_files.get_mut(index) else {
                return Err(crate::error!(
                    "Incremental metadata update input index out of bounds"
                ));
            };
            *input = parse_input_line(&format!("input\t{input_line}"), patch_section_mode)?;
        }
        if lines.next().is_some() {
            return Err(crate::error!(
                "Unexpected trailing incremental metadata update data"
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn render_sections(&self) -> String {
        let mut out = String::new();
        self.write_rendered_sections(&mut out)
            .expect("writing incremental sections to String should not fail");
        out
    }

    #[cfg(test)]
    fn write_rendered_sections(&self, out: &mut impl std::fmt::Write) -> std::fmt::Result {
        let sections = self.sections.iter().collect::<Vec<_>>();
        let relocations = self.relocations.iter().collect::<Vec<_>>();
        let fdes = self.fdes.iter().collect::<Vec<_>>();
        let dynamic_relocations = self.dynamic_relocations.iter().collect::<Vec<_>>();
        write_rendered_records(out, &sections, &relocations, &fdes, &dynamic_relocations)
    }
}

fn write_rendered_records(
    mut out: &mut impl std::fmt::Write,
    sections: &[&SectionRecord],
    relocations: &[&RelocationRecord],
    fdes: &[&FdeRecord],
    dynamic_relocations: &[&DynamicRelocationRecord],
) -> std::fmt::Result {
    let mut section_inputs = Vec::new();
    let mut section_input_ids = HashMap::new();
    for section in sections {
        add_section_input(
            &mut section_inputs,
            &mut section_input_ids,
            section.input_file.as_str(),
            section.input.as_str(),
        );
    }
    for relocation in relocations {
        add_section_input(
            &mut section_inputs,
            &mut section_input_ids,
            relocation.input_file.as_str(),
            relocation.input.as_str(),
        );
        if let Some(target) = &relocation.target {
            add_section_input(
                &mut section_inputs,
                &mut section_input_ids,
                target.input_file.as_str(),
                target.input.as_str(),
            );
        }
    }
    for fde in fdes {
        add_section_input(
            &mut section_inputs,
            &mut section_input_ids,
            fde.input_file.as_str(),
            fde.input.as_str(),
        );
    }
    for relocation in dynamic_relocations {
        add_section_input(
            &mut section_inputs,
            &mut section_input_ids,
            relocation.input_file.as_str(),
            relocation.input.as_str(),
        );
    }

    writeln!(&mut out, "section-inputs\t{}", section_inputs.len())?;
    for (input_file, input) in section_inputs {
        writeln!(&mut out, "section-input\t{input_file}\t{input}")?;
    }

    writeln!(&mut out, "sections\t{}", sections.len())?;
    for section in sections {
        let section_input_id =
            section_input_ids[&(section.input_file.as_str(), section.input.as_str())];
        writeln!(
            &mut out,
            "section\t{}\t{}\t{}\t{}",
            section_input_id, section.section_index, section.output_offset, section.size
        )?;
    }
    writeln!(&mut out, "relocs\t{}", relocations.len())?;
    for relocation in relocations {
        let section_input_id =
            section_input_ids[&(relocation.input_file.as_str(), relocation.input.as_str())];
        let (target_section_input_id, target_section_index, target_section_offset) = relocation
            .target
            .as_ref()
            .map_or((None, None, None), |target| {
                (
                    Some(section_input_ids[&(target.input_file.as_str(), target.input.as_str())]),
                    Some(target.section_index),
                    Some(target.section_offset),
                )
            });
        writeln!(
            &mut out,
            "reloc3\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            section_input_id,
            relocation.section_index,
            relocation.target_symbol_id,
            relocation.relocation_offset,
            relocation.output_offset,
            relocation.size,
            relocation.kind,
            relocation.addend,
            OptionalRecordField(relocation.written_value),
            relocation.target_value,
            OptionalRecordField(relocation.applied_target_value),
            relocation.target_name.as_deref().unwrap_or(ABSENT_FIELD),
            OptionalRecordField(target_section_input_id),
            OptionalRecordField(target_section_index),
            OptionalRecordField(target_section_offset)
        )?;
    }
    writeln!(&mut out, "fdes\t{}", fdes.len())?;
    for fde in fdes {
        let section_input_id = section_input_ids[&(fde.input_file.as_str(), fde.input.as_str())];
        writeln!(
            &mut out,
            "fde\t{}\t{}\t{}\t{}\t{}\t{}",
            section_input_id,
            fde.section_index,
            fde.eh_frame_section_index,
            fde.input_offset,
            fde.output_offset,
            fde.size
        )?;
    }
    writeln!(&mut out, "dynrels\t{}", dynamic_relocations.len())?;
    for relocation in dynamic_relocations {
        let section_input_id =
            section_input_ids[&(relocation.input_file.as_str(), relocation.input.as_str())];
        if let (Some(output_r_offset), Some(output_r_info)) =
            (relocation.output_r_offset, relocation.output_r_info)
        {
            writeln!(
                &mut out,
                "dynrel\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                section_input_id,
                relocation.section_index,
                relocation.relocation_offset,
                relocation.output_offset,
                relocation.size,
                output_r_offset,
                output_r_info
            )?;
        } else {
            writeln!(
                &mut out,
                "dynrel\t{}\t{}\t{}\t{}\t{}",
                section_input_id,
                relocation.section_index,
                relocation.relocation_offset,
                relocation.output_offset,
                relocation.size
            )?;
        }
    }
    Ok(())
}

struct OptionalRecordField<T>(Option<T>);

impl<T: std::fmt::Display> std::fmt::Display for OptionalRecordField<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(formatter),
            None => ABSENT_FIELD.fmt(formatter),
        }
    }
}

fn write_indexed_records_streaming(
    state_dir: &Path,
    sections: &[SectionRecord],
    relocations: &[RelocationRecord],
    fdes: &[FdeRecord],
    dynamic_relocations: &[DynamicRelocationRecord],
) -> Result<(String, Vec<PatchRecordLocation>)> {
    std::fs::create_dir_all(state_dir).with_context(|| {
        format!(
            "Failed to create incremental state directory `{}`",
            state_dir.display()
        )
    })?;

    let tmp_path = state_dir.join("sections-indexed.tmp");
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp_path)
        .with_context(|| {
            format!(
                "Failed to create indexed incremental sections `{}`",
                tmp_path.display()
            )
        })?;
    let mut writer = SectionSidecarWriter::new(file);
    let mut records_by_input = HashMap::<&str, CompactRecordRefs<'_>>::new();
    for record in sections {
        records_by_input
            .entry(record.input_file.as_str())
            .or_default()
            .sections
            .push(record);
    }
    let mut relocation_aliases = Vec::new();
    for record in relocations {
        records_by_input
            .entry(record.input_file.as_str())
            .or_default()
            .relocations
            .push(record);
        if let Some(target) = record.target.as_ref()
            && target.input_file != record.input_file
        {
            relocation_aliases.push((target.input_file.as_str(), record.input_file.as_str()));
        }
    }
    for record in fdes {
        records_by_input
            .entry(record.input_file.as_str())
            .or_default()
            .fdes
            .push(record);
    }
    for record in dynamic_relocations {
        records_by_input
            .entry(record.input_file.as_str())
            .or_default()
            .dynamic_relocations
            .push(record);
    }
    let mut input_files = records_by_input
        .iter()
        .map(|(input_file, _)| *input_file)
        .collect::<Vec<_>>();
    input_files.sort_unstable();
    let blocks = input_files
        .into_par_iter()
        .map(|input_file| {
            let records = &records_by_input[input_file];
            let mut sections = records.sections.clone();
            sections.sort_unstable();
            let mut relocations = records.relocations.clone();
            relocations.sort_unstable();
            let mut fdes = records.fdes.clone();
            fdes.sort_unstable();
            let mut dynamic_relocations = records.dynamic_relocations.clone();
            dynamic_relocations.sort_unstable();
            let mut block = String::new();
            write_rendered_records(
                &mut block,
                &sections,
                &relocations,
                &fdes,
                &dynamic_relocations,
            )
            .expect("writing incremental patch records to String should not fail");
            let block = zstd::stream::encode_all(block.as_bytes(), SECTIONS_COMPRESSION_LEVEL)
                .with_context(|| {
                    format!(
                        "Failed to compress indexed incremental sections `{}`",
                        tmp_path.display()
                    )
                })?;
            Ok((input_file, block))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut offset = 0;
    let mut locations = Vec::with_capacity(blocks.len());
    let mut location_by_owner = HashMap::new();
    for (input_file, block) in blocks {
        writer.write_bytes(&block).with_context(|| {
            format!(
                "Failed to write indexed incremental sections `{}`",
                tmp_path.display()
            )
        })?;
        let len = block.len() as u64;
        let location = PatchRecordLocation {
            input_file: input_file.to_owned(),
            offset,
            len,
            hash: hash_bytes(&block),
        };
        location_by_owner.insert(input_file, location.clone());
        locations.push(location);
        offset += len;
    }
    relocation_aliases.sort_unstable();
    relocation_aliases.dedup();
    for (target_input_file, owner_input_file) in relocation_aliases {
        let Some(owner_location) = location_by_owner.get(owner_input_file) else {
            continue;
        };
        let mut location = owner_location.clone();
        location.input_file = target_input_file.to_owned();
        locations.push(location);
    }
    locations.sort_by(|left, right| {
        (
            left.input_file.as_str(),
            left.offset,
            left.len,
            left.hash.as_str(),
        )
            .cmp(&(
                right.input_file.as_str(),
                right.offset,
                right.len,
                right.hash.as_str(),
            ))
    });
    locations.dedup();
    let hash = writer.finish().with_context(|| {
        format!(
            "Failed to finish indexed incremental sections `{}`",
            tmp_path.display()
        )
    })?;
    let file_name = format!("{COMPRESSED_SECTIONS_FILE_PREFIX}{hash}");
    let path = state_dir.join(&file_name);
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&tmp_path, &path).with_context(|| {
        format!(
            "Failed to install indexed incremental sections `{}`",
            path.display()
        )
    })?;
    Ok((file_name, locations))
}

struct SectionSidecarWriter {
    file: std::io::BufWriter<std::fs::File>,
    hasher: blake3::Hasher,
}

impl SectionSidecarWriter {
    fn new(file: std::fs::File) -> Self {
        Self {
            file: std::io::BufWriter::new(file),
            hasher: blake3::Hasher::new(),
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        Ok(())
    }

    fn finish(mut self) -> std::io::Result<String> {
        self.file.flush()?;
        Ok(self.hasher.finalize().to_hex().to_string())
    }
}

fn add_section_input<'a>(
    section_inputs: &mut Vec<(&'a str, &'a str)>,
    section_input_ids: &mut HashMap<(&'a str, &'a str), usize>,
    input_file: &'a str,
    input: &'a str,
) {
    let key = (input_file, input);
    if !section_input_ids.contains_key(&key) {
        let index = section_inputs.len();
        section_input_ids.insert(key, index);
        section_inputs.push(key);
    }
}

fn read_sections_sidecar(state_dir: &Path, file_name: &str) -> Result<String> {
    validate_sections_file_name(file_name)?;
    let path = state_dir.join(file_name);
    if file_name.starts_with(COMPRESSED_SECTIONS_FILE_PREFIX) {
        let bytes = std::fs::read(&path).with_context(|| {
            format!(
                "Failed to read compressed incremental sections `{}`",
                path.display()
            )
        })?;
        let expected_name = compressed_section_sidecar_file_name(&bytes);
        if file_name != expected_name {
            return Err(crate::error!(
                "Incremental sections `{}` do not match their content hash",
                path.display()
            ));
        }
        let contents = zstd::stream::decode_all(bytes.as_slice()).with_context(|| {
            format!(
                "Failed to decompress incremental sections `{}`",
                path.display()
            )
        })?;
        return String::from_utf8(contents)
            .context("Invalid UTF-8 in compressed incremental sections");
    }
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read incremental sections `{}`", path.display()))?;
    if file_name.starts_with(SECTIONS_FILE_PREFIX) {
        let expected_name = section_sidecar_file_name(&contents);
        if file_name != expected_name {
            return Err(crate::error!(
                "Incremental sections `{}` do not match their content hash",
                path.display()
            ));
        }
    }
    Ok(contents)
}

fn write_macho_resolutions_sidecar(
    state_dir: &Path,
    resolutions: &[MachOSymbolResolutionRecord],
) -> Result<String> {
    std::fs::create_dir_all(state_dir).with_context(|| {
        format!(
            "Failed to create incremental state directory `{}`",
            state_dir.display()
        )
    })?;
    let mut contents = String::new();
    write_rendered_macho_symbol_resolutions(&mut contents, resolutions)
        .expect("writing Mach-O resolutions to String should not fail");
    let compressed =
        zstd::stream::encode_all(contents.as_bytes(), MACHO_RESOLUTIONS_COMPRESSION_LEVEL)
            .context("Failed to compress incremental Mach-O resolutions")?;
    let file_name = compressed_macho_resolutions_sidecar_file_name(&compressed);
    let path = state_dir.join(&file_name);
    if !path.exists() {
        let tmp_path = state_dir.join(format!("{file_name}.tmp"));
        std::fs::write(&tmp_path, &compressed).with_context(|| {
            format!(
                "Failed to write incremental Mach-O resolutions `{}`",
                tmp_path.display()
            )
        })?;
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp_path, &path).with_context(|| {
            format!(
                "Failed to install incremental Mach-O resolutions `{}`",
                path.display()
            )
        })?;
    }
    Ok(file_name)
}

fn read_macho_resolutions_sidecar(
    state_dir: &Path,
    file_name: &str,
) -> Result<Vec<MachOSymbolResolutionRecord>> {
    validate_macho_resolutions_file_name(file_name)?;
    let path = state_dir.join(file_name);
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "Failed to read incremental Mach-O resolutions `{}`",
            path.display()
        )
    })?;
    let expected_name = compressed_macho_resolutions_sidecar_file_name(&bytes);
    if file_name != expected_name {
        return Err(crate::error!(
            "Incremental Mach-O resolutions `{}` do not match their content hash",
            path.display()
        ));
    }
    let contents = zstd::stream::decode_all(bytes.as_slice()).with_context(|| {
        format!(
            "Failed to decompress incremental Mach-O resolutions `{}`",
            path.display()
        )
    })?;
    let contents =
        std::str::from_utf8(&contents).context("Invalid UTF-8 in Mach-O resolution sidecar")?;
    parse_macho_symbol_resolutions(contents.lines())
}

fn write_rendered_macho_symbol_resolutions(
    mut out: &mut impl std::fmt::Write,
    resolutions: &[MachOSymbolResolutionRecord],
) -> std::fmt::Result {
    let mut target_inputs = Vec::new();
    let mut target_input_ids = HashMap::new();
    for resolution in resolutions {
        if let Some(target) = resolution.target.as_ref() {
            add_section_input(
                &mut target_inputs,
                &mut target_input_ids,
                target.input_file.as_str(),
                target.input.as_str(),
            );
        }
    }
    writeln!(&mut out, "macho-resolution-inputs\t{}", target_inputs.len())?;
    for (input_file, input) in target_inputs {
        writeln!(&mut out, "macho-resolution-input\t{input_file}\t{input}")?;
    }
    writeln!(&mut out, "macho-resolutions\t{}", resolutions.len())?;
    for resolution in resolutions {
        let thunk_addresses = if resolution.thunk_addresses.is_empty() {
            ABSENT_FIELD.to_owned()
        } else {
            resolution
                .thunk_addresses
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        let (target_input_id, target_section_index, target_section_offset) = resolution
            .target
            .as_ref()
            .map_or((None, None, None), |target| {
                (
                    Some(target_input_ids[&(target.input_file.as_str(), target.input.as_str())]),
                    Some(target.section_index),
                    Some(target.section_offset),
                )
            });
        writeln!(
            &mut out,
            "macho-resolution\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            resolution.name,
            OptionalRecordField(resolution.direct_value),
            OptionalRecordField(resolution.got_address),
            OptionalRecordField(resolution.stub_address),
            thunk_addresses,
            OptionalRecordField(target_input_id),
            OptionalRecordField(target_section_index),
            OptionalRecordField(target_section_offset),
        )?;
    }
    Ok(())
}

fn parse_macho_symbol_resolutions<'a>(
    mut lines: impl Iterator<Item = &'a str>,
) -> Result<Vec<MachOSymbolResolutionRecord>> {
    let input_count: usize = parse_prefixed_line(lines.next(), "macho-resolution-inputs")?
        .parse()
        .context("Invalid Mach-O resolution input count")?;
    let mut inputs = Vec::with_capacity(input_count);
    for _ in 0..input_count {
        let rest = parse_prefixed_line(lines.next(), "macho-resolution-input")?;
        let (input_file, input) = rest
            .split_once('\t')
            .context("Malformed Mach-O resolution input")?;
        inputs.push((SharedText::from(input_file), SharedText::from(input)));
    }
    let resolution_count: usize = parse_prefixed_line(lines.next(), "macho-resolutions")?
        .parse()
        .context("Invalid Mach-O resolution count")?;
    let mut resolutions = Vec::with_capacity(resolution_count);
    for _ in 0..resolution_count {
        let rest = parse_prefixed_line(lines.next(), "macho-resolution")?;
        let parts = rest.split('\t').collect::<Vec<_>>();
        if parts.len() != 8 {
            return Err(crate::error!("Malformed Mach-O resolution record"));
        }
        let parse_optional_u64 = |value: &str, field: &str| -> Result<Option<u64>> {
            (value != ABSENT_FIELD)
                .then(|| {
                    value
                        .parse()
                        .with_context(|| format!("Invalid Mach-O resolution {field}"))
                })
                .transpose()
        };
        let direct_value = parse_optional_u64(parts[1], "direct value")?;
        let got_address = parse_optional_u64(parts[2], "GOT address")?;
        let stub_address = parse_optional_u64(parts[3], "stub address")?;
        let thunk_addresses = if parts[4] == ABSENT_FIELD {
            Vec::new()
        } else {
            parts[4]
                .split(',')
                .map(|address| {
                    address
                        .parse()
                        .context("Invalid Mach-O resolution thunk address")
                })
                .collect::<Result<Vec<_>>>()?
        };
        let target =
            if parts[5] == ABSENT_FIELD && parts[6] == ABSENT_FIELD && parts[7] == ABSENT_FIELD {
                None
            } else {
                let input_id: usize = parts[5]
                    .parse()
                    .context("Invalid Mach-O resolution target input index")?;
                let (input_file, input) = inputs
                    .get(input_id)
                    .context("Mach-O resolution target input index out of bounds")?;
                Some(RelocationTargetRecord {
                    input_file: input_file.clone(),
                    input: input.clone(),
                    section_index: parts[6]
                        .parse()
                        .context("Invalid Mach-O resolution target section index")?,
                    section_offset: parts[7]
                        .parse()
                        .context("Invalid Mach-O resolution target section offset")?,
                })
            };
        resolutions.push(MachOSymbolResolutionRecord {
            name: SharedText::from(parts[0]),
            direct_value,
            got_address,
            stub_address,
            thunk_addresses,
            target,
        });
    }
    if lines.next().is_some() {
        return Err(crate::error!(
            "Unexpected trailing Mach-O resolution sidecar data"
        ));
    }
    Ok(resolutions)
}

fn validate_macho_resolutions_file_name(file_name: &str) -> Result {
    if !file_name.starts_with(COMPRESSED_MACHO_RESOLUTIONS_FILE_PREFIX)
        || file_name.contains('/')
        || file_name.contains('\\')
        || Path::new(file_name).is_absolute()
    {
        return Err(crate::error!(
            "Invalid incremental Mach-O resolution sidecar name `{file_name}`"
        ));
    }
    Ok(())
}

fn validate_sections_file_name(file_name: &str) -> Result {
    if file_name == SECTIONS_FILE {
        return Ok(());
    }
    if !(file_name.starts_with(SECTIONS_FILE_PREFIX)
        || file_name.starts_with(COMPRESSED_SECTIONS_FILE_PREFIX))
        || file_name.contains('/')
        || file_name.contains('\\')
        || Path::new(file_name).is_absolute()
    {
        return Err(crate::error!(
            "Invalid incremental sections sidecar name `{file_name}`"
        ));
    }
    Ok(())
}

impl PartialEq for FileContentState {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        if !self.hash.is_empty() && !other.hash.is_empty() {
            return self.hash == other.hash;
        }
        self.identity.is_some() && self.identity == other.identity
    }
}

impl SectionRecord {
    #[cfg(test)]
    fn new(
        input: InputRef<'_>,
        section_index: object::SectionIndex,
        output_offset: u64,
        size: u64,
    ) -> Self {
        Self::new_with_texts(
            encode_path(&input.file.filename).into(),
            encode_input_ref(input).into(),
            section_index,
            output_offset,
            size,
        )
    }

    fn new_with_texts(
        input_file: SharedText,
        input: SharedText,
        section_index: object::SectionIndex,
        output_offset: u64,
        size: u64,
    ) -> Self {
        Self {
            input_file,
            input,
            section_index: section_index.0 as u32,
            output_offset,
            size,
        }
    }
}

impl RelocationRecord {
    #[cfg(test)]
    fn new(
        input: InputRef<'_>,
        section_index: object::SectionIndex,
        target_symbol_id: u32,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        kind: u32,
        addend: i64,
        written_value: u64,
        target_value: u64,
        target_name: Option<String>,
        target: Option<(InputRef<'_>, object::SectionIndex, u64)>,
    ) -> Self {
        Self::new_with_texts(
            encode_path(&input.file.filename).into(),
            encode_input_ref(input).into(),
            section_index,
            target_symbol_id,
            relocation_offset,
            output_offset,
            size,
            kind,
            addend,
            written_value,
            target_value,
            None,
            target_name.map(Into::into),
            target.map(|(target_input, target_section_index, section_offset)| {
                (
                    encode_path(&target_input.file.filename).into(),
                    encode_input_ref(target_input).into(),
                    target_section_index,
                    section_offset,
                )
            }),
        )
    }

    fn new_with_texts(
        input_file: SharedText,
        input: SharedText,
        section_index: object::SectionIndex,
        target_symbol_id: u32,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        kind: u32,
        addend: i64,
        written_value: u64,
        target_value: u64,
        applied_target_value: Option<u64>,
        target_name: Option<SharedText>,
        target: Option<(SharedText, SharedText, object::SectionIndex, u64)>,
    ) -> Self {
        Self {
            target_symbol_id,
            written_value: Some(written_value),
            target_value,
            applied_target_value,
            target_name,
            target: target.map(|(input_file, input, section_index, section_offset)| {
                RelocationTargetRecord::new(input_file, input, section_index, section_offset)
            }),
            input_file,
            input,
            section_index: section_index.0 as u32,
            relocation_offset,
            output_offset,
            size,
            kind,
            addend,
        }
    }
}

impl DeferredRelocationRecord<'_> {
    fn materialize(self, record_texts: &RecordTextInterner) -> Result<RelocationRecord> {
        let (input_file, input) = record_texts.intern_input(self.input);
        let target = record_texts.intern_relocation_target(self.target_symbol_id, || {
            Ok((self.target.target_name.map(hex::encode), self.target.target))
        })?;
        Ok(RelocationRecord::new_with_texts(
            input_file,
            input,
            self.section_index,
            self.target_symbol_id,
            self.relocation_offset,
            self.output_offset,
            self.size,
            self.kind,
            self.addend,
            self.written_value,
            self.target_value,
            self.applied_target_value,
            target.target_name,
            target.target,
        ))
    }
}

impl RelocationTargetRecord {
    fn new(
        input_file: SharedText,
        input: SharedText,
        section_index: object::SectionIndex,
        section_offset: u64,
    ) -> Self {
        Self {
            input_file,
            input,
            section_index: section_index.0 as u32,
            section_offset,
        }
    }
}

impl FdeRecord {
    #[cfg(test)]
    fn new(
        input: InputRef<'_>,
        section_index: object::SectionIndex,
        eh_frame_section_index: object::SectionIndex,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Self {
        Self::new_with_texts(
            encode_path(&input.file.filename).into(),
            encode_input_ref(input).into(),
            section_index,
            eh_frame_section_index,
            input_offset,
            output_offset,
            size,
        )
    }

    fn new_with_texts(
        input_file: SharedText,
        input: SharedText,
        section_index: object::SectionIndex,
        eh_frame_section_index: object::SectionIndex,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Self {
        Self {
            input_file,
            input,
            section_index: section_index.0 as u32,
            eh_frame_section_index: eh_frame_section_index.0 as u32,
            input_offset,
            output_offset,
            size,
        }
    }
}

impl DynamicRelocationRecord {
    #[cfg(test)]
    fn new(
        input: InputRef<'_>,
        section_index: object::SectionIndex,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        output_info: Option<(u64, u64)>,
    ) -> Self {
        Self::new_with_texts(
            encode_path(&input.file.filename).into(),
            encode_input_ref(input).into(),
            section_index,
            relocation_offset,
            output_offset,
            size,
            output_info,
        )
    }

    fn new_with_texts(
        input_file: SharedText,
        input: SharedText,
        section_index: object::SectionIndex,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        output_info: Option<(u64, u64)>,
    ) -> Self {
        let (output_r_offset, output_r_info) = output_info
            .map_or((None, None), |(r_offset, r_info)| {
                (Some(r_offset), Some(r_info))
            });
        Self {
            input_file,
            input,
            section_index: section_index.0 as u32,
            relocation_offset,
            output_offset,
            size,
            output_r_offset,
            output_r_info,
        }
    }

    fn has_restorable_rela_output_info(&self) -> bool {
        self.size == crate::elf::RELA_ENTRY_SIZE
            && self.output_r_offset.is_some()
            && self.output_r_info.is_some()
    }
}

fn generated_section_record(name: &str, output_offset: u64, size: u64) -> SectionRecord {
    SectionRecord {
        input_file: GENERATED_SECTION_INPUT_FILE.into(),
        input: name.into(),
        section_index: GENERATED_SECTION_INDEX,
        output_offset,
        size,
    }
}

impl FileContentState {
    fn from_path_identity_only(path: &Path) -> Result<Self> {
        let Some(identity) = FileIdentity::from_path(path)? else {
            return Self::from_path(path);
        };
        Ok(Self {
            len: identity.len,
            hash: String::new(),
            identity: Some(identity),
        })
    }

    fn from_path(path: &Path) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("Failed to read `{}`", path.display()))?;
        let mut state = Self::from_bytes(&bytes);
        state.identity = FileIdentity::from_path(path).ok().flatten();
        Ok(state)
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            len: bytes.len() as u64,
            hash: hash_bytes(bytes),
            identity: None,
        }
    }

    fn from_input_file(
        input_file: &crate::input_data::InputFile,
        previous: Option<&FileContentState>,
    ) -> Self {
        let identity = FileIdentity::from_path(&input_file.filename).ok().flatten();
        if let (Some(identity), Some(previous)) = (identity.as_ref(), previous)
            && previous.identity.as_ref() == Some(identity)
        {
            let mut state = previous.clone();
            state.identity = Some(identity.clone());
            return state;
        }

        if let Some(identity) = identity.as_ref()
            && previous.is_none_or(|previous| previous.hash.is_empty())
        {
            return Self {
                len: identity.len,
                hash: String::new(),
                identity: Some(identity.clone()),
            };
        }

        let mut state = Self::from_bytes(input_file.full_data());
        state.identity = identity;
        state
    }

    fn identity_matches_path(&self, path: &Path) -> Result<bool> {
        let Some(previous) = self.identity.as_ref() else {
            return Ok(false);
        };
        Ok(FileIdentity::from_path(path)?.as_ref() == Some(previous))
    }

    fn data_identity_matches_path(&self, path: &Path) -> Result<bool> {
        let Some(previous) = self.identity.as_ref() else {
            return Ok(false);
        };
        Ok(FileIdentity::from_path(path)?
            .as_ref()
            .is_some_and(|current| previous.matches_same_data_ignoring_change_time(current)))
    }

    fn identity_is_ambiguous_since(&self, link_start: Option<&FileIdentity>) -> bool {
        self.identity
            .as_ref()
            .zip(link_start)
            .is_some_and(|(identity, link_start)| identity.may_have_changed_since(link_start))
    }

    fn render_identity(&self) -> String {
        self.identity
            .as_ref()
            .map_or_else(|| "-".to_owned(), FileIdentity::render)
    }
}

impl FileIdentity {
    fn matches_same_data_ignoring_change_time(&self, other: &Self) -> bool {
        self.len == other.len
            && self.dev == other.dev
            && self.ino == other.ino
            && self.modified_sec == other.modified_sec
            && self.modified_nsec == other.modified_nsec
    }

    fn modified_on_or_after(&self, lower_bound: &Self) -> bool {
        timestamp_on_or_after(
            self.modified_sec,
            self.modified_nsec,
            lower_bound.modified_sec,
            lower_bound.modified_nsec,
        )
    }

    fn may_have_changed_since(&self, lower_bound: &Self) -> bool {
        self.modified_on_or_after(lower_bound)
            || timestamp_on_or_after(
                self.changed_sec,
                self.changed_nsec,
                lower_bound.modified_sec,
                lower_bound.modified_nsec,
            )
    }

    fn from_path(path: &Path) -> Result<Option<Self>> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("Failed to read metadata for `{}`", path.display()))?;
        #[cfg(unix)]
        {
            Ok(Some(Self::from_metadata(&metadata)))
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Ok(None)
        }
    }

    #[cfg(unix)]
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            dev: metadata.dev(),
            ino: metadata.ino(),
            modified_sec: metadata.mtime(),
            modified_nsec: metadata.mtime_nsec(),
            changed_sec: metadata.ctime(),
            changed_nsec: metadata.ctime_nsec(),
        }
    }

    fn render(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.len,
            self.dev,
            self.ino,
            self.modified_sec,
            self.modified_nsec,
            self.changed_sec,
            self.changed_nsec
        )
    }

    fn parse(value: &str) -> Result<Option<Self>> {
        if value == "-" {
            return Ok(None);
        }

        let mut parts = value.split(':');
        let mut next = |field| {
            parts
                .next()
                .with_context(|| format!("Malformed incremental file identity `{field}`"))
        };
        let identity = Self {
            len: next("len")?
                .parse()
                .context("Invalid incremental file identity length")?,
            dev: next("dev")?
                .parse()
                .context("Invalid incremental file identity device")?,
            ino: next("ino")?
                .parse()
                .context("Invalid incremental file identity inode")?,
            modified_sec: next("mtime")?
                .parse()
                .context("Invalid incremental file identity mtime")?,
            modified_nsec: next("mtime_nsec")?
                .parse()
                .context("Invalid incremental file identity mtime_nsec")?,
            changed_sec: next("ctime")?
                .parse()
                .context("Invalid incremental file identity ctime")?,
            changed_nsec: next("ctime_nsec")?
                .parse()
                .context("Invalid incremental file identity ctime_nsec")?,
        };
        if parts.next().is_some() {
            return Err(crate::error!("Malformed incremental file identity"));
        }
        Ok(Some(identity))
    }
}

fn timestamp_on_or_after(sec: i64, _nsec: i64, lower_sec: i64, _lower_nsec: i64) -> bool {
    // Some filesystems report coarse timestamps or fail to advance nanoseconds for rapid
    // rewrites. Treat the whole filesystem second as ambiguous once it overlaps the link start.
    sec >= lower_sec
}

struct LazyOutputBytes<F> {
    bytes: Option<memmap2::Mmap>,
    load: Option<F>,
}

impl<F> LazyOutputBytes<F>
where
    F: FnOnce() -> Result<memmap2::Mmap>,
{
    fn new(load: F) -> Self {
        Self {
            bytes: None,
            load: Some(load),
        }
    }

    fn get(&mut self) -> Result<&[u8]> {
        if self.bytes.is_none() {
            let load = self
                .load
                .take()
                .context("Incremental output bytes were already consumed")?;
            self.bytes = Some(load()?);
        }
        Ok(self.bytes.as_deref().unwrap_or_default())
    }
}

struct LazyInputSnapshotBytes<F> {
    bytes: Option<Option<Vec<u8>>>,
    load: Option<F>,
}

impl<F> LazyInputSnapshotBytes<F>
where
    F: FnOnce() -> Result<Option<Vec<u8>>>,
{
    fn new(load: F) -> Self {
        Self {
            bytes: None,
            load: Some(load),
        }
    }

    fn get(&mut self) -> Result<Option<&[u8]>> {
        if self.bytes.is_none() {
            let load = self
                .load
                .take()
                .context("Incremental input snapshot bytes were already consumed")?;
            self.bytes = Some(load()?);
        }
        Ok(self.get_if_loaded())
    }

    fn get_if_loaded(&self) -> Option<&[u8]> {
        self.bytes.as_ref().and_then(Option::as_deref)
    }
}

fn read_output_bytes(path: &Path) -> Result<memmap2::Mmap> {
    let file = OpenOptions::new().read(true).open(path).with_context(|| {
        format!(
            "Failed to read output `{}` for incremental state",
            path.display()
        )
    })?;
    unsafe { MmapOptions::new().map(&file) }.with_context(|| {
        format!(
            "Failed to mmap output `{}` for incremental state",
            path.display()
        )
    })
}

#[cfg(test)]
fn record_patch_fingerprints<F>(
    input_files: &mut [FileState],
    file_loader: &FileLoader<'_>,
    sections: &[SectionRecord],
    relocations: &[RelocationRecord],
    fdes: &[FdeRecord],
    dynamic_relocations: &[DynamicRelocationRecord],
    output: &mut LazyOutputBytes<F>,
) -> Result
where
    F: FnOnce() -> Result<memmap2::Mmap>,
{
    record_patch_fingerprints_for_inputs(
        input_files,
        file_loader,
        sections,
        relocations,
        fdes,
        dynamic_relocations,
        output,
        |_| true,
    )
}

#[cfg(test)]
fn record_patch_fingerprints_for_inputs<F, P>(
    input_files: &mut [FileState],
    file_loader: &FileLoader<'_>,
    sections: &[SectionRecord],
    relocations: &[RelocationRecord],
    fdes: &[FdeRecord],
    dynamic_relocations: &[DynamicRelocationRecord],
    output: &mut LazyOutputBytes<F>,
    should_record: P,
) -> Result
where
    F: FnOnce() -> Result<memmap2::Mmap>,
    P: Fn(&crate::input_data::InputFile) -> bool,
{
    let mut sections_by_file = HashMap::<&str, Vec<&SectionRecord>>::new();
    for section in sections {
        sections_by_file
            .entry(section.input_file.as_str())
            .or_default()
            .push(section);
    }

    let mut dynamic_relocations_by_file = HashMap::<&str, Vec<&DynamicRelocationRecord>>::new();
    for relocation in dynamic_relocations {
        dynamic_relocations_by_file
            .entry(relocation.input_file.as_str())
            .or_default()
            .push(relocation);
    }

    let mut relocations_by_file = HashMap::<&str, Vec<&RelocationRecord>>::new();
    for relocation in relocations {
        relocations_by_file
            .entry(relocation.input_file.as_str())
            .or_default()
            .push(relocation);
    }

    let mut relocation_targets_by_file = HashMap::<&str, Vec<&RelocationRecord>>::new();
    for relocation in relocations {
        if let Some(target) = &relocation.target {
            relocation_targets_by_file
                .entry(target.input_file.as_str())
                .or_default()
                .push(relocation);
        }
    }

    let mut fdes_by_file = HashMap::<&str, Vec<&FdeRecord>>::new();
    for fde in fdes {
        fdes_by_file
            .entry(fde.input_file.as_str())
            .or_default()
            .push(fde);
    }

    if sections_by_file.is_empty()
        && relocations_by_file.is_empty()
        && relocation_targets_by_file.is_empty()
        && dynamic_relocations_by_file.is_empty()
        && fdes_by_file.is_empty()
    {
        return Ok(());
    }

    let loaded_by_path = file_loader
        .loaded_files
        .iter()
        .map(|file| (encode_path(&file.filename), *file))
        .collect::<HashMap<_, _>>();

    for input in input_files {
        let Some(sections) = sections_by_file.get(input.path.as_str()) else {
            input.patch = None;
            continue;
        };
        let input_dynamic_relocations = dynamic_relocations_by_file.get(input.path.as_str());
        let input_relocations = relocations_by_file.get(input.path.as_str());
        let input_relocation_targets = relocation_targets_by_file.get(input.path.as_str());
        let input_fdes = fdes_by_file.get(input.path.as_str());
        if input
            .patch
            .as_ref()
            .is_some_and(|patch| patch_state_matches_section_records(patch, sections))
        {
            continue;
        }
        let Some(input_file) = loaded_by_path.get(&input.path) else {
            input.patch = None;
            continue;
        };
        if !should_record(input_file) {
            input.patch = None;
            continue;
        }
        let patch = current_patch_state(
            input_file.data(),
            input.path.as_str(),
            output.get()?,
            sections,
            input_dynamic_relocations
                .map(Vec::as_slice)
                .unwrap_or_default(),
            input_relocations.map(Vec::as_slice).unwrap_or_default(),
            input_relocation_targets
                .map(Vec::as_slice)
                .unwrap_or_default(),
            input_fdes.map(Vec::as_slice).unwrap_or_default(),
            true,
        )?;
        if patch.is_some() && input.content.hash.is_empty() {
            input.content.hash = hash_bytes(input_file.full_data());
        }
        input.patch = patch;
    }

    Ok(())
}

fn current_patch_state(
    bytes: &[u8],
    input_file_path: &str,
    output: &[u8],
    sections: &[&SectionRecord],
    dynamic_relocations: &[&DynamicRelocationRecord],
    relocations: &[&RelocationRecord],
    relocation_targets: &[&RelocationRecord],
    fdes: &[&FdeRecord],
    normalize_rust_archive_patch_inputs: bool,
) -> Result<Option<FilePatchState>> {
    let archive_member_set_proof = archive_member_set_proof(bytes)?;
    let patch_sections = direct_copy_patch_sections(
        bytes,
        input_file_path,
        output,
        sections,
        dynamic_relocations.iter().copied(),
        relocations.iter().copied(),
    )?;
    let dynamic_relocation_patches = dynamic_relocation_patches_for_current_records(
        bytes,
        input_file_path,
        dynamic_relocations.iter().copied(),
    )?;
    let relocation_addend_ranges = relocation_addend_ranges_for_current_records(
        bytes,
        input_file_path,
        relocations.iter().copied(),
    )?;
    let relocation_target_ranges = relocation_target_ranges_for_current_records(
        bytes,
        input_file_path,
        relocation_targets.iter().copied(),
    )?;
    let fde_relocation_ranges =
        fde_patch_input_ranges_for_current_records(bytes, input_file_path, fdes.iter().copied())?;
    Ok(patch_fingerprint_for_current_records_with_extra_ranges_and_archive_member_patch_fingerprints(
        bytes,
        input_file_path,
        patch_sections.iter().cloned(),
        dynamic_relocation_patches
            .iter()
            .filter_map(|patch| patch.input_range.clone())
            .chain(relocation_addend_ranges)
            .chain(relocation_target_ranges)
            .chain(fde_relocation_ranges),
        normalize_rust_archive_patch_inputs,
    )?
    .map(|(fingerprint, archive_member_patch_fingerprints)| FilePatchState {
        fingerprint,
        archive_member_set_proof,
        archive_member_patch_fingerprints,
        sections: patch_sections
            .iter()
            .map(|section| FilePatchSectionState {
                input: section.input.clone(),
                section_index: section.section_index,
                section_name: section.section_name.clone(),
                input_size: section.input_size,
                output_offset: section.output_offset,
                output_size: section.output_size,
                data_hash: section.data_hash.clone(),
                cstring_nul_boundaries_hash: section.cstring_nul_boundaries_hash.clone(),
            })
            .collect(),
        raw_sections: None,
    }))
}

fn current_patch_state_from_snapshot(
    state_dir: &Path,
    input: &FileState,
    output: &[u8],
    sections: &[SectionRecord],
    relocations: &[RelocationRecord],
    fdes: &[FdeRecord],
    dynamic_relocations: &[DynamicRelocationRecord],
    normalize_rust_archive_patch_inputs: bool,
) -> Result<Option<FilePatchState>> {
    let Some(bytes) = read_verified_input_snapshot(state_dir, input)? else {
        return Ok(None);
    };
    let sections = sections
        .iter()
        .filter(|section| section.input_file == input.path)
        .collect::<Vec<_>>();
    let dynamic_relocations = dynamic_relocations
        .iter()
        .filter(|relocation| relocation.input_file == input.path)
        .collect::<Vec<_>>();
    let input_relocations = relocations
        .iter()
        .filter(|relocation| relocation.input_file == input.path)
        .collect::<Vec<_>>();
    let relocation_targets = relocations
        .iter()
        .filter(|relocation| {
            relocation
                .target
                .as_ref()
                .is_some_and(|target| target.input_file == input.path)
        })
        .collect::<Vec<_>>();
    let fdes = fdes
        .iter()
        .filter(|record| record.input_file == input.path)
        .collect::<Vec<_>>();
    current_patch_state(
        &bytes,
        input.path.as_str(),
        output,
        &sections,
        &dynamic_relocations,
        &input_relocations,
        &relocation_targets,
        &fdes,
        normalize_rust_archive_patch_inputs,
    )
}

#[cfg(test)]
fn patch_state_matches_section_records(
    patch: &FilePatchState,
    sections: &[&SectionRecord],
) -> bool {
    if patch.sections.is_empty() || patch.sections.len() != sections.len() {
        return false;
    }

    let mut patch_sections = patch
        .sections
        .iter()
        .map(|section| {
            (
                section.input.as_str(),
                section.section_index,
                section.output_offset,
                section.output_size,
            )
        })
        .collect::<Vec<_>>();
    patch_sections.sort();

    let mut section_records = sections
        .iter()
        .map(|section| {
            (
                section.input.as_str(),
                section.section_index,
                section.output_offset,
                section.size,
            )
        })
        .collect::<Vec<_>>();
    section_records.sort();

    patch_sections == section_records
}

fn direct_copy_patch_sections<'a>(
    bytes: &[u8],
    input_file_path: &str,
    output: &[u8],
    sections: &[&'a SectionRecord],
    dynamic_relocations: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
    relocations: impl IntoIterator<Item = &'a RelocationRecord>,
) -> Result<Vec<PatchSection>> {
    let mut patch_sections = Vec::new();
    let dynamic_relocation_offsets =
        dynamic_relocation_offsets_by_input_section(dynamic_relocations);
    let relocation_offsets = relocation_offsets_by_input_section(relocations);

    let mut sections_by_input = HashMap::<&str, Vec<&SectionRecord>>::new();
    for record in sections {
        sections_by_input
            .entry(record.input.as_str())
            .or_default()
            .push(record);
    }

    for (input_ref, records) in sections_by_input {
        let Some(input_bytes) = patch_input_bytes_with_lookup(
            bytes,
            input_file_path,
            input_ref,
            PatchInputLookup::CurrentRecordedRange,
        )?
        else {
            continue;
        };
        let file = object::File::parse(input_bytes.bytes)
            .context("Failed to parse incremental patch candidate input")?;
        for record in records {
            let section = file
                .section_by_index(patch_section_object_index(&file, record.section_index)?)
                .context("Missing incremental patch candidate section")?;
            let data = section
                .data()
                .context("Failed to read incremental patch candidate section data")?;
            let dynamic_relocations =
                dynamic_relocation_offsets.get(&(input_ref, record.section_index));
            let relocations = relocation_offsets.get(&(input_ref, record.section_index));
            let Some(preserve_ranges) = section_direct_patch_preserve_ranges(
                &file,
                &section,
                data,
                dynamic_relocations,
                relocations,
            ) else {
                continue;
            };
            if data.len() > record.size as usize {
                continue;
            }
            let start = record.output_offset as usize;
            let end = start
                .checked_add(record.size as usize)
                .context("Incremental patch output range overflow")?;
            let Some(output_range) = output.get(start..end) else {
                continue;
            };
            let (data_out, padding) = output_range.split_at(data.len());
            if patchable_bytes_match(data_out, data, &preserve_ranges)
                && padding.iter().all(|byte| *byte == 0)
            {
                patch_sections.push(PatchSection {
                    input: record.input.to_string(),
                    section_index: record.section_index,
                    section_name: patch_section_name_for_matching(&section),
                    input_size: data.len() as u64,
                    output_offset: record.output_offset,
                    output_size: record.size,
                    data_hash: Some(hash_bytes(data)),
                    cstring_nul_boundaries_hash: cstring_nul_boundaries_hash_for_section(
                        section.name().ok(),
                        data,
                    ),
                });
            }
        }
    }
    Ok(patch_sections)
}

fn dynamic_relocation_offsets_by_input_section<'a>(
    dynamic_relocations: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
) -> HashMap<(&'a str, u32), HashSet<u64>> {
    let mut offsets = HashMap::<(&str, u32), HashSet<u64>>::new();
    for relocation in dynamic_relocations {
        offsets
            .entry((relocation.input.as_str(), relocation.section_index))
            .or_default()
            .insert(relocation.relocation_offset);
    }
    offsets
}

fn relocation_offsets_by_input_section<'a>(
    relocations: impl IntoIterator<Item = &'a RelocationRecord>,
) -> HashMap<(&'a str, u32), HashSet<u64>> {
    let mut offsets = HashMap::<(&str, u32), HashSet<u64>>::new();
    for relocation in relocations {
        offsets
            .entry((relocation.input.as_str(), relocation.section_index))
            .or_default()
            .insert(relocation.relocation_offset);
    }
    offsets
}

fn section_flags_allow_patching(flags: object::SectionFlags) -> bool {
    match flags {
        object::SectionFlags::Elf { sh_flags } => {
            // Sections that sld actually merges are written by the merge-strings path, so they
            // don't produce direct-copy patch records. Merge-flagged sections that reach this
            // point were copied directly, for example under --no-string-merge.
            sh_flags & u64::from(object::elf::SHF_ALLOC) != 0
        }
        object::SectionFlags::MachO { flags } => {
            matches!(
                flags & object::macho::SECTION_TYPE,
                object::macho::S_REGULAR | object::macho::S_CSTRING_LITERALS
            )
        }
        _ => false,
    }
}

pub(crate) fn section_name_allows_direct_patching(name: &[u8]) -> bool {
    // Keep Mach-O patching to ordinary data, fixed-layout string literals, and fixed-size
    // relocation-free code. Unwind, initializer, and other special sections need separate
    // validation.
    (!name.starts_with(b"__") || matches!(name, b"__data" | b"__const" | b"__cstring" | b"__text"))
        && !matches!(name, b".init" | b".fini")
        && !name.starts_with(b".eh_frame")
        && !name.starts_with(b".init_array")
        && !name.starts_with(b".fini_array")
        && !name.starts_with(b".preinit_array")
        && !name.starts_with(b".ctors")
        && !name.starts_with(b".dtors")
}

pub(crate) fn section_name_allows_incremental_padding(name: &[u8]) -> bool {
    (name.starts_with(b".") || name == b"__const") && section_name_allows_direct_patching(name)
}

fn section_direct_patch_preserve_ranges<'data>(
    file: &object::File<'data>,
    section: &impl object::ObjectSection<'data>,
    section_data: &[u8],
    dynamic_relocation_offsets: Option<&HashSet<u64>>,
    relocation_offsets: Option<&HashSet<u64>>,
) -> Option<Vec<std::ops::Range<usize>>> {
    let section_name = section.name().ok().map(|name| name.as_bytes());
    let is_elf_debug_section = matches!(
        section.flags(),
        object::SectionFlags::Elf { sh_flags }
            if sh_flags & u64::from(object::elf::SHF_ALLOC) == 0
                && section_name.is_some_and(|name| name.starts_with(b".debug_"))
    );
    let is_macho_text = matches!(section.flags(), object::SectionFlags::MachO { .. })
        && section_name == Some(b"__text".as_slice());
    if !(section_flags_allow_patching(section.flags()) || is_elf_debug_section)
        || !section_name.is_none_or(section_name_allows_direct_patching)
        || ((section_name == Some(b"__const".as_slice())
            || section_name == Some(b"__cstring".as_slice()))
            && section.relocations().next().is_some())
    {
        return None;
    }
    if is_macho_text && section.relocations().next().is_some() {
        return macho_aarch64_text_relocation_ranges(file, section, section_data);
    }
    let required_relocation_offsets = if is_elf_debug_section {
        Some(relocation_offsets?)
    } else {
        None
    };

    relocation_preserve_ranges(
        file,
        section,
        section_data,
        dynamic_relocation_offsets,
        required_relocation_offsets,
    )
}

#[derive(PartialEq, Eq)]
struct MachOTextRelocationContext {
    range: std::ops::Range<usize>,
    r_type: u8,
    r_pcrel: bool,
    target: object::RelocationTarget,
    subtractor: Option<object::SymbolIndex>,
    addend: i64,
    bytes: [u8; 4],
}

struct MachOTextRelocationReplay {
    relocation_index: Option<usize>,
    input_file: SharedText,
    input: String,
    section_index: u32,
    previous_range: Option<std::ops::Range<usize>>,
    current_range: std::ops::Range<usize>,
    r_type: u8,
    kind: u32,
    rel_info: RelocationKindInfo,
    previous_written_value: Option<u64>,
    previous_applied_target_value: Option<u64>,
    current_target_value: u64,
    current_relocation_target_candidates: Vec<u64>,
    current_target_section_offset: u64,
    target_symbol_id: u32,
    target_name: Option<SharedText>,
    target_is_global: bool,
    target: Option<RelocationTargetRecord>,
    addend: i64,
}

struct MachOTextRelocationReplays {
    replays: Vec<MachOTextRelocationReplay>,
    rematerialized_sections: Vec<(SharedText, String, u32, String, u32)>,
    symbol_resolutions_changed: bool,
}

fn macho_aarch64_text_relocation_contexts<'data>(
    file: &object::File<'data>,
    section: &impl object::ObjectSection<'data>,
    section_data: &[u8],
) -> Option<Vec<MachOTextRelocationContext>> {
    if file.architecture() != object::Architecture::Aarch64
        || !matches!(section.flags(), object::SectionFlags::MachO { .. })
        || section.name().ok() != Some("__text")
    {
        return None;
    }

    let mut contexts = Vec::new();
    for (offset, relocation) in section.relocations() {
        if relocation.kind() == object::RelocationKind::None {
            continue;
        }
        let object::RelocationFlags::MachO {
            r_type,
            r_pcrel,
            r_length: 2,
        } = relocation.flags()
        else {
            return None;
        };
        if !matches!(
            r_type,
            object::macho::ARM64_RELOC_BRANCH26
                | object::macho::ARM64_RELOC_PAGE21
                | object::macho::ARM64_RELOC_PAGEOFF12
        ) {
            return None;
        }
        let start = usize::try_from(offset).ok()?;
        if start % 4 != 0 {
            return None;
        }
        let end = start.checked_add(4)?;
        if end > section_data.len() {
            return None;
        }
        let bytes = section_data.get(start..end)?.try_into().ok()?;
        contexts.push(MachOTextRelocationContext {
            range: start..end,
            r_type,
            r_pcrel,
            target: relocation.target(),
            subtractor: relocation.subtractor(),
            addend: relocation.addend(),
            bytes,
        });
    }
    contexts.sort_by_key(|context| context.range.start);
    let mut previous_end = 0;
    for context in &contexts {
        if context.range.start < previous_end {
            return None;
        }
        previous_end = context.range.end;
    }
    Some(contexts)
}

fn macho_aarch64_text_relocation_ranges<'data>(
    file: &object::File<'data>,
    section: &impl object::ObjectSection<'data>,
    section_data: &[u8],
) -> Option<Vec<std::ops::Range<usize>>> {
    Some(
        macho_aarch64_text_relocation_contexts(file, section, section_data)?
            .into_iter()
            .map(|context| context.range)
            .collect(),
    )
}

fn macho_relocation_target_identity(
    file: &object::File<'_>,
    target: object::RelocationTarget,
) -> Result<Option<(Vec<u8>, Option<object::SectionIndex>)>> {
    match target {
        object::RelocationTarget::Symbol(symbol_index) => {
            let symbol = file.symbol_by_index(symbol_index)?;
            let name = symbol.name_bytes()?.to_vec();
            Ok(Some((name, symbol.section_index())))
        }
        object::RelocationTarget::Section(section_index) => {
            Ok(Some((Vec::new(), Some(section_index))))
        }
        object::RelocationTarget::Absolute => Ok(Some((Vec::new(), None))),
        _ => Ok(None),
    }
}

struct MachORelocationSymbolPosition {
    name: Vec<u8>,
    symbol_index: object::SymbolIndex,
    section_index: object::SectionIndex,
    section_offset: u64,
}

fn macho_relocation_symbol_position(
    file: &object::File<'_>,
    target: object::RelocationTarget,
) -> Result<Option<MachORelocationSymbolPosition>> {
    let object::RelocationTarget::Symbol(symbol_index) = target else {
        return Ok(None);
    };
    let symbol = file.symbol_by_index(symbol_index)?;
    let Some(section_index) = symbol.section_index() else {
        return Ok(None);
    };
    let section = file.section_by_index(section_index)?;
    let section_offset = symbol
        .address()
        .checked_sub(section.address())
        .context("Mach-O relocation target precedes its section")?;
    Ok(Some(MachORelocationSymbolPosition {
        name: symbol.name_bytes()?.to_vec(),
        symbol_index,
        section_index,
        section_offset,
    }))
}

fn macho_relocation_instruction_shape(
    rel_info: RelocationKindInfo,
    bytes: &[u8; 4],
) -> Option<[u8; 4]> {
    let mut shape = *bytes;
    rel_info.write_to_buffer(0, &mut shape).ok()?;
    Some(shape)
}

fn macho_text_relocation_signature(
    file: &object::File<'_>,
    context: &MachOTextRelocationContext,
) -> Result<(u8, bool, i64, Option<(Vec<u8>, Option<usize>)>)> {
    Ok((
        context.r_type,
        context.r_pcrel,
        context.addend,
        macho_relocation_target_identity(file, context.target)?
            .map(|(name, section)| (name, section.map(|section| section.0))),
    ))
}

fn macho_output_address_for_current_section_offset(
    output_file: &object::File<'_>,
    matched_sections: &[MatchedPatchSection],
    input: &str,
    section_index: object::SectionIndex,
    section_offset: u64,
) -> Option<u64> {
    let section = matched_sections.iter().find(|section| {
        section.current.input == input && section.current.section_index == section_index.0 as u32
    })?;
    let file_offset = section.current.output_offset.checked_add(section_offset)?;
    macho_output_address_for_file_offset(output_file, file_offset)
}

fn unique_macho_target_value(values: impl IntoIterator<Item = u64>) -> Option<u64> {
    let mut values = values.into_iter();
    let value = values.next()?;
    values.all(|candidate| candidate == value).then_some(value)
}

fn macho_output_address_for_current_target(
    output_file: &object::File<'_>,
    matched_sections: &[MatchedPatchSection],
    relocations: &[RelocationRecord],
    resolutions: &[MachOSymbolResolutionRecord],
    target: &RelocationTargetRecord,
) -> Option<u64> {
    if let Some(value) = macho_output_address_for_current_section_offset(
        output_file,
        matched_sections,
        target.input.as_str(),
        object::SectionIndex(target.section_index as usize),
        target.section_offset,
    ) {
        return Some(value);
    }
    unique_macho_target_value(
        relocations
            .iter()
            .filter(|relocation| relocation.target.as_ref() == Some(target))
            .map(|relocation| relocation.target_value)
            .chain(
                resolutions
                    .iter()
                    .filter(|resolution| resolution.target.as_ref() == Some(target))
                    .filter_map(|resolution| resolution.direct_value),
            ),
    )
}

fn macho_symbol_resolution_for_name<'a>(
    resolutions: &'a [MachOSymbolResolutionRecord],
    name: &[u8],
) -> Option<&'a MachOSymbolResolutionRecord> {
    let encoded = hex::encode(name);
    let mut matches = resolutions
        .iter()
        .filter(|resolution| resolution.name.as_str() == encoded);
    let resolution = matches.next()?;
    matches.next().is_none().then_some(resolution)
}

fn macho_symbol_resolution_index_for_name(
    resolutions: &[MachOSymbolResolutionRecord],
    encoded_name: &str,
) -> std::result::Result<Option<usize>, String> {
    let mut matches = resolutions
        .iter()
        .enumerate()
        .filter_map(|(index, resolution)| (resolution.name == encoded_name).then_some(index));
    let index = matches.next();
    if matches.next().is_some() {
        return Err("ambiguous Mach-O symbol resolution catalog entry".to_owned());
    }
    Ok(index)
}

fn macho_text_relocation_target_candidates(
    resolutions: &[MachOSymbolResolutionRecord],
    target_name: Option<&str>,
    r_type: u8,
    target_value: u64,
) -> Vec<u64> {
    let resolution = target_name
        .and_then(|name| hex::decode(name).ok())
        .as_deref()
        .and_then(|name| macho_symbol_resolution_for_name(resolutions, name));
    let mut candidates = vec![if r_type == object::macho::ARM64_RELOC_BRANCH26 {
        resolution
            .and_then(|resolution| resolution.stub_address)
            .unwrap_or(target_value)
    } else {
        target_value
    }];
    if r_type == object::macho::ARM64_RELOC_BRANCH26
        && let Some(resolution) = resolution
    {
        candidates.extend_from_slice(&resolution.thunk_addresses);
    }
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(*candidate));
    candidates
}

fn macho_aarch64_applied_target_addend(
    r_type: u8,
    target_value: u64,
    applied_target_value: u64,
    addend: i64,
) -> i64 {
    if r_type == object::macho::ARM64_RELOC_BRANCH26 && applied_target_value != target_value {
        0
    } else {
        addend
    }
}

fn rematerialized_macho_text_relocation_replay(
    relocations: &[RelocationRecord],
    resolutions: &[MachOSymbolResolutionRecord],
    input: &FileState,
    patch_section: &MatchedPatchSection,
    current_file: &object::File<'_>,
    current_context: &MachOTextRelocationContext,
    matched_sections: &[MatchedPatchSection],
    output_file: &object::File<'_>,
) -> Result<std::result::Result<MachOTextRelocationReplay, String>> {
    if current_context.subtractor.is_some() {
        return Ok(Err(format!(
            "unsupported current Mach-O text relocation in {}",
            display_hex_path(&input.path)
        )));
    }
    let raw_relocation = object::macho::RelocationInfo {
        r_address: 0,
        r_symbolnum: 0,
        r_pcrel: current_context.r_pcrel,
        r_length: 2,
        r_extern: matches!(current_context.target, object::RelocationTarget::Symbol(_)),
        r_type: current_context.r_type,
    };
    let rel_info =
        <crate::macho_aarch64::MachOAArch64 as crate::platform::Arch>::relocation_from_raw(
            raw_relocation,
        )
        .map_err(|error| {
            crate::error!("Failed to decode current Mach-O text relocation: {error:#?}")
        })?;

    let mut target_name = None;
    let mut target_is_global = false;
    let target;
    let mut resolution = None;
    let current_target_value = match current_context.target {
        object::RelocationTarget::Symbol(symbol_index) => {
            let symbol = current_file
                .symbol_by_index(symbol_index)
                .context("Missing current Mach-O text relocation symbol")?;
            let name = symbol.name_bytes()?.to_vec();
            if !name.is_empty() {
                target_name = Some(SharedText::from(hex::encode(&name)));
                target_is_global = symbol.is_global();
                resolution = macho_symbol_resolution_for_name(resolutions, &name);
            }
            if let Some(section_index) = symbol.section_index() {
                let section = current_file.section_by_index(section_index)?;
                let section_offset = symbol
                    .address()
                    .checked_sub(section.address())
                    .context("Mach-O relocation target precedes its section")?;
                let current_target = RelocationTargetRecord {
                    input_file: input.path.clone().into(),
                    input: patch_section.current.input.clone().into(),
                    section_index: section_index.0 as u32,
                    section_offset,
                };
                let Some(value) = macho_output_address_for_current_target(
                    output_file,
                    matched_sections,
                    relocations,
                    resolutions,
                    &current_target,
                ) else {
                    return Ok(Err(format!(
                        "could not resolve current Mach-O text relocation target in {}",
                        display_hex_path(&input.path)
                    )));
                };
                target = Some(current_target);
                value
            } else {
                let Some(value) = resolution.and_then(|resolution| resolution.direct_value) else {
                    return Ok(Err(format!(
                        "missing Mach-O symbol resolution for {} in {}",
                        display_hex_text(&hex::encode(&name)),
                        display_hex_path(&input.path)
                    )));
                };
                target = resolution.and_then(|resolution| resolution.target.clone());
                value
            }
        }
        object::RelocationTarget::Section(section_index) => {
            let current_target = RelocationTargetRecord {
                input_file: input.path.clone().into(),
                input: patch_section.current.input.clone().into(),
                section_index: section_index.0 as u32,
                section_offset: 0,
            };
            let Some(value) = macho_output_address_for_current_target(
                output_file,
                matched_sections,
                relocations,
                resolutions,
                &current_target,
            ) else {
                return Ok(Err(format!(
                    "could not resolve current Mach-O text relocation section in {}",
                    display_hex_path(&input.path)
                )));
            };
            target = Some(current_target);
            value
        }
        _ => {
            return Ok(Err(format!(
                "unsupported current Mach-O text relocation target in {}",
                display_hex_path(&input.path)
            )));
        }
    };

    let current_relocation_target_candidates = macho_text_relocation_target_candidates(
        resolutions,
        target_name.as_deref(),
        current_context.r_type,
        current_target_value,
    );

    let target_symbol_id = relocations
        .iter()
        .find(|relocation| {
            relocation.target_name == target_name
                && (target.is_none() || relocation.target == target)
        })
        .map_or(0, |relocation| relocation.target_symbol_id);
    Ok(Ok(MachOTextRelocationReplay {
        relocation_index: None,
        input_file: input.path.clone().into(),
        input: patch_section.current.input.clone(),
        section_index: patch_section.current.section_index,
        previous_range: None,
        current_range: current_context.range.clone(),
        r_type: current_context.r_type,
        kind: encode_macho_aarch64_relocation_kind(raw_relocation),
        rel_info,
        previous_written_value: None,
        previous_applied_target_value: None,
        current_target_value,
        current_relocation_target_candidates,
        current_target_section_offset: target.as_ref().map_or(0, |target| target.section_offset),
        target_symbol_id,
        target_name,
        target_is_global,
        target,
        addend: current_context.addend,
    }))
}

fn relocation_is_in_rematerialized_macho_text_section(
    relocation: &RelocationRecord,
    rematerialized_sections: &[(SharedText, String, u32, String, u32)],
) -> bool {
    rematerialized_sections.iter().any(
        |(previous_input_file, previous_input, previous_section_index, _, _)| {
            relocation.input_file == *previous_input_file
                && relocation.input == *previous_input
                && relocation.section_index == *previous_section_index
        },
    )
}

fn reconcile_rematerialized_macho_symbol_resolutions(
    resolutions: &mut Vec<MachOSymbolResolutionRecord>,
    relocations: &[RelocationRecord],
    replays: &[MachOTextRelocationReplay],
    rematerialized_sections: &[(SharedText, String, u32, String, u32)],
    input: &FileState,
) -> std::result::Result<(Vec<RelocationTargetSymbolPatch>, bool), String> {
    let mut affected_names = HashSet::<SharedText>::new();
    let mut current_references =
        HashMap::<SharedText, Vec<(u64, Option<RelocationTargetRecord>)>>::new();

    for relocation in relocations {
        let Some(target_name) = relocation.target_name.as_ref() else {
            continue;
        };
        if relocation_is_in_rematerialized_macho_text_section(relocation, rematerialized_sections) {
            affected_names.insert(target_name.clone());
        } else {
            current_references
                .entry(target_name.clone())
                .or_default()
                .push((relocation.target_value, relocation.target.clone()));
        }
    }
    for replay in replays
        .iter()
        .filter(|replay| replay.relocation_index.is_none() && replay.target_is_global)
    {
        let Some(target_name) = replay.target_name.as_ref() else {
            continue;
        };
        affected_names.insert(target_name.clone());
        current_references
            .entry(target_name.clone())
            .or_default()
            .push((replay.current_target_value, replay.target.clone()));
    }

    let mut output_symbols = Vec::new();
    let mut changed = false;
    for target_name in affected_names {
        let references = current_references
            .get(&target_name)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut target_values = references.iter().map(|(value, _)| *value);
        let current_target_value = target_values.next();
        if current_target_value.is_some()
            && !target_values.all(|value| Some(value) == current_target_value)
        {
            return Err(format!(
                "conflicting current Mach-O symbol resolutions in {}",
                display_hex_path(&input.path)
            ));
        }
        let mut targets = references.iter().filter_map(|(_, target)| target.as_ref());
        let current_target = targets.next().cloned();
        if current_target.is_some()
            && !targets.all(|target| Some(target) == current_target.as_ref())
        {
            return Err(format!(
                "conflicting current Mach-O symbol owners in {}",
                display_hex_path(&input.path)
            ));
        }

        let resolution_index =
            macho_symbol_resolution_index_for_name(resolutions, target_name.as_str())?;
        let Some(current_target_value) = current_target_value else {
            let Some(resolution_index) = resolution_index else {
                continue;
            };
            let resolution = &resolutions[resolution_index];
            if resolution.direct_value.unwrap_or_default() != 0
                && resolution.got_address.is_none()
                && resolution.stub_address.is_none()
                && resolution.thunk_addresses.is_empty()
            {
                return Err(format!(
                    "removed Mach-O text relocation target may change symbol liveness in {}",
                    display_hex_path(&input.path)
                ));
            }
            continue;
        };

        if let Some(resolution_index) = resolution_index {
            let resolution = &mut resolutions[resolution_index];
            if resolution.direct_value != Some(current_target_value) {
                output_symbols.push(RelocationTargetSymbolPatch {
                    target_name: target_name.to_string(),
                    previous_target_value: resolution.direct_value.unwrap_or_default(),
                    target_value: current_target_value,
                    allow_missing: true,
                });
                resolution.direct_value = Some(current_target_value);
                if resolution.target.is_none() {
                    resolution.target = current_target;
                }
                changed = true;
            }
        } else {
            let Some(current_target) = current_target else {
                return Err(format!(
                    "missing current Mach-O symbol owner in {}",
                    display_hex_path(&input.path)
                ));
            };
            output_symbols.push(RelocationTargetSymbolPatch {
                target_name: target_name.to_string(),
                previous_target_value: 0,
                target_value: current_target_value,
                allow_missing: true,
            });
            resolutions.push(MachOSymbolResolutionRecord {
                name: target_name,
                direct_value: Some(current_target_value),
                got_address: None,
                stub_address: None,
                thunk_addresses: Vec::new(),
                target: Some(current_target),
            });
            changed = true;
        }
    }
    Ok((output_symbols, changed))
}

fn macho_text_relocation_replays_for_input(
    relocations: &[RelocationRecord],
    resolutions: &mut Vec<MachOSymbolResolutionRecord>,
    input: &FileState,
    matched_sections: &[MatchedPatchSection],
    previous_resolver: &PatchInputResolver<'_>,
    current_resolver: &PatchInputResolver<'_>,
    previous_output: &[u8],
    target_patches: &mut RelocationTargetPatches,
) -> Result<std::result::Result<MachOTextRelocationReplays, String>> {
    let mut replays = Vec::new();
    let mut rematerialized_sections = Vec::new();
    let output_file = object::File::parse(previous_output)
        .context("Failed to parse previous Mach-O output for text relocation replay")?;
    let target_moves = target_patches
        .macho_target_moves
        .iter()
        .map(|target_move| (target_move.relocation_index, *target_move))
        .collect::<HashMap<_, _>>();
    let mut consumed_target_moves = HashSet::new();
    let mut same_object_target_moves = Vec::new();
    let mut target_input_ranges = Vec::new();
    let mut target_output_symbols = Vec::new();
    let mut sections_by_input = HashMap::<&str, Vec<&MatchedPatchSection>>::new();
    for section in matched_sections {
        sections_by_input
            .entry(section.current.input.as_str())
            .or_default()
            .push(section);
    }

    for (current_input_ref, sections) in sections_by_input {
        let Some(current_input_bytes) = current_resolver.resolve(
            input.path.as_str(),
            current_input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(Err(format!(
                "could not resolve current Mach-O text patch input in {}",
                display_hex_path(&input.path)
            )));
        };
        let current_file = object::File::parse(current_input_bytes.bytes)
            .context("Failed to parse current Mach-O text replay input")?;

        for patch_section in sections {
            let Some(current_index) = patch_section_index(&current_file, &patch_section.current)?
            else {
                return Ok(Err(format!(
                    "missing current Mach-O text patch section in {}",
                    display_hex_path(&input.path)
                )));
            };
            let current_section = current_file
                .section_by_index(current_index)
                .context("Missing current Mach-O text replay section")?;
            if current_file.architecture() != object::Architecture::Aarch64
                || !matches!(current_section.flags(), object::SectionFlags::MachO { .. })
                || current_section.name().ok() != Some("__text")
            {
                continue;
            }

            let Some(previous_input_bytes) = previous_resolver.resolve(
                input.path.as_str(),
                patch_section.previous.input.as_str(),
                PatchInputLookup::MatchArchiveMember,
            )?
            else {
                return Ok(Err(format!(
                    "could not resolve previous Mach-O text patch input in {}",
                    display_hex_path(&input.path)
                )));
            };
            let previous_file = object::File::parse(previous_input_bytes.bytes)
                .context("Failed to parse previous Mach-O text replay input")?;
            let Some(previous_index) =
                patch_section_index(&previous_file, &patch_section.previous)?
            else {
                return Ok(Err(format!(
                    "missing previous Mach-O text patch section in {}",
                    display_hex_path(&input.path)
                )));
            };
            let previous_section = previous_file
                .section_by_index(previous_index)
                .context("Missing previous Mach-O text replay section")?;
            let previous_data = previous_section
                .data()
                .context("Failed to read previous Mach-O text replay section data")?;
            let current_data = current_section
                .data()
                .context("Failed to read current Mach-O text replay section data")?;
            let Some(previous_contexts) = macho_aarch64_text_relocation_contexts(
                &previous_file,
                &previous_section,
                previous_data,
            ) else {
                return Ok(Err(format!(
                    "unsupported previous Mach-O text relocations in {}",
                    display_hex_path(&input.path)
                )));
            };
            let Some(current_contexts) = macho_aarch64_text_relocation_contexts(
                &current_file,
                &current_section,
                current_data,
            ) else {
                return Ok(Err(format!(
                    "unsupported current Mach-O text relocations in {}",
                    display_hex_path(&input.path)
                )));
            };

            let mut relocation_indices = relocations
                .iter()
                .enumerate()
                .filter_map(|(index, relocation)| {
                    (relocation.input_file == input.path
                        && relocation.input == patch_section.previous.input
                        && relocation.section_index == patch_section.previous.section_index
                        && decode_macho_aarch64_relocation_kind(relocation.kind).is_some())
                    .then_some(index)
                })
                .collect::<Vec<_>>();
            relocation_indices.sort_by_key(|index| relocations[*index].relocation_offset);
            let has_moved_target = relocation_indices
                .iter()
                .any(|index| target_moves.contains_key(index));
            if previous_contexts.len() != relocation_indices.len() {
                return Ok(Err(format!(
                    "changed Mach-O text relocation count in {}",
                    display_hex_path(&input.path)
                )));
            }
            let previous_signatures = previous_contexts
                .iter()
                .map(|context| macho_text_relocation_signature(&previous_file, context))
                .collect::<Result<Vec<_>>>()?;
            let current_signatures = current_contexts
                .iter()
                .map(|context| macho_text_relocation_signature(&current_file, context))
                .collect::<Result<Vec<_>>>()?;
            let relocation_set_changed = previous_signatures != current_signatures;
            let is_reordering = if previous_signatures.len() == current_signatures.len() {
                let mut previous = previous_signatures.clone();
                let mut current = current_signatures.clone();
                previous.sort_unstable();
                current.sort_unstable();
                previous == current
            } else {
                false
            };
            if relocation_set_changed
                && (previous_signatures.len() != current_signatures.len() || is_reordering)
            {
                for relocation_index in &relocation_indices {
                    if target_moves.contains_key(relocation_index) {
                        consumed_target_moves.insert(*relocation_index);
                    }
                }
                for current_context in &current_contexts {
                    let replay = match rematerialized_macho_text_relocation_replay(
                        relocations,
                        resolutions,
                        input,
                        patch_section,
                        &current_file,
                        current_context,
                        matched_sections,
                        &output_file,
                    )? {
                        Ok(replay) => replay,
                        Err(reason) => return Ok(Err(reason)),
                    };
                    replays.push(replay);
                }
                rematerialized_sections.push((
                    input.path.clone().into(),
                    patch_section.previous.input.clone(),
                    patch_section.previous.section_index,
                    patch_section.current.input.clone(),
                    patch_section.current.section_index,
                ));
                continue;
            }
            if previous_contexts.len() != current_contexts.len() {
                return Ok(Err(format!(
                    "changed Mach-O text relocation count in {}",
                    display_hex_path(&input.path)
                )));
            }

            let replay_start = replays.len();
            let mut section_has_moved_target = has_moved_target;
            for ((previous_context, current_context), relocation_index) in previous_contexts
                .iter()
                .zip(&current_contexts)
                .zip(relocation_indices)
            {
                let relocation = &relocations[relocation_index];
                let Some(raw_relocation) = decode_macho_aarch64_relocation_kind(relocation.kind)
                else {
                    return Ok(Err(format!(
                        "unsupported recorded Mach-O text relocation in {}",
                        display_hex_path(&input.path)
                    )));
                };
                if previous_context.range.start as u64 != relocation.relocation_offset
                    || previous_context.r_type != raw_relocation.r_type
                    || previous_context.r_pcrel != raw_relocation.r_pcrel
                    || current_context.r_type != previous_context.r_type
                    || current_context.r_pcrel != previous_context.r_pcrel
                    || previous_context.addend != relocation.addend
                    || current_context.addend != previous_context.addend
                    || previous_context.subtractor.is_some()
                    || current_context.subtractor.is_some()
                {
                    return Ok(Err(format!(
                        "changed Mach-O text relocation semantics in {}",
                        display_hex_path(&input.path)
                    )));
                }
                let previous_target =
                    macho_relocation_target_identity(&previous_file, previous_context.target)?;
                let current_target =
                    macho_relocation_target_identity(&current_file, current_context.target)?;
                if previous_target.is_none() || previous_target != current_target {
                    return Ok(Err(format!(
                        "changed Mach-O text relocation target in {}",
                        display_hex_path(&input.path)
                    )));
                }

                let same_object_target_move = if let Some(target) =
                    relocation.target.as_ref().filter(|target| {
                        target.input_file == input.path
                            && target.input == patch_section.previous.input
                            && relocation.input == target.input
                    }) {
                    let Some(previous_position) =
                        macho_relocation_symbol_position(&previous_file, previous_context.target)?
                    else {
                        return Ok(Err(format!(
                            "missing previous Mach-O text relocation target in {}",
                            display_hex_path(&input.path)
                        )));
                    };
                    let Some(current_position) =
                        macho_relocation_symbol_position(&current_file, current_context.target)?
                    else {
                        return Ok(Err(format!(
                            "missing current Mach-O text relocation target in {}",
                            display_hex_path(&input.path)
                        )));
                    };
                    let recorded_name = relocation
                        .target_name
                        .as_deref()
                        .map(hex::decode)
                        .transpose()
                        .context("Malformed recorded Mach-O relocation target name")?;
                    if recorded_name.as_deref() != Some(previous_position.name.as_slice())
                        || previous_position.section_index.0 as u32 != target.section_index
                        || previous_position.section_offset != target.section_offset
                    {
                        return Ok(Err(format!(
                            "previous Mach-O text relocation target changed in {}",
                            display_hex_path(&input.path)
                        )));
                    }
                    if let Some(value_range) = macho_symbol_value_field_range(
                        current_input_bytes.bytes,
                        current_position.symbol_index,
                    ) {
                        target_input_ranges.push(
                            current_input_bytes.file_offset + value_range.start
                                ..current_input_bytes.file_offset + value_range.end,
                        );
                    }
                    let delta = i128::from(current_position.section_offset)
                        - i128::from(previous_position.section_offset);
                    let Some(current_target_value) =
                        add_signed_delta_u64(relocation.target_value, delta)
                    else {
                        return Ok(Err(format!(
                            "Mach-O text relocation target value overflowed in {}",
                            display_hex_path(&input.path)
                        )));
                    };
                    let target_move = MachORelocationTargetMove {
                        relocation_index,
                        current_section_offset: current_position.section_offset,
                        current_target_value,
                    };
                    if current_target_value != relocation.target_value
                        || current_position.section_offset != target.section_offset
                    {
                        section_has_moved_target = true;
                        same_object_target_moves.push(target_move);
                        target_output_symbols.push(RelocationTargetSymbolPatch {
                            target_name: relocation
                                .target_name
                                .as_deref()
                                .context("Missing recorded Mach-O relocation target name")?
                                .to_owned(),
                            previous_target_value: relocation.target_value,
                            target_value: current_target_value,
                            allow_missing: true,
                        });
                        Some(target_move)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let rel_info =
                    <crate::macho_aarch64::MachOAArch64 as crate::platform::Arch>::relocation_from_raw(
                        raw_relocation,
                    )
                    .map_err(|error| {
                        crate::error!(
                            "Failed to decode recorded Mach-O text relocation: {error:#?}"
                        )
                    })?;
                if previous_context.range.start == current_context.range.start
                    && previous_context.bytes != current_context.bytes
                {
                    return Ok(Err(format!(
                        "changed Mach-O text relocation bytes in {}",
                        display_hex_path(&input.path)
                    )));
                }
                if macho_relocation_instruction_shape(rel_info, &previous_context.bytes)
                    != macho_relocation_instruction_shape(rel_info, &current_context.bytes)
                {
                    return Ok(Err(format!(
                        "changed Mach-O text relocation instruction in {}",
                        display_hex_path(&input.path)
                    )));
                }

                let Some(previous_written_value) = relocation.written_value else {
                    return Ok(Err(format!(
                        "missing written Mach-O text relocation value in {}",
                        display_hex_path(&input.path)
                    )));
                };
                let target_move = target_moves.get(&relocation_index);
                if target_move.is_some() {
                    consumed_target_moves.insert(relocation_index);
                }
                let target_move = target_move.copied().or(same_object_target_move);
                let current_target_value = target_move
                    .map_or(relocation.target_value, |target_move| {
                        target_move.current_target_value
                    });
                let previous_applied_target_value = relocation
                    .applied_target_value
                    .unwrap_or(relocation.target_value);
                if target_move.is_some() && previous_applied_target_value != relocation.target_value
                {
                    return Ok(Err(format!(
                        "moved Mach-O stub or thunk target in {}",
                        display_hex_path(&input.path)
                    )));
                }

                replays.push(MachOTextRelocationReplay {
                    relocation_index: Some(relocation_index),
                    input_file: input.path.clone().into(),
                    input: patch_section.current.input.clone(),
                    section_index: patch_section.current.section_index,
                    previous_range: Some(previous_context.range.clone()),
                    current_range: current_context.range.clone(),
                    r_type: raw_relocation.r_type,
                    kind: relocation.kind,
                    rel_info,
                    previous_written_value: Some(previous_written_value),
                    previous_applied_target_value: Some(previous_applied_target_value),
                    current_target_value,
                    current_relocation_target_candidates: macho_text_relocation_target_candidates(
                        resolutions,
                        relocation.target_name.as_deref(),
                        raw_relocation.r_type,
                        current_target_value,
                    ),
                    current_target_section_offset: target_move
                        .and_then(|target_move| {
                            relocation
                                .target
                                .as_ref()
                                .map(|_| target_move.current_section_offset)
                        })
                        .or_else(|| {
                            relocation
                                .target
                                .as_ref()
                                .map(|target| target.section_offset)
                        })
                        .unwrap_or_default(),
                    target_symbol_id: relocation.target_symbol_id,
                    target_name: relocation.target_name.clone(),
                    target_is_global: false,
                    target: relocation.target.clone(),
                    addend: relocation.addend,
                });
            }
            if patch_section.previous.data_hash == patch_section.current.data_hash
                && !section_has_moved_target
            {
                replays.truncate(replay_start);
            }
        }
    }

    if consumed_target_moves.len() != target_moves.len() {
        return Ok(Err(format!(
            "missing Mach-O text relocation for moved target in {}",
            display_hex_path(&input.path)
        )));
    }
    target_patches
        .macho_target_moves
        .extend(same_object_target_moves);
    target_patches.input_ranges.extend(target_input_ranges);
    dedup_ranges(&mut target_patches.input_ranges);
    target_patches.output_symbols.extend(target_output_symbols);
    let (resolution_output_symbols, symbol_resolutions_changed) =
        match reconcile_rematerialized_macho_symbol_resolutions(
            resolutions,
            relocations,
            &replays,
            &rematerialized_sections,
            input,
        ) {
            Ok(updates) => updates,
            Err(reason) => return Ok(Err(reason)),
        };
    target_patches
        .output_symbols
        .extend(resolution_output_symbols);
    Ok(Ok(MachOTextRelocationReplays {
        replays,
        rematerialized_sections,
        symbol_resolutions_changed,
    }))
}

fn apply_macho_text_relocation_replays(
    patches: &mut [ResolvedSectionPatch],
    replays: &MachOTextRelocationReplays,
    previous_output: &[u8],
    relocations: &mut Vec<RelocationRecord>,
) -> std::result::Result<(), String> {
    let output_file = object::File::parse(previous_output)
        .map_err(|error| format!("failed to parse previous Mach-O output: {error}"))?;
    let mut rematerialized_records = Vec::new();
    for replay in &replays.replays {
        let Some(patch) = patches.iter_mut().find(|patch| {
            patch.section.input == replay.input
                && patch.section.section_index == replay.section_index
        }) else {
            return Err("missing changed Mach-O text section for relocation replay".to_owned());
        };
        let current_output_offset = patch
            .section
            .output_offset
            .checked_add(replay.current_range.start as u64)
            .ok_or_else(|| "current Mach-O text relocation offset overflowed".to_owned())?;
        let current_place =
            macho_output_address_for_file_offset(&output_file, current_output_offset)
                .ok_or_else(|| "current Mach-O text relocation has no output address".to_owned())?;
        if let (
            Some(previous_range),
            Some(previous_written_value),
            Some(previous_applied_target_value),
        ) = (
            replay.previous_range.as_ref(),
            replay.previous_written_value,
            replay.previous_applied_target_value,
        ) {
            let previous_start = patch
                .section
                .output_offset
                .checked_add(previous_range.start as u64)
                .and_then(|offset| usize::try_from(offset).ok())
                .ok_or_else(|| "previous Mach-O text relocation range overflowed".to_owned())?;
            let previous_end = previous_start
                .checked_add(previous_range.len())
                .ok_or_else(|| "previous Mach-O text relocation range overflowed".to_owned())?;
            let previous_bytes = previous_output
                .get(previous_start..previous_end)
                .ok_or_else(|| {
                    "previous Mach-O text relocation range is out of bounds".to_owned()
                })?;
            let mut replayed_previous = previous_bytes.to_vec();
            replay
                .rel_info
                .write_to_buffer(previous_written_value, &mut replayed_previous)
                .map_err(|error| {
                    format!("failed to validate previous Mach-O text relocation: {error:#}")
                })?;
            if replayed_previous != previous_bytes {
                return Err("previous Mach-O text relocation encoding changed".to_owned());
            }
            let previous_place =
                macho_output_address_for_file_offset(&output_file, previous_start as u64)
                    .ok_or_else(|| {
                        "previous Mach-O text relocation has no output address".to_owned()
                    })?;
            if macho_aarch64_relocated_value(
                replay.r_type,
                previous_applied_target_value,
                macho_aarch64_applied_target_addend(
                    replay.r_type,
                    replay.current_target_value,
                    previous_applied_target_value,
                    replay.addend,
                ),
                previous_place,
            ) != Some(previous_written_value)
            {
                return Err("previous Mach-O text relocation value changed".to_owned());
            }
        }
        let current_bytes = patch
            .patch
            .data
            .get_mut(replay.current_range.clone())
            .ok_or_else(|| "current Mach-O text relocation range is out of bounds".to_owned())?;
        let mut selected = None;
        for target_value in &replay.current_relocation_target_candidates {
            let Some(written_value) = macho_aarch64_relocated_value(
                replay.r_type,
                *target_value,
                macho_aarch64_applied_target_addend(
                    replay.r_type,
                    replay.current_target_value,
                    *target_value,
                    replay.addend,
                ),
                current_place,
            ) else {
                continue;
            };
            let mut relocated = current_bytes.to_vec();
            if replay
                .rel_info
                .write_to_buffer(written_value, &mut relocated)
                .is_ok()
            {
                selected = Some((*target_value, written_value, relocated));
                break;
            }
        }
        let Some((applied_target_value, written_value, relocated)) = selected else {
            return Err("no reachable target for Mach-O text relocation".to_owned());
        };
        current_bytes.copy_from_slice(&relocated);
        patch
            .patch
            .preserve_ranges
            .retain(|range| range != &replay.current_range);

        if let Some(relocation_index) = replay.relocation_index {
            let relocation = relocations
                .get_mut(relocation_index)
                .ok_or_else(|| "missing recorded Mach-O text relocation".to_owned())?;
            relocation.input = replay.input.clone().into();
            relocation.section_index = replay.section_index;
            relocation.relocation_offset = replay.current_range.start as u64;
            relocation.output_offset = current_output_offset;
            relocation.written_value = Some(written_value);
            relocation.target_value = replay.current_target_value;
            relocation.applied_target_value = Some(applied_target_value);
            if let Some(target) = relocation.target.as_mut() {
                target.section_offset = replay.current_target_section_offset;
            }
        } else {
            rematerialized_records.push(RelocationRecord {
                target_symbol_id: replay.target_symbol_id,
                written_value: Some(written_value),
                target_value: replay.current_target_value,
                applied_target_value: Some(applied_target_value),
                target_name: replay.target_name.clone(),
                target: replay.target.clone(),
                input_file: replay.input_file.clone(),
                input: replay.input.clone().into(),
                section_index: replay.section_index,
                relocation_offset: replay.current_range.start as u64,
                output_offset: current_output_offset,
                size: replay.current_range.len() as u64,
                kind: replay.kind,
                addend: replay.addend,
            });
        }
    }

    if patches.iter().any(|patch| {
        patch.section.section_name.as_deref() == Some("__TEXT,__text")
            && !patch.patch.preserve_ranges.is_empty()
    }) {
        return Err("missing recorded Mach-O text relocation for replay".to_owned());
    }
    if !replays.rematerialized_sections.is_empty() {
        relocations.retain(|relocation| {
            !replays.rematerialized_sections.iter().any(
                |(input_file, previous_input, previous_section_index, _, _)| {
                    relocation.input_file == *input_file
                        && relocation.input == *previous_input
                        && relocation.section_index == *previous_section_index
                        && decode_macho_aarch64_relocation_kind(relocation.kind).is_some()
                },
            )
        });
        relocations.extend(rematerialized_records);
        relocations.sort_unstable();
    }
    Ok(())
}

fn macho_output_address_for_file_offset(file: &object::File<'_>, file_offset: u64) -> Option<u64> {
    for section in file.sections() {
        let Some((section_file_offset, section_size)) = section.file_range() else {
            continue;
        };
        let section_end = section_file_offset.checked_add(section_size)?;
        if file_offset >= section_file_offset && file_offset < section_end {
            return section
                .address()
                .checked_add(file_offset.checked_sub(section_file_offset)?);
        }
    }
    None
}

fn section_size_allows_direct_patching(
    section_name: Option<&[u8]>,
    previous_input_size: u64,
    current_input_size: usize,
) -> bool {
    (section_name != Some(b"__cstring".as_slice()) && section_name != Some(b"__text".as_slice()))
        || u64::try_from(current_input_size).is_ok_and(|size| size == previous_input_size)
}

fn cstring_literal_boundaries_are_stable(previous: &[u8], current: &[u8]) -> bool {
    previous.len() == current.len()
        && previous
            .iter()
            .zip(current)
            .all(|(previous, current)| (*previous == 0) == (*current == 0))
}

fn cstring_nul_boundaries_hash(bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sld-cstring-nul-boundaries-v1");
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    for (offset, byte) in bytes.iter().enumerate() {
        if *byte == 0 {
            hasher.update(&(offset as u64).to_le_bytes());
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn cstring_nul_boundaries_hash_for_section(
    section_name: Option<&str>,
    bytes: &[u8],
) -> Option<String> {
    (section_name == Some("__cstring")).then(|| cstring_nul_boundaries_hash(bytes))
}

fn relocation_preserve_ranges<'data>(
    file: &object::File<'data>,
    section: &impl object::ObjectSection<'data>,
    section_data: &[u8],
    dynamic_relocation_offsets: Option<&HashSet<u64>>,
    required_relocation_offsets: Option<&HashSet<u64>>,
) -> Option<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::<std::ops::Range<usize>>::new();
    for (offset, relocation) in section.relocations() {
        if relocation.kind() == object::RelocationKind::None {
            continue;
        }
        if required_relocation_offsets.is_some_and(|offsets| !offsets.contains(&offset)) {
            return None;
        }
        let is_recorded_dynamic_relocation =
            dynamic_relocation_offsets.is_some_and(|offsets| offsets.contains(&offset));
        let start = usize::try_from(offset).ok()?;
        let len = usize::from(relocation.size() / 8);
        let end = start.checked_add(len)?;
        if end > section_data.len() {
            return None;
        }
        let generic_explicit_relocation = !relocation.has_implicit_addend()
            && relocation.encoding() == object::RelocationEncoding::Generic
            && relocation.size() != 0
            && relocation.size() % 8 == 0;
        // Mach-O absolute pointers encode their addend in-place. Preserve only
        // the zero-addend form targeting an unchanged input symbol.
        let zero_addend_macho_external_absolute =
            matches!(section.flags(), object::SectionFlags::MachO { .. })
                && relocation.has_implicit_addend()
                && relocation.kind() == object::RelocationKind::Absolute
                && relocation.size() != 0
                && relocation.size() % 8 == 0
                && section_data[start..end].iter().all(|byte| *byte == 0)
                && match relocation.target() {
                    object::RelocationTarget::Symbol(index) => file
                        .symbol_by_index(index)
                        .ok()
                        .is_some_and(|symbol| symbol.is_undefined()),
                    _ => false,
                };
        // These x86-64 ELF operands are contiguous four-byte fields. Standard GOTPCREL is
        // guarded on updates because one instruction form can be rewritten by relaxation.
        let preservable_x86_64_elf_pc_relative =
            matches!(section.flags(), object::SectionFlags::Elf { .. })
                && file.architecture() == object::Architecture::X86_64
                && relocation.size() == 32
                && match (relocation.kind(), relocation.encoding()) {
                    (object::RelocationKind::Relative, object::RelocationEncoding::Generic) => true,
                    (
                        object::RelocationKind::PltRelative,
                        object::RelocationEncoding::Generic | object::RelocationEncoding::X86Branch,
                    ) => true,
                    (object::RelocationKind::GotRelative, object::RelocationEncoding::Generic) => {
                        true
                    }
                    _ => false,
                };
        let explicit_relocation_field = generic_explicit_relocation
            || (!relocation.has_implicit_addend() && preservable_x86_64_elf_pc_relative);
        if !(explicit_relocation_field || zero_addend_macho_external_absolute)
            || (!is_recorded_dynamic_relocation
                && relocation.kind() != object::RelocationKind::Absolute
                && !preservable_x86_64_elf_pc_relative)
        {
            return None;
        }
        ranges.push(start..end);
    }
    ranges.sort_by_key(|range| range.start);
    let mut previous_end = 0;
    for range in &ranges {
        if range.start < previous_end {
            return None;
        }
        previous_end = range.end;
    }
    Some(ranges)
}

fn patchable_bytes_match(
    output: &[u8],
    input: &[u8],
    preserve_ranges: &[std::ops::Range<usize>],
) -> bool {
    if output.len() != input.len() {
        return false;
    }
    let mut position = 0;
    for range in preserve_ranges {
        if output[position..range.start] != input[position..range.start] {
            return false;
        }
        position = range.end;
    }
    output[position..] == input[position..]
}

fn patch_section_name_for_matching<'data>(
    section: &impl object::ObjectSection<'data>,
) -> Option<String> {
    let name = section.name().ok()?;
    if !section_name_is_stable_for_patch_matching(name) {
        return None;
    }
    if matches!(section.flags(), object::SectionFlags::MachO { .. }) {
        let segment = section.segment_name().ok().flatten()?;
        return Some(format!("{segment},{name}"));
    }
    Some(name.to_owned())
}

fn section_name_is_stable_for_patch_matching(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..L")
        && !name.contains(".L__")
        && !name.contains("__unnamed_")
}

fn patch_input_bytes<'data>(
    bytes: &'data [u8],
    input_file_path: &str,
    input_ref: &str,
) -> Result<Option<PatchInputBytes<'data>>> {
    patch_input_bytes_with_lookup(
        bytes,
        input_file_path,
        input_ref,
        PatchInputLookup::MatchArchiveMember,
    )
}

fn patch_input_bytes_with_lookup<'data>(
    bytes: &'data [u8],
    input_file_path: &str,
    input_ref: &str,
    lookup: PatchInputLookup,
) -> Result<Option<PatchInputBytes<'data>>> {
    let Some(parsed) = parse_patch_input_ref(input_file_path, input_ref)? else {
        return Ok(Some(PatchInputBytes {
            bytes,
            file_offset: 0,
        }));
    };
    if parsed.range.is_empty() {
        return Ok(None);
    }

    if matches!(lookup, PatchInputLookup::CurrentRecordedRange) {
        let Some(input_bytes) = bytes.get(parsed.range.clone()) else {
            return Ok(None);
        };
        return Ok(Some(PatchInputBytes {
            bytes: input_bytes,
            file_offset: parsed.range.start,
        }));
    }

    match patch_archive_member_bytes(bytes, &parsed.identifier)? {
        ArchiveMemberMatch::Unique(member) => return Ok(Some(member)),
        ArchiveMemberMatch::Ambiguous => return Ok(None),
        ArchiveMemberMatch::Unavailable => {}
    }

    let Some(input_bytes) = bytes.get(parsed.range.clone()) else {
        return Ok(None);
    };
    Ok(Some(PatchInputBytes {
        bytes: input_bytes,
        file_offset: parsed.range.start,
    }))
}

#[cfg(test)]
fn patch_input_range(
    input_file_path: &str,
    input_ref: &str,
) -> Result<Option<std::ops::Range<usize>>> {
    Ok(parse_patch_input_ref(input_file_path, input_ref)?.map(|input_ref| input_ref.range))
}

fn parse_patch_input_ref(
    input_file_path: &str,
    input_ref: &str,
) -> Result<Option<ParsedPatchInputRef>> {
    let input_file_path_bytes =
        hex::decode(input_file_path).context("Malformed incremental input file path")?;
    let input_ref_bytes = hex::decode(input_ref).context("Malformed incremental input ref")?;
    if input_ref_bytes == input_file_path_bytes {
        return Ok(None);
    }

    let Some(rest) = input_ref_bytes
        .strip_prefix(input_file_path_bytes.as_slice())
        .and_then(|rest| rest.strip_prefix(&[0]))
    else {
        return Ok(Some(ParsedPatchInputRef {
            identifier: Vec::new(),
            range: 0..0,
        }));
    };
    let Some(separator) = rest.iter().position(|byte| *byte == 0) else {
        return Ok(Some(ParsedPatchInputRef {
            identifier: Vec::new(),
            range: 0..0,
        }));
    };
    let identifier = rest[..separator].to_vec();
    let range_bytes = &rest[separator + 1..];
    let range =
        std::str::from_utf8(range_bytes).context("Malformed incremental archive member range")?;
    let Some((start, end)) = range.split_once(':') else {
        return Ok(Some(ParsedPatchInputRef {
            identifier: Vec::new(),
            range: 0..0,
        }));
    };
    let start = start
        .parse()
        .context("Invalid incremental archive member start offset")?;
    let end = end
        .parse()
        .context("Invalid incremental archive member end offset")?;
    if start > end {
        return Ok(Some(ParsedPatchInputRef {
            identifier: Vec::new(),
            range: 0..0,
        }));
    }
    Ok(Some(ParsedPatchInputRef {
        identifier,
        range: start..end,
    }))
}

fn patch_archive_member_bytes<'data>(
    bytes: &'data [u8],
    identifier: &[u8],
) -> Result<ArchiveMemberMatch<'data>> {
    if identifier.is_empty() {
        return Ok(ArchiveMemberMatch::Unavailable);
    }
    let patch_identifier = archive_member_patch_identifier(identifier);
    let Ok(archive) = ArchiveIterator::from_archive_bytes(bytes) else {
        return Ok(ArchiveMemberMatch::Unavailable);
    };
    let mut matched = None;
    for entry in archive {
        match entry? {
            ArchiveEntry::Regular(content)
                if archive_member_patch_identifier(content.ident.as_slice())
                    == patch_identifier =>
            {
                let member = PatchInputBytes {
                    bytes: content.entry_data,
                    file_offset: content.data_offset,
                };
                if matched.replace(member).is_some() {
                    return Ok(ArchiveMemberMatch::Ambiguous);
                }
            }
            ArchiveEntry::Regular(_) | ArchiveEntry::Thin(_) => {}
        }
    }
    Ok(matched.map_or(ArchiveMemberMatch::Unavailable, ArchiveMemberMatch::Unique))
}

fn archive_members_match_snapshot(
    state_dir: &Path,
    previous_input: &FileState,
    previous_bytes: Option<&[u8]>,
    current_bytes: &[u8],
    normalize_rust_archive_patch_inputs: bool,
) -> Result<bool> {
    let current_members = if normalize_rust_archive_patch_inputs {
        archive_member_patch_identifiers(current_bytes)?
    } else {
        archive_member_identifiers(current_bytes)?
    };
    if current_members.is_none() && !stored_patch_state_references_archive_member(previous_input) {
        return Ok(true);
    }
    let Some(previous_bytes) = previous_bytes else {
        return Ok(false);
    };
    let previous_members = if normalize_rust_archive_patch_inputs {
        archive_member_patch_identifiers(previous_bytes)?
    } else {
        archive_member_identifiers(previous_bytes)?
    };
    let members_match = previous_members == current_members;
    if !members_match {
        if let (Some(previous), Some(current)) = (&previous_members, &current_members) {
            append_log(
                state_dir,
                &format!(
                    "archive member identifier sequence differed: previous={} current={}",
                    previous.len(),
                    current.len(),
                ),
            )?;
        }
    }
    Ok(members_match)
}

fn archive_member_set_proof_matches_current(
    previous_input: &FileState,
    previous_patch: &PreviousPatchState,
    current_proof: Option<&ArchiveMemberSetProof>,
    normalize_rust_archive_patch_inputs: bool,
) -> Option<bool> {
    let previous_proof = previous_input
        .patch
        .as_ref()
        .and_then(|patch| patch.archive_member_set_proof.as_ref());
    let Some(current_proof) = current_proof else {
        return Some(
            previous_proof.is_none()
                && !patch_state_references_archive_member(previous_input, previous_patch),
        );
    };
    let previous_proof = previous_proof?;
    let hashes_match = if normalize_rust_archive_patch_inputs {
        previous_proof.normalized_ordered_hash == current_proof.normalized_ordered_hash
    } else {
        previous_proof.raw_ordered_hash == current_proof.raw_ordered_hash
    };
    Some(previous_proof.member_count == current_proof.member_count && hashes_match)
}

fn patch_state_references_archive_member(
    previous_input: &FileState,
    previous_patch: &PreviousPatchState,
) -> bool {
    previous_patch
        .sections
        .iter()
        .any(|section| section.input != previous_input.path)
}

fn stored_patch_state_references_archive_member(previous_input: &FileState) -> bool {
    previous_input.patch.as_ref().is_some_and(|patch| {
        patch
            .sections
            .iter()
            .any(|section| section.input != previous_input.path)
    })
}

fn archive_member_identifiers(bytes: &[u8]) -> Result<Option<Vec<Vec<u8>>>> {
    let Ok(archive) = ArchiveIterator::from_archive_bytes(bytes) else {
        return Ok(None);
    };
    let mut identifiers = Vec::new();
    for entry in archive {
        match entry? {
            ArchiveEntry::Regular(content) => identifiers.push(content.ident.as_slice().to_vec()),
            ArchiveEntry::Thin(entry) => identifiers.push(entry.ident.as_slice().to_vec()),
        }
    }
    Ok(Some(identifiers))
}

fn archive_member_patch_identifiers(bytes: &[u8]) -> Result<Option<Vec<Vec<u8>>>> {
    Ok(archive_member_identifiers(bytes)?.map(|members| {
        members
            .iter()
            .map(|member| archive_member_patch_identifier(member))
            .collect()
    }))
}

fn archive_member_set_proof(bytes: &[u8]) -> Result<Option<ArchiveMemberSetProof>> {
    let Some(raw_members) = archive_member_identifiers(bytes)? else {
        return Ok(None);
    };
    let normalized_members = raw_members
        .iter()
        .map(|member| archive_member_patch_identifier(member))
        .collect::<Vec<_>>();
    Ok(Some(ArchiveMemberSetProof {
        raw_ordered_hash: archive_member_identifier_list_hash(
            b"sld-archive-members-raw-ordered-v1",
            &raw_members,
        ),
        normalized_ordered_hash: archive_member_identifier_list_hash(
            b"sld-archive-members-normalized-ordered-v2",
            &normalized_members,
        ),
        member_count: raw_members.len(),
        rustc_link_content_digest: rustc_rlib_link_content_digest(bytes),
    }))
}

fn rustc_rlib_link_content_digest_matches_previous(input: &FileState, bytes: &[u8]) -> bool {
    let Some(current_digest) = rustc_rlib_link_content_digest(bytes) else {
        return false;
    };
    rustc_rlib_link_content_digest_value_matches_previous(input, &current_digest)
}

fn rustc_rlib_link_content_digest_matches_previous_path(
    input: &FileState,
    path: &Path,
) -> Option<FileContentState> {
    #[cfg(not(unix))]
    {
        let _ = (input, path);
        None
    }
    #[cfg(unix)]
    {
        let before = FileIdentity::from_path(path).ok().flatten()?;
        let file = OpenOptions::new().read(true).open(path).ok()?;
        let opened = FileIdentity::from_metadata(&file.metadata().ok()?);
        if before != opened {
            return None;
        }
        let data = object::read::ReadCache::new(file);
        let archive = object::read::archive::ArchiveFile::parse(&data).ok()?;
        if archive.is_thin() {
            return None;
        }
        for member in archive.members() {
            let member = member.ok()?;
            if member.name() != RUSTC_RLIB_LINK_METADATA_MEMBER {
                continue;
            }
            let (offset, size) = member.file_range();
            if size > RUSTC_RLIB_LINK_METADATA_WRAPPER_MAX_LEN {
                return None;
            }
            let current_digest =
                rustc_rlib_link_content_digest_from_wrapper(data.range(offset, size))?;
            if !rustc_rlib_link_content_digest_value_matches_previous(input, &current_digest) {
                return None;
            }
            let after = FileIdentity::from_path(path).ok().flatten()?;
            if opened != after {
                return None;
            }
            return Some(FileContentState {
                len: after.len,
                hash: String::new(),
                identity: Some(after),
            });
        }
        None
    }
}

fn rustc_rlib_link_content_digest_value_matches_previous(
    input: &FileState,
    current_digest: &str,
) -> bool {
    previous_rustc_rlib_link_content_digest(input) == Some(current_digest)
}

fn previous_rustc_rlib_link_content_digest(input: &FileState) -> Option<&str> {
    input
        .patch
        .as_ref()
        .and_then(|patch| patch.archive_member_set_proof.as_ref())
        .and_then(|proof| proof.rustc_link_content_digest.as_deref())
}

fn rustc_rlib_link_content_digest(bytes: &[u8]) -> Option<String> {
    let archive = ArchiveIterator::from_archive_bytes(bytes).ok()?;
    for entry in archive {
        let ArchiveEntry::Regular(content) = entry.ok()? else {
            return None;
        };
        if content.ident.as_slice() != RUSTC_RLIB_LINK_METADATA_MEMBER {
            continue;
        }
        return rustc_rlib_link_content_digest_from_wrapper(content.entry_data);
    }
    None
}

fn rustc_rlib_link_content_digest_from_wrapper<'data, R: object::read::ReadRef<'data>>(
    data: R,
) -> Option<String> {
    let file = object::File::parse(data).ok()?;
    let section = file.section_by_name(RUSTC_RLIB_LINK_METADATA_SECTION)?;
    decode_rustc_rlib_link_content_digest(section.data().ok()?)
}

fn decode_rustc_rlib_link_content_digest(metadata: &[u8]) -> Option<String> {
    let metadata = metadata.strip_suffix(RUSTC_SERIALIZED_METADATA_END)?;
    let suffix_len = RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX.len() + blake3::OUT_LEN * 2;
    let suffix = metadata.get(metadata.len().checked_sub(suffix_len)?..)?;
    let digest = suffix.strip_prefix(RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX)?;
    let digest = std::str::from_utf8(digest).ok()?;
    is_blake3_hex_digest(digest).then(|| digest.to_owned())
}

fn rustc_rlib_raw_object_digests_from_wrapper<'data, R: object::read::ReadRef<'data>>(
    data: R,
) -> Option<Vec<RustcRlibRawObjectDigest>> {
    let file = object::File::parse(data).ok()?;
    let section = file.section_by_name(RUSTC_RLIB_LINK_METADATA_SECTION)?;
    decode_rustc_rlib_raw_object_digests(section.data().ok()?)
}

fn decode_rustc_rlib_raw_object_digests(metadata: &[u8]) -> Option<Vec<RustcRlibRawObjectDigest>> {
    let mut metadata = metadata.strip_suffix(RUSTC_SERIALIZED_METADATA_END)?;
    let link_content_suffix_len = RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX.len() + blake3::OUT_LEN * 2;
    if let Some(suffix_start) = metadata.len().checked_sub(link_content_suffix_len)
        && let Some(suffix) = metadata.get(suffix_start..)
        && let Some(digest) = suffix.strip_prefix(RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX)
        && let Ok(digest) = std::str::from_utf8(digest)
        && is_blake3_hex_digest(digest)
    {
        metadata = &metadata[..suffix_start];
    }

    let payload_len = usize::try_from(read_u64_le(
        metadata.get(metadata.len().checked_sub(8)?..)?,
    )?)
    .ok()?;
    let payload_end = metadata.len().checked_sub(8)?;
    let payload_start = payload_end.checked_sub(payload_len)?;
    let prefix_start = payload_start.checked_sub(RUSTC_RLIB_RAW_OBJECT_DIGESTS_PREFIX.len())?;
    if metadata.get(prefix_start..payload_start)? != RUSTC_RLIB_RAW_OBJECT_DIGESTS_PREFIX {
        return None;
    }
    let mut payload = metadata.get(payload_start..payload_end)?;
    let object_count = usize::try_from(take_u64_le(&mut payload)?).ok()?;
    if object_count > payload.len() / (8 + blake3::OUT_LEN * 2) {
        return None;
    }
    let mut objects = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        let identifier_len = usize::try_from(take_u64_le(&mut payload)?).ok()?;
        let identifier = take_bytes(&mut payload, identifier_len)?.to_vec();
        if identifier.is_empty() {
            return None;
        }
        let digest = std::str::from_utf8(take_bytes(&mut payload, blake3::OUT_LEN * 2)?).ok()?;
        if !is_blake3_hex_digest(digest) {
            return None;
        }
        objects.push(RustcRlibRawObjectDigest {
            identifier,
            digest: digest.to_owned(),
        });
    }
    payload.is_empty().then_some(objects)
}

fn take_u64_le(bytes: &mut &[u8]) -> Option<u64> {
    read_u64_le(take_bytes(bytes, 8)?)
}

fn take_bytes<'a>(bytes: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    let (taken, rest) = bytes.split_at_checked(len)?;
    *bytes = rest;
    Some(taken)
}

fn archive_member_identifier_list_hash(domain: &[u8], members: &[Vec<u8>]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&(members.len() as u64).to_le_bytes());
    for member in members {
        hasher.update(&(member.len() as u64).to_le_bytes());
        hasher.update(member);
    }
    hasher.finalize().to_hex().to_string()
}

fn archive_member_patch_identifier(identifier: &[u8]) -> Vec<u8> {
    let Ok(filename) = std::str::from_utf8(identifier) else {
        return identifier.to_vec();
    };
    let parts = filename.split('.').collect::<Vec<_>>();
    let [crate_name, codegen_unit, invocation, "rcgu", "o"] = parts.as_slice() else {
        return identifier.to_vec();
    };
    if crate_name.is_empty() || codegen_unit.is_empty() || invocation.is_empty() {
        return identifier.to_vec();
    }
    format!("{crate_name}.{codegen_unit}.rcgu.o").into_bytes()
}

#[cfg(test)]
fn patch_fingerprint(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
) -> Result<Option<String>> {
    patch_fingerprint_with_extra_ranges(bytes, input_file_path, sections, std::iter::empty())
}

#[cfg(test)]
fn patch_fingerprint_with_extra_ranges(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    extra_ranges: impl IntoIterator<Item = std::ops::Range<usize>>,
) -> Result<Option<String>> {
    patch_fingerprint_with_extra_ranges_mode(bytes, input_file_path, sections, extra_ranges, true)
}

fn patch_fingerprint_with_extra_ranges_mode(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    extra_ranges: impl IntoIterator<Item = std::ops::Range<usize>>,
    normalize_rust_archive_patch_inputs: bool,
) -> Result<Option<String>> {
    let resolver = PatchInputResolver::new(bytes, normalize_rust_archive_patch_inputs)?;
    patch_fingerprint_with_resolver(
        bytes,
        input_file_path,
        sections,
        extra_ranges,
        &resolver,
        PatchInputLookup::MatchArchiveMember,
        normalize_rust_archive_patch_inputs,
    )
}

fn patch_fingerprint_for_current_records_with_extra_ranges_and_archive_member_patch_fingerprints(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    extra_ranges: impl IntoIterator<Item = std::ops::Range<usize>>,
    normalize_rust_archive_patch_inputs: bool,
) -> Result<Option<(String, Option<Vec<ArchiveMemberPatchFingerprint>>)>> {
    let Some(ranges) = patch_ranges_with_lookup(
        bytes,
        input_file_path,
        sections,
        PatchInputLookup::CurrentRecordedRange,
    )?
    else {
        return Ok(None);
    };
    patch_fingerprint_from_ranges_with_archive_member_patch_fingerprints(
        bytes,
        ranges,
        extra_ranges,
        normalize_rust_archive_patch_inputs,
        None,
    )
}

fn patch_fingerprint_with_resolver(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    extra_ranges: impl IntoIterator<Item = std::ops::Range<usize>>,
    resolver: &PatchInputResolver<'_>,
    lookup: PatchInputLookup,
    normalize_rust_archive_patch_inputs: bool,
) -> Result<Option<String>> {
    let Some(ranges) =
        patch_ranges_with_resolver(bytes, input_file_path, sections, resolver, lookup)?
    else {
        return Ok(None);
    };
    patch_fingerprint_from_ranges(
        bytes,
        ranges,
        extra_ranges,
        normalize_rust_archive_patch_inputs,
    )
}

fn patch_fingerprint_with_resolver_and_archive_member_patch_fingerprints(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    extra_ranges: impl IntoIterator<Item = std::ops::Range<usize>>,
    resolver: &PatchInputResolver<'_>,
    lookup: PatchInputLookup,
    normalize_rust_archive_patch_inputs: bool,
    previous_archive_member_patch_fingerprints: Option<&[ArchiveMemberPatchFingerprint]>,
) -> Result<Option<(String, Option<Vec<ArchiveMemberPatchFingerprint>>)>> {
    let Some(ranges) =
        patch_ranges_with_resolver(bytes, input_file_path, sections, resolver, lookup)?
    else {
        return Ok(None);
    };
    patch_fingerprint_from_ranges_with_archive_member_patch_fingerprints(
        bytes,
        ranges,
        extra_ranges,
        normalize_rust_archive_patch_inputs,
        previous_archive_member_patch_fingerprints,
    )
}

fn patch_fingerprint_from_ranges(
    bytes: &[u8],
    ranges: Vec<std::ops::Range<usize>>,
    extra_ranges: impl IntoIterator<Item = std::ops::Range<usize>>,
    normalize_rust_archive_patch_inputs: bool,
) -> Result<Option<String>> {
    Ok(
        patch_fingerprint_from_ranges_with_archive_member_patch_fingerprints(
            bytes,
            ranges,
            extra_ranges,
            normalize_rust_archive_patch_inputs,
            None,
        )?
        .map(|(fingerprint, _)| fingerprint),
    )
}

fn patch_fingerprint_from_ranges_with_archive_member_patch_fingerprints(
    bytes: &[u8],
    mut ranges: Vec<std::ops::Range<usize>>,
    extra_ranges: impl IntoIterator<Item = std::ops::Range<usize>>,
    normalize_rust_archive_patch_inputs: bool,
    previous_archive_member_patch_fingerprints: Option<&[ArchiveMemberPatchFingerprint]>,
) -> Result<Option<(String, Option<Vec<ArchiveMemberPatchFingerprint>>)>> {
    ranges.extend(extra_ranges);
    dedup_ranges(&mut ranges);
    let Some(ranges) = normalize_patch_ranges(ranges, bytes.len()) else {
        return Ok(None);
    };

    if normalize_rust_archive_patch_inputs
        && let Some(fingerprint) = archive_patch_fingerprint_with_previous(
            bytes,
            &ranges,
            previous_archive_member_patch_fingerprints,
            rustc_work_product_provenance_enabled(),
        )?
    {
        return Ok(Some(fingerprint));
    }

    if let Some(fingerprint) = macho_object_patch_fingerprint(bytes, 0, &ranges) {
        return Ok(Some((fingerprint.to_hex().to_string(), None)));
    }

    let mut hasher = blake3::Hasher::new();
    let mut position = 0;
    for range in &ranges {
        hasher.update(&bytes[position..range.start]);
        update_hash_with_zeroes(&mut hasher, range.end - range.start);
        position = range.end;
    }
    hasher.update(&bytes[position..]);
    Ok(Some((hasher.finalize().to_hex().to_string(), None)))
}

#[cfg(test)]
fn archive_patch_fingerprint(
    bytes: &[u8],
    ranges: &[std::ops::Range<usize>],
) -> Result<Option<String>> {
    Ok(
        archive_patch_fingerprint_with_previous(bytes, ranges, None, false)?
            .map(|(fingerprint, _)| fingerprint),
    )
}

fn archive_patch_fingerprint_with_previous(
    bytes: &[u8],
    ranges: &[std::ops::Range<usize>],
    previous_archive_member_patch_fingerprints: Option<&[ArchiveMemberPatchFingerprint]>,
    trust_rustc_object_digests: bool,
) -> Result<Option<(String, Option<Vec<ArchiveMemberPatchFingerprint>>)>> {
    let Ok(archive) = ArchiveIterator::from_archive_bytes(bytes) else {
        return Ok(None);
    };
    let mut members = Vec::new();
    let mut is_rust_archive = false;
    let mut raw_object_digests = None;
    let mut raw_object_digest_metadata_count = 0;
    for entry in archive {
        match entry? {
            ArchiveEntry::Regular(content) => {
                let raw_identifier = content.ident.as_slice();
                let identifier = archive_member_patch_identifier(content.ident.as_slice());
                is_rust_archive |= identifier != content.ident.as_slice();
                if trust_rustc_object_digests && raw_identifier == RUSTC_RLIB_LINK_METADATA_MEMBER {
                    raw_object_digest_metadata_count += 1;
                    raw_object_digests =
                        rustc_rlib_raw_object_digests_from_wrapper(content.entry_data);
                }
                members.push((
                    raw_identifier.to_vec(),
                    identifier,
                    content.entry_data,
                    content.data_offset,
                ));
            }
            ArchiveEntry::Thin(_) => return Ok(None),
        }
    }
    let rustc_object_digests = (raw_object_digest_metadata_count == 1)
        .then_some(raw_object_digests)
        .flatten()
        .and_then(|objects| {
            let mut digests = HashMap::with_capacity(objects.len());
            for object in objects {
                let identifier = archive_member_patch_identifier(&object.identifier);
                if digests.insert(identifier, object.digest).is_some() {
                    return None;
                }
            }
            Some(digests)
        });
    let previous_archive_member_patch_fingerprints = previous_archive_member_patch_fingerprints
        .map(|fingerprints| {
            fingerprints
                .iter()
                .map(|fingerprint| (fingerprint.archive_member_index, fingerprint))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let member_hashes = members
        .par_iter()
        .enumerate()
        .map(
            |(archive_member_index, (_, identifier, data, data_offset))| {
                if identifier.starts_with(b"__.SYMDEF")
                    || (is_rust_archive
                        && matches!(identifier.as_slice(), b"lib.rmeta" | b"lib.rmeta-link"))
                {
                    return None;
                }
                let ranges_hash =
                    archive_member_patch_ranges_hash(*data_offset, data.len(), ranges);
                let rustc_object_digest = rustc_object_digests
                    .as_ref()
                    .and_then(|digests| digests.get(identifier.as_slice()));
                // A raw producer digest cannot replace the output-specific patch
                // fingerprint. It can only prove that a prior constituent remains
                // valid when the member position and masked ranges also match.
                if let Some(rustc_object_digest) = rustc_object_digest
                    && let Some(previous) =
                        previous_archive_member_patch_fingerprints.get(&archive_member_index)
                    && previous.identifier == *identifier
                    && previous.data_len == data.len()
                    && previous.ranges_hash == ranges_hash
                    && previous.rustc_object_digest == *rustc_object_digest
                    && let Some(fingerprint) = parse_blake3_hex_digest(&previous.fingerprint)
                {
                    return Some((fingerprint, Some((*previous).clone())));
                }
                let mut hasher = blake3::Hasher::new();
                let fingerprint = macho_object_patch_fingerprint(data, *data_offset, ranges)
                    .unwrap_or_else(|| {
                        hasher.update(b"sld-archive-member-patch-fingerprint-v1");
                        hasher.update(&(data.len() as u64).to_le_bytes());
                        update_hash_with_ranges(&mut hasher, data, *data_offset, ranges);
                        hasher.finalize()
                    });
                let archive_member_patch_fingerprint =
                    rustc_object_digest.map(|rustc_object_digest| ArchiveMemberPatchFingerprint {
                        archive_member_index,
                        identifier: identifier.clone(),
                        data_len: data.len(),
                        ranges_hash,
                        rustc_object_digest: rustc_object_digest.clone(),
                        fingerprint: fingerprint.to_hex().to_string(),
                    });
                Some((*fingerprint.as_bytes(), archive_member_patch_fingerprint))
            },
        )
        .collect::<Vec<_>>();

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sld-parallel-archive-patch-fingerprint-v2");
    let mut archive_member_patch_fingerprints = Vec::new();
    for ((_, identifier, _, _), member_hash) in members.into_iter().zip(member_hashes) {
        let Some((member_hash, archive_member_patch_fingerprint)) = member_hash else {
            continue;
        };
        hasher.update(&(identifier.len() as u64).to_le_bytes());
        hasher.update(&identifier);
        hasher.update(&member_hash);
        if let Some(archive_member_patch_fingerprint) = archive_member_patch_fingerprint {
            archive_member_patch_fingerprints.push(archive_member_patch_fingerprint);
        }
    }
    Ok(Some((
        format!(
            "{PARALLEL_ARCHIVE_PATCH_FINGERPRINT_PREFIX}{}",
            hasher.finalize().to_hex()
        ),
        (!archive_member_patch_fingerprints.is_empty())
            .then_some(archive_member_patch_fingerprints),
    )))
}

fn macho_object_patch_fingerprint(
    bytes: &[u8],
    file_offset: usize,
    ranges: &[std::ops::Range<usize>],
) -> Option<blake3::Hash> {
    let file = object::File::parse(bytes).ok()?;
    if file.format() != object::BinaryFormat::MachO
        || file.kind() != object::ObjectKind::Relocatable
        || file.architecture() != object::Architecture::Aarch64
        || !file.is_64()
        || !file.is_little_endian()
    {
        return None;
    }
    let object::FileFlags::MachO { flags } = file.flags() else {
        return None;
    };

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sld-macho-object-patch-fingerprint-v4");
    hasher.update(bytes.get(4..12)?);
    hasher.update(&flags.to_le_bytes());

    let sections = file.sections().collect::<Vec<_>>();
    hasher.update(&(sections.len() as u64).to_le_bytes());
    let mut has_masked_text = false;
    let mut masked_text_undefined_symbols = HashSet::new();
    for section in sections {
        let segment_name = section.segment_name_bytes().ok().flatten()?;
        let section_name = section.name_bytes().ok()?;
        hash_length_prefixed(&mut hasher, segment_name);
        hash_length_prefixed(&mut hasher, section_name);
        let object::SectionFlags::MachO { flags } = section.flags() else {
            return None;
        };
        hasher.update(&flags.to_le_bytes());
        hasher.update(&section.align().to_le_bytes());

        let data = section.data().ok()?;
        let fully_masked = section.file_range().is_some_and(|(offset, size)| {
            let Ok(offset) = usize::try_from(offset) else {
                return false;
            };
            let Ok(size) = usize::try_from(size) else {
                return false;
            };
            let Some(start) = file_offset.checked_add(offset) else {
                return false;
            };
            let Some(end) = start.checked_add(size) else {
                return false;
            };
            size != 0
                && ranges
                    .iter()
                    .any(|range| range.start <= start && range.end >= end)
        });
        has_masked_text |= fully_masked && section_name == b"__text";
        hasher.update(&[u8::from(fully_masked)]);
        if !fully_masked {
            hasher.update(&section.size().to_le_bytes());
            hasher.update(&(data.len() as u64).to_le_bytes());
            let (data_offset, _) = section.file_range().unwrap_or_default();
            let data_offset = file_offset.checked_add(usize::try_from(data_offset).ok()?)?;
            update_hash_with_ranges(&mut hasher, data, data_offset, ranges);
        }

        let relocations = section.relocations().collect::<Vec<_>>();
        if fully_masked && section_name == b"__text" {
            for (_, relocation) in relocations {
                if let object::RelocationTarget::Symbol(symbol_index) = relocation.target()
                    && file
                        .symbol_by_index(symbol_index)
                        .is_ok_and(|symbol| symbol.is_undefined())
                {
                    masked_text_undefined_symbols.insert(symbol_index);
                }
            }
            continue;
        }
        hasher.update(&(relocations.len() as u64).to_le_bytes());
        for (offset, relocation) in relocations {
            if !(fully_masked && section_name == b"__text") {
                hasher.update(&offset.to_le_bytes());
            }
            let object::RelocationFlags::MachO {
                r_type,
                r_pcrel,
                r_length,
            } = relocation.flags()
            else {
                return None;
            };
            hasher.update(&[r_type, u8::from(r_pcrel), r_length]);
            hasher.update(&relocation.addend().to_le_bytes());
            hasher.update(&[u8::from(relocation.has_implicit_addend())]);
            hash_macho_relocation_target(&mut hasher, relocation.target())?;
            match relocation.subtractor() {
                Some(symbol) => {
                    hasher.update(&[1]);
                    hasher.update(&(symbol.0 as u64).to_le_bytes());
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
    }

    let symbols = file
        .symbols()
        .filter(|symbol| !masked_text_undefined_symbols.contains(&symbol.index()))
        .collect::<Vec<_>>();
    hasher.update(&(symbols.len() as u64).to_le_bytes());
    for symbol in symbols {
        hash_length_prefixed(&mut hasher, symbol.name_bytes().ok()?);
        hash_macho_symbol_section(&mut hasher, symbol.section())?;
        let value_range = macho_symbol_value_field_range(bytes, symbol.index())
            .map(|range| file_offset + range.start..file_offset + range.end);
        let address_is_masked = value_range.is_some_and(|value_range| {
            ranges
                .iter()
                .any(|range| range.start <= value_range.start && range.end >= value_range.end)
        });
        hasher.update(&[u8::from(address_is_masked)]);
        if !address_is_masked {
            let address = if let Some(section_index) = symbol.section_index() {
                symbol
                    .address()
                    .checked_sub(file.section_by_index(section_index).ok()?.address())?
            } else {
                symbol.address()
            };
            hasher.update(&address.to_le_bytes());
        }
        hasher.update(&symbol.size().to_le_bytes());
        hasher.update(&[
            macho_symbol_kind_tag(symbol.kind())?,
            macho_symbol_scope_tag(symbol.scope()),
            u8::from(symbol.is_weak()),
        ]);
        let object::SymbolFlags::MachO { n_desc } = symbol.flags() else {
            return None;
        };
        hasher.update(&n_desc.to_le_bytes());
    }

    has_masked_text.then(|| hasher.finalize())
}

fn hash_length_prefixed(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn hash_macho_relocation_target(
    hasher: &mut blake3::Hasher,
    target: object::RelocationTarget,
) -> Option<()> {
    match target {
        object::RelocationTarget::Symbol(symbol) => {
            hasher.update(&[0]);
            hasher.update(&(symbol.0 as u64).to_le_bytes());
        }
        object::RelocationTarget::Section(section) => {
            hasher.update(&[1]);
            hasher.update(&(section.0 as u64).to_le_bytes());
        }
        object::RelocationTarget::Absolute => {
            hasher.update(&[2]);
        }
        _ => return None,
    }
    Some(())
}

fn hash_macho_symbol_section(
    hasher: &mut blake3::Hasher,
    section: object::SymbolSection,
) -> Option<()> {
    match section {
        object::SymbolSection::Unknown => {
            hasher.update(&[0]);
        }
        object::SymbolSection::None => {
            hasher.update(&[1]);
        }
        object::SymbolSection::Undefined => {
            hasher.update(&[2]);
        }
        object::SymbolSection::Absolute => {
            hasher.update(&[3]);
        }
        object::SymbolSection::Common => {
            hasher.update(&[4]);
        }
        object::SymbolSection::Section(section) => {
            hasher.update(&[5]);
            hasher.update(&(section.0 as u64).to_le_bytes());
        }
        _ => return None,
    }
    Some(())
}

fn macho_symbol_kind_tag(kind: object::SymbolKind) -> Option<u8> {
    Some(match kind {
        object::SymbolKind::Unknown => 0,
        object::SymbolKind::Text => 1,
        object::SymbolKind::Data => 2,
        object::SymbolKind::Section => 3,
        object::SymbolKind::File => 4,
        object::SymbolKind::Label => 5,
        object::SymbolKind::Tls => 6,
        _ => return None,
    })
}

fn macho_symbol_scope_tag(scope: object::SymbolScope) -> u8 {
    match scope {
        object::SymbolScope::Unknown => 0,
        object::SymbolScope::Compilation => 1,
        object::SymbolScope::Linkage => 2,
        object::SymbolScope::Dynamic => 3,
    }
}

fn archive_member_patch_ranges_hash(
    data_offset: usize,
    data_len: usize,
    ranges: &[std::ops::Range<usize>],
) -> String {
    let data_end = data_offset + data_len;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sld-archive-member-patch-ranges-v1");
    for range in ranges {
        if range.end <= data_offset || range.start >= data_end {
            continue;
        }
        let start = range.start.max(data_offset) - data_offset;
        let end = range.end.min(data_end) - data_offset;
        hasher.update(&(start as u64).to_le_bytes());
        hasher.update(&(end as u64).to_le_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn update_hash_with_ranges(
    hasher: &mut blake3::Hasher,
    bytes: &[u8],
    data_offset: usize,
    ranges: &[std::ops::Range<usize>],
) {
    let data_end = data_offset + bytes.len();
    let mut position = 0;
    for range in ranges {
        if range.end <= data_offset || range.start >= data_end {
            continue;
        }
        let start = range.start.max(data_offset) - data_offset;
        let end = range.end.min(data_end) - data_offset;
        hasher.update(&bytes[position..start]);
        update_hash_with_zeroes(hasher, end - start);
        position = end;
    }
    hasher.update(&bytes[position..]);
}

fn patch_fingerprint_matches_previous_without_extra_ranges(
    previous_bytes: &[u8],
    current_fingerprint: &str,
    input_file_path: &str,
    matched_sections: &[MatchedPatchSection],
    normalize_rust_archive_patch_inputs: bool,
) -> Result<bool> {
    Ok(patch_fingerprint_with_extra_ranges_mode(
        previous_bytes,
        input_file_path,
        matched_sections
            .iter()
            .map(|section| section.previous.clone()),
        std::iter::empty(),
        normalize_rust_archive_patch_inputs,
    )?
    .as_deref()
        == Some(current_fingerprint))
}

fn normalize_patch_ranges(
    mut ranges: Vec<std::ops::Range<usize>>,
    bytes_len: usize,
) -> Option<Vec<std::ops::Range<usize>>> {
    ranges.sort_by_key(|range| range.start);
    let mut previous_end = 0;
    for range in &ranges {
        if range.start > range.end || range.end > bytes_len || range.start < previous_end {
            return None;
        }
        previous_end = range.end;
    }

    (!ranges.is_empty()).then_some(ranges)
}

#[cfg(test)]
fn match_patch_sections_from_current_hashes(
    current_bytes: &[u8],
    input_file_path: &str,
    sections: &[PatchSection],
) -> Result<Option<MatchedPatchSections>> {
    let current_resolver = PatchInputResolver::new(current_bytes, true)?;
    match_patch_sections_from_current_hashes_with_resolver(
        input_file_path,
        sections,
        &current_resolver,
    )
}

fn match_patch_sections_from_current_hashes_with_resolver(
    input_file_path: &str,
    sections: &[PatchSection],
    current_resolver: &PatchInputResolver<'_>,
) -> Result<Option<MatchedPatchSections>> {
    if sections.is_empty() || sections.iter().any(|section| section.data_hash.is_none()) {
        return Ok(None);
    }

    let Some(current_sections) = current_patch_sections_for_matching_with_resolver(
        input_file_path,
        sections,
        current_resolver,
    )?
    else {
        return Ok(None);
    };

    let mut matched_sections = Vec::with_capacity(sections.len());
    let mut changed_sections = Vec::new();
    for (previous, current) in sections.iter().cloned().zip(current_sections) {
        // A locally-generated name cannot identify a moved or changed section. It can still
        // remain at its recorded index when its content is unchanged; fingerprint validation
        // below rejects layout or non-patchable changes, and a later content change falls back
        // to reference-based matching.
        if previous.section_name.is_none() && previous.data_hash != current.data_hash {
            return Ok(None);
        }
        if previous.data_hash != current.data_hash {
            changed_sections.push(current.clone());
        }
        matched_sections.push(MatchedPatchSection { previous, current });
    }

    Ok(Some(MatchedPatchSections {
        sections: matched_sections,
        changed_sections,
    }))
}

fn current_patch_sections_for_matching_with_resolver(
    input_file_path: &str,
    sections: &[PatchSection],
    resolver: &PatchInputResolver<'_>,
) -> Result<Option<Vec<PatchSection>>> {
    let mut current_sections = std::iter::repeat_with(|| None)
        .take(sections.len())
        .collect::<Vec<_>>();
    let mut sections_by_input = HashMap::<&str, Vec<usize>>::new();
    for (section_index, section) in sections.iter().enumerate() {
        sections_by_input
            .entry(section.input.as_str())
            .or_default()
            .push(section_index);
    }

    for (input_ref, section_indices) in sections_by_input {
        let Some(input_bytes) = resolver.resolve(
            input_file_path,
            input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(None);
        };
        let file = object::File::parse(input_bytes.bytes)
            .context("Failed to parse changed patch input")?;
        for stored_section_index in section_indices {
            let patch_section = &sections[stored_section_index];
            let Some(section_index) = patch_section_index(&file, patch_section)? else {
                return Ok(None);
            };
            let section = file
                .section_by_index(section_index)
                .context("Missing changed patch section")?;
            let data = section
                .data()
                .context("Failed to read changed patch section data")?;
            if !section_size_allows_direct_patching(
                section.name().ok().map(str::as_bytes),
                patch_section.input_size,
                data.len(),
            ) || data.len() > patch_section.output_size as usize
            {
                return Ok(None);
            }
            let mut current = patch_section.clone();
            current.section_index = section_index.0 as u32;
            current.input_size = data.len() as u64;
            current.data_hash = Some(hash_bytes(data));
            current.cstring_nul_boundaries_hash =
                cstring_nul_boundaries_hash_for_section(section.name().ok(), data);
            current_sections[stored_section_index] = Some(current);
        }
    }

    Ok(Some(
        current_sections
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .context("Missing current matched patch section")?,
    ))
}

#[cfg(test)]
fn match_patch_sections(
    state_dir: &Path,
    previous_input: &FileState,
    current_bytes: &[u8],
    sections: &[PatchSection],
) -> Result<Option<MatchedPatchSections>> {
    let Some(previous_bytes) = read_verified_input_snapshot(state_dir, previous_input)? else {
        return Ok(None);
    };
    let previous_resolver = PatchInputResolver::new(&previous_bytes, true)?;
    let current_resolver = PatchInputResolver::new(current_bytes, true)?;
    match_patch_sections_with_resolvers(
        previous_input,
        &previous_resolver,
        &current_resolver,
        sections,
    )
}

fn match_patch_sections_with_resolvers(
    previous_input: &FileState,
    previous_resolver: &PatchInputResolver<'_>,
    current_resolver: &PatchInputResolver<'_>,
    sections: &[PatchSection],
) -> Result<Option<MatchedPatchSections>> {
    let mut sections_by_input = HashMap::<&str, Vec<(usize, &PatchSection)>>::new();
    for (section_index, section) in sections.iter().enumerate() {
        sections_by_input
            .entry(section.input.as_str())
            .or_default()
            .push((section_index, section));
    }

    let mut matched_sections = vec![None; sections.len()];
    let mut changed_sections = Vec::new();
    for (input_ref, sections) in sections_by_input {
        let Some(previous_input_bytes) = previous_resolver.resolve(
            previous_input.path.as_str(),
            input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(None);
        };
        let Some(current_input_bytes) = current_resolver.resolve(
            previous_input.path.as_str(),
            input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(None);
        };
        let previous_file = object::File::parse(previous_input_bytes.bytes)
            .context("Failed to parse previous patch input")?;
        let current_file = object::File::parse(current_input_bytes.bytes)
            .context("Failed to parse current patch input")?;
        let previous_references = section_reference_map(&previous_file)?;
        let current_references = section_reference_map(&current_file)?;

        for (matched_index, section) in sections {
            let Some(previous_index) = patch_section_index(&previous_file, section)? else {
                return Ok(None);
            };
            let Some(current_index) = match_current_patch_section_index(
                &current_file,
                section,
                previous_index,
                &previous_references,
                &current_references,
            )?
            else {
                return Ok(None);
            };

            let mut previous = section.clone();
            previous.section_index = previous_index.0 as u32;
            let mut current = section.clone();
            current.section_index = current_index.0 as u32;

            let previous_section = previous_file
                .section_by_index(previous_index)
                .context("Missing previous incremental patch section")?;
            let current_section = current_file
                .section_by_index(current_index)
                .context("Missing current incremental patch section")?;
            let previous_data = previous_section
                .data()
                .context("Failed to read previous incremental patch section data")?;
            let current_data = current_section
                .data()
                .context("Failed to read current incremental patch section data")?;
            previous.input_size = previous_data.len() as u64;
            current.input_size = current_data.len() as u64;
            current.data_hash = Some(hash_bytes(current_data));
            current.cstring_nul_boundaries_hash =
                cstring_nul_boundaries_hash_for_section(current_section.name().ok(), current_data);
            if previous_data != current_data {
                changed_sections.push(current.clone());
            }

            matched_sections[matched_index] = Some(MatchedPatchSection { previous, current });
        }
    }

    Ok(Some(MatchedPatchSections {
        sections: matched_sections
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .context("Missing matched incremental patch section")?,
        changed_sections,
    }))
}

fn match_current_patch_section_index(
    current_file: &object::File<'_>,
    patch_section: &PatchSection,
    previous_index: object::SectionIndex,
    previous_references: &HashMap<object::SectionIndex, Vec<SectionReference>>,
    current_references: &HashMap<object::SectionIndex, Vec<SectionReference>>,
) -> Result<Option<object::SectionIndex>> {
    if patch_section.section_name.is_some() {
        return patch_section_index(current_file, patch_section);
    }

    let Some(previous_signature) = previous_references.get(&previous_index) else {
        return Ok(None);
    };
    Ok(match_section_by_references(
        previous_signature,
        current_references,
    ))
}

fn match_section_by_references(
    previous_signature: &[SectionReference],
    current_references: &HashMap<object::SectionIndex, Vec<SectionReference>>,
) -> Option<object::SectionIndex> {
    if previous_signature.is_empty() {
        return None;
    }

    let mut matches = current_references
        .iter()
        .filter_map(|(index, signature)| (signature == previous_signature).then_some(*index));
    let index = matches.next()?;
    matches.next().is_none().then_some(index)
}

#[cfg(test)]
fn anonymous_patch_reference_counts(
    previous_index: object::SectionIndex,
    previous_references: &HashMap<object::SectionIndex, Vec<SectionReference>>,
    current_references: &HashMap<object::SectionIndex, Vec<SectionReference>>,
) -> (usize, usize) {
    let Some(previous_signature) = previous_references.get(&previous_index) else {
        return (0, 0);
    };
    (
        previous_signature.len(),
        current_references
            .values()
            .filter(|signature| *signature == previous_signature)
            .count(),
    )
}

fn section_reference_map(
    file: &object::File<'_>,
) -> Result<HashMap<object::SectionIndex, Vec<SectionReference>>> {
    let mut references = HashMap::<object::SectionIndex, Vec<SectionReference>>::new();
    for section in file.sections() {
        let Some(source_section_name) = patch_section_name_for_matching(&section) else {
            continue;
        };
        for (relocation_offset, relocation) in section.relocations() {
            let Some(target_section) = relocation_target_section(file, relocation.target())? else {
                continue;
            };
            references
                .entry(target_section)
                .or_default()
                .push(SectionReference {
                    source_section_name: source_section_name.clone(),
                    relocation_offset,
                    relocation_kind: format!("{:?}", relocation.kind()),
                    relocation_encoding: format!("{:?}", relocation.encoding()),
                    relocation_size: relocation.size(),
                    relocation_addend: relocation.addend(),
                });
        }
    }
    for signature in references.values_mut() {
        signature.sort();
    }
    Ok(references)
}

fn relocation_target_section(
    file: &object::File<'_>,
    target: object::RelocationTarget,
) -> Result<Option<object::SectionIndex>> {
    match target {
        object::RelocationTarget::Section(section) => Ok(Some(section)),
        object::RelocationTarget::Symbol(symbol) => {
            Ok(file.symbol_by_index(symbol)?.section_index())
        }
        object::RelocationTarget::Absolute => Ok(None),
        _ => Ok(None),
    }
}

#[cfg(test)]
fn changed_patch_sections(
    state_dir: &Path,
    previous_input: &FileState,
    current_bytes: &[u8],
    sections: &[MatchedPatchSection],
) -> Result<Option<Vec<PatchSection>>> {
    let Some(previous_bytes) = read_verified_input_snapshot(state_dir, previous_input)? else {
        return Ok(None);
    };
    let previous_resolver = PatchInputResolver::new(&previous_bytes, true)?;
    let current_resolver = PatchInputResolver::new(current_bytes, true)?;
    changed_patch_sections_with_resolvers(
        previous_input,
        &previous_resolver,
        &current_resolver,
        sections,
    )
}

fn changed_patch_sections_with_resolvers(
    previous_input: &FileState,
    previous_resolver: &PatchInputResolver<'_>,
    current_resolver: &PatchInputResolver<'_>,
    sections: &[MatchedPatchSection],
) -> Result<Option<Vec<PatchSection>>> {
    let mut changed_sections = Vec::new();

    let mut sections_by_input = HashMap::<&str, Vec<&MatchedPatchSection>>::new();
    for section in sections {
        sections_by_input
            .entry(section.current.input.as_str())
            .or_default()
            .push(section);
    }

    for (input_ref, sections) in sections_by_input {
        let Some(previous_input_bytes) = previous_resolver.resolve(
            previous_input.path.as_str(),
            input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(None);
        };
        let Some(current_input_bytes) = current_resolver.resolve(
            previous_input.path.as_str(),
            input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(None);
        };
        let previous_file = object::File::parse(previous_input_bytes.bytes)
            .context("Failed to parse previous patch input")?;
        let current_file = object::File::parse(current_input_bytes.bytes)
            .context("Failed to parse current patch input")?;

        for patch_section in sections {
            let Some(previous_index) =
                patch_section_index(&previous_file, &patch_section.previous)?
            else {
                return Ok(None);
            };
            let Some(current_index) = patch_section_index(&current_file, &patch_section.current)?
            else {
                return Ok(None);
            };
            let previous_section = previous_file
                .section_by_index(previous_index)
                .context("Missing previous incremental patch section")?;
            let current_section = current_file
                .section_by_index(current_index)
                .context("Missing current incremental patch section")?;
            let previous_data = previous_section
                .data()
                .context("Failed to read previous incremental patch section data")?;
            let current_data = current_section
                .data()
                .context("Failed to read current incremental patch section data")?;
            if previous_data != current_data {
                changed_sections.push(patch_section.current.clone());
            }
        }
    }

    Ok(Some(changed_sections))
}

fn matched_cstring_literal_boundaries_are_stable(
    input_file_path: &str,
    sections: &[MatchedPatchSection],
    previous_resolver: Option<&PatchInputResolver<'_>>,
    current_resolver: &PatchInputResolver<'_>,
) -> Result<bool> {
    let mut sections_by_input = HashMap::<&str, Vec<&MatchedPatchSection>>::new();
    for section in sections {
        sections_by_input
            .entry(section.current.input.as_str())
            .or_default()
            .push(section);
    }

    for (input_ref, sections) in sections_by_input {
        let Some(current_input_bytes) = current_resolver.resolve(
            input_file_path,
            input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(false);
        };
        let current_file = object::File::parse(current_input_bytes.bytes)
            .context("Failed to parse current cstring patch input")?;
        let previous_file = if let Some(previous_resolver) = previous_resolver {
            let Some(previous_input_bytes) = previous_resolver.resolve(
                input_file_path,
                input_ref,
                PatchInputLookup::MatchArchiveMember,
            )?
            else {
                return Ok(false);
            };
            Some(
                object::File::parse(previous_input_bytes.bytes)
                    .context("Failed to parse previous cstring patch input")?,
            )
        } else {
            None
        };

        for patch_section in sections {
            let Some(current_index) = patch_section_index(&current_file, &patch_section.current)?
            else {
                return Ok(false);
            };
            let current_section = current_file
                .section_by_index(current_index)
                .context("Missing current cstring patch section")?;
            if current_section.name().ok() != Some("__cstring") {
                continue;
            }
            let current_data = current_section
                .data()
                .context("Failed to read current cstring patch section data")?;
            if patch_section
                .previous
                .cstring_nul_boundaries_hash
                .as_deref()
                == Some(cstring_nul_boundaries_hash(current_data).as_str())
            {
                continue;
            }
            let Some(previous_file) = previous_file.as_ref() else {
                return Ok(false);
            };
            let Some(previous_index) = patch_section_index(previous_file, &patch_section.previous)?
            else {
                return Ok(false);
            };
            let previous_section = previous_file
                .section_by_index(previous_index)
                .context("Missing previous cstring patch section")?;
            if previous_section.name().ok() != Some("__cstring") {
                return Ok(false);
            }
            if !cstring_literal_boundaries_are_stable(
                previous_section
                    .data()
                    .context("Failed to read previous cstring patch section data")?,
                current_data,
            ) {
                return Ok(false);
            }
        }
    }

    Ok(true)
}

fn matched_x86_64_elf_got_relaxation_contexts_are_stable(
    previous_bytes: Option<&[u8]>,
    current_bytes: &[u8],
    input_file_path: &str,
    sections: &[MatchedPatchSection],
) -> Result<bool> {
    let mut sections_by_input = HashMap::<&str, Vec<&MatchedPatchSection>>::new();
    for section in sections {
        sections_by_input
            .entry(section.current.input.as_str())
            .or_default()
            .push(section);
    }

    for (input_ref, sections) in sections_by_input {
        let Some(current_input_bytes) =
            patch_input_bytes(current_bytes, input_file_path, input_ref)?
        else {
            return Ok(false);
        };
        let current_file = object::File::parse(current_input_bytes.bytes)
            .context("Failed to parse current x86-64 ELF GOT patch input")?;
        if current_file.format() != object::BinaryFormat::Elf
            || current_file.architecture() != object::Architecture::X86_64
        {
            continue;
        }

        let Some(previous_bytes) = previous_bytes else {
            return Ok(false);
        };
        let Some(previous_input_bytes) =
            patch_input_bytes(previous_bytes, input_file_path, input_ref)?
        else {
            return Ok(false);
        };
        let previous_file = object::File::parse(previous_input_bytes.bytes)
            .context("Failed to parse previous x86-64 ELF GOT patch input")?;
        if previous_file.format() != object::BinaryFormat::Elf
            || previous_file.architecture() != object::Architecture::X86_64
        {
            return Ok(false);
        }

        for patch_section in sections {
            let Some(previous_index) =
                patch_section_index(&previous_file, &patch_section.previous)?
            else {
                return Ok(false);
            };
            let Some(current_index) = patch_section_index(&current_file, &patch_section.current)?
            else {
                return Ok(false);
            };
            let previous_section = previous_file
                .section_by_index(previous_index)
                .context("Missing previous x86-64 ELF GOT patch section")?;
            let current_section = current_file
                .section_by_index(current_index)
                .context("Missing current x86-64 ELF GOT patch section")?;
            let Some(previous_candidates) =
                x86_64_elf_got_relaxation_candidates(&previous_section)?
            else {
                return Ok(false);
            };
            let Some(current_candidates) = x86_64_elf_got_relaxation_candidates(&current_section)?
            else {
                return Ok(false);
            };
            if previous_candidates != current_candidates {
                return Ok(false);
            }
        }
    }

    Ok(true)
}

fn x86_64_elf_got_relaxation_candidates<'data>(
    section: &impl object::ObjectSection<'data>,
) -> Result<Option<Vec<u64>>> {
    let data = section
        .data()
        .context("Failed to read x86-64 ELF GOT patch section data")?;
    let mut candidates = Vec::new();
    for (offset, relocation) in section.relocations() {
        if !matches!(
            (relocation.kind(), relocation.encoding(), relocation.size()),
            (
                object::RelocationKind::GotRelative,
                object::RelocationEncoding::Generic,
                32
            )
        ) {
            continue;
        }
        let offset_index = usize::try_from(offset)
            .context("x86-64 ELF GOT relocation offset does not fit usize")?;
        let Some(opcode) = offset_index
            .checked_sub(2)
            .and_then(|index| data.get(index))
        else {
            return Ok(None);
        };
        // Standard GOTPCREL can only relax when its instruction opcode is `mov`.
        // Other instruction-byte changes are copied directly without changing
        // the linker's relaxation decision.
        if *opcode == 0x8b {
            candidates.push(offset);
        }
    }
    Ok(Some(candidates))
}

fn patch_section_index(
    file: &object::File<'_>,
    patch_section: &PatchSection,
) -> Result<Option<object::SectionIndex>> {
    let Some(name) = patch_section.section_name.as_deref() else {
        return patch_section_object_index(file, patch_section.section_index).map(Some);
    };

    let mut matches = file.sections().filter_map(|section| {
        (patch_section_name_for_matching(&section).as_deref() == Some(name))
            .then(|| section.index())
    });
    let Some(index) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Ok(None);
    }
    Ok(Some(index))
}

#[cfg(test)]
fn patch_sections_for_input(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
) -> Result<Option<Vec<SectionPatch>>> {
    Ok(
        resolved_patch_sections_for_input(bytes, input_file_path, sections)?
            .map(|patches| patches.into_iter().map(|resolved| resolved.patch).collect()),
    )
}

#[cfg(test)]
fn resolve_current_patch_sections<'a>(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    dynamic_relocations: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
    relocations: impl IntoIterator<Item = &'a RelocationRecord>,
) -> Result<Option<Vec<PatchSection>>> {
    let resolver = PatchInputResolver::new(bytes, true)?;
    resolve_current_patch_sections_with_resolver(
        input_file_path,
        sections,
        dynamic_relocations,
        relocations,
        &resolver,
    )
}

fn resolve_current_patch_sections_with_resolver<'a>(
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    dynamic_relocations: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
    relocations: impl IntoIterator<Item = &'a RelocationRecord>,
    resolver: &PatchInputResolver<'_>,
) -> Result<Option<Vec<PatchSection>>> {
    Ok(resolved_patch_sections_for_input_with_resolver(
        input_file_path,
        sections,
        dynamic_relocations,
        relocations,
        resolver,
    )?
    .map(|patches| {
        patches
            .into_iter()
            .map(|resolved| resolved.section)
            .collect()
    }))
}

#[cfg(test)]
fn resolved_patch_sections_for_input(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
) -> Result<Option<Vec<ResolvedSectionPatch>>> {
    resolved_patch_sections_for_input_with_dynamic_relocations(
        bytes,
        input_file_path,
        sections,
        std::iter::empty(),
        std::iter::empty(),
    )
}

#[cfg(test)]
fn resolved_patch_sections_for_input_with_dynamic_relocations<'a>(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    dynamic_relocations: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
    relocations: impl IntoIterator<Item = &'a RelocationRecord>,
) -> Result<Option<Vec<ResolvedSectionPatch>>> {
    let resolver = PatchInputResolver::new(bytes, true)?;
    resolved_patch_sections_for_input_with_resolver(
        input_file_path,
        sections,
        dynamic_relocations,
        relocations,
        &resolver,
    )
}

fn resolved_patch_sections_for_input_with_resolver<'a>(
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    dynamic_relocations: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
    relocations: impl IntoIterator<Item = &'a RelocationRecord>,
    resolver: &PatchInputResolver<'_>,
) -> Result<Option<Vec<ResolvedSectionPatch>>> {
    let sections = sections.into_iter().collect::<Vec<_>>();
    let dynamic_relocation_offsets =
        dynamic_relocation_offsets_by_input_section(dynamic_relocations);
    let relocation_offsets = relocation_offsets_by_input_section(relocations);
    let mut patches = std::iter::repeat_with(|| None)
        .take(sections.len())
        .collect::<Vec<_>>();
    let mut sections_by_input = HashMap::<&str, Vec<usize>>::new();
    for (section_index, section) in sections.iter().enumerate() {
        sections_by_input
            .entry(section.input.as_str())
            .or_default()
            .push(section_index);
    }

    for (input_ref, section_indices) in sections_by_input {
        let Some(input_bytes) = resolver.resolve(
            input_file_path,
            input_ref,
            PatchInputLookup::MatchArchiveMember,
        )?
        else {
            return Ok(None);
        };
        let file = object::File::parse(input_bytes.bytes)
            .context("Failed to parse changed incremental input")?;
        for stored_section_index in section_indices {
            let patch_section = &sections[stored_section_index];
            let Some(section_index) = patch_section_index(&file, patch_section)? else {
                return Ok(None);
            };
            let section = file
                .section_by_index(section_index)
                .context("Missing changed incremental input section")?;
            let data = section
                .data()
                .context("Failed to read changed incremental input section data")?;
            let dynamic_relocations =
                dynamic_relocation_offsets.get(&(input_ref, patch_section.section_index));
            let relocations = relocation_offsets.get(&(input_ref, patch_section.section_index));
            let Some(preserve_ranges) = section_direct_patch_preserve_ranges(
                &file,
                &section,
                data,
                dynamic_relocations,
                relocations,
            ) else {
                return Ok(None);
            };
            if !section_size_allows_direct_patching(
                section.name().ok().map(str::as_bytes),
                patch_section.input_size,
                data.len(),
            ) || data.len() > patch_section.output_size as usize
            {
                return Ok(None);
            }
            let mut resolved_section = patch_section.clone();
            resolved_section.section_index = section_index.0 as u32;
            resolved_section.input_size = data.len() as u64;
            resolved_section.data_hash = Some(hash_bytes(data));
            resolved_section.cstring_nul_boundaries_hash =
                cstring_nul_boundaries_hash_for_section(section.name().ok(), data);
            patches[stored_section_index] = Some(ResolvedSectionPatch {
                section: resolved_section,
                patch: SectionPatch {
                    output_offset: patch_section.output_offset,
                    size: patch_section.output_size,
                    data: data.to_owned(),
                    deferred_relocation: None,
                    preserve_ranges,
                    adjustments: Vec::new(),
                },
            });
        }
    }
    Ok(Some(
        patches
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .context("Missing resolved incremental patch section")?,
    ))
}

fn dynamic_relocation_patches_for_input<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
) -> Result<Vec<DynamicRelocationPatch>> {
    dynamic_relocation_patches_for_input_with_lookup(
        bytes,
        input_file_path,
        records,
        PatchInputLookup::MatchArchiveMember,
    )
}

fn dynamic_relocation_patches_for_current_records<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
) -> Result<Vec<DynamicRelocationPatch>> {
    dynamic_relocation_patches_for_input_with_lookup(
        bytes,
        input_file_path,
        records,
        PatchInputLookup::CurrentRecordedRange,
    )
}

fn dynamic_relocation_patches_for_input_with_lookup<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
    lookup: PatchInputLookup,
) -> Result<Vec<DynamicRelocationPatch>> {
    let records = records.into_iter().collect::<Vec<_>>();
    let mut patches = Vec::new();
    let mut records_by_input = HashMap::<&str, Vec<&DynamicRelocationRecord>>::new();
    for record in records {
        records_by_input
            .entry(record.input.as_str())
            .or_default()
            .push(record);
    }

    for (input_ref, records) in records_by_input {
        let Some(input_bytes) =
            patch_input_bytes_with_lookup(bytes, input_file_path, input_ref, lookup)?
        else {
            continue;
        };
        patches.extend(dynamic_relocation_patches_for_input_bytes(
            input_bytes.bytes,
            input_bytes.file_offset,
            records,
        )?);
    }

    Ok(patches)
}

fn added_dynamic_relocation_patches_for_input(
    current_bytes: &[u8],
    previous_bytes: &[u8],
    input_file_path: &str,
    matched_sections: &[MatchedPatchSection],
    previous_dynamic_relocations: &[DynamicRelocationRecord],
    previous_sections: &[SectionRecord],
) -> Vec<DynamicRelocationPatch> {
    if matched_sections.iter().any(|section| {
        section.previous.input != input_file_path || section.current.input != input_file_path
    }) {
        return Vec::new();
    }

    let Some(current_section_headers) = elf_section_headers(current_bytes) else {
        return Vec::new();
    };
    let Some(previous_section_headers) = elf_section_headers(previous_bytes) else {
        return Vec::new();
    };
    let free_slots = free_dynamic_relocation_output_slots(
        previous_sections,
        previous_dynamic_relocations
            .iter()
            .filter(|record| record.size == crate::elf::RELA_ENTRY_SIZE),
    );
    if free_slots.is_empty() {
        return Vec::new();
    }

    let previous_records = previous_dynamic_relocations
        .iter()
        .filter(|record| {
            record.input_file == input_file_path
                && record.input == input_file_path
                && record.has_restorable_rela_output_info()
        })
        .collect::<Vec<_>>();
    if previous_records.is_empty() {
        return Vec::new();
    }

    let mut patches = Vec::new();
    let mut next_free_slot = 0;
    for matched in matched_sections {
        let previous_entries = rela_entries_for_section(
            previous_bytes,
            &previous_section_headers,
            matched.previous.section_index,
        )
        .unwrap_or_default();
        let current_entries = rela_entries_for_section(
            current_bytes,
            &current_section_headers,
            matched.current.section_index,
        )
        .unwrap_or_default();
        let previous_offsets = previous_entries
            .iter()
            .map(|entry| entry.offset)
            .collect::<HashSet<_>>();
        for entry in current_entries
            .iter()
            .filter(|entry| !previous_offsets.contains(&entry.offset))
        {
            let Some(reference) = previous_records.iter().find_map(|record| {
                (record.section_index == matched.previous.section_index).then(|| {
                    previous_entries
                        .iter()
                        .find(|previous| {
                            previous.offset == record.relocation_offset
                                && previous.info == entry.info
                        })
                        .map(|_| *record)
                })?
            }) else {
                continue;
            };
            let Some(output_offset) = free_slots.get(next_free_slot).copied() else {
                return patches;
            };
            next_free_slot += 1;

            let delta = i128::from(entry.offset) - i128::from(reference.relocation_offset);
            let Some(output_r_offset) =
                add_signed_delta_u64(reference.output_r_offset.unwrap(), delta)
            else {
                continue;
            };
            let output_r_info = reference.output_r_info.unwrap();
            let mut data = vec![0; crate::elf::RELA_ENTRY_SIZE as usize];
            data[0..8].copy_from_slice(&output_r_offset.to_le_bytes());
            data[8..16].copy_from_slice(&output_r_info.to_le_bytes());
            data[16..24].copy_from_slice(&entry.addend.to_le_bytes());
            patches.push(DynamicRelocationPatch {
                record: DynamicRelocationRecord {
                    input_file: input_file_path.to_owned().into(),
                    input: input_file_path.to_owned().into(),
                    section_index: matched.current.section_index,
                    relocation_offset: entry.offset,
                    output_offset,
                    size: crate::elf::RELA_ENTRY_SIZE,
                    output_r_offset: Some(output_r_offset),
                    output_r_info: Some(output_r_info),
                },
                input_range: Some(entry.addend_range.clone()),
                patch: SectionPatch {
                    output_offset,
                    size: crate::elf::RELA_ENTRY_SIZE,
                    data,
                    deferred_relocation: None,
                    preserve_ranges: Vec::new(),
                    adjustments: Vec::new(),
                },
                is_new: true,
            });
        }
    }

    patches
}

fn free_dynamic_relocation_output_slots<'a>(
    sections: &[SectionRecord],
    dynamic_relocations: impl IntoIterator<Item = &'a DynamicRelocationRecord>,
) -> Vec<u64> {
    let used = dynamic_relocations
        .into_iter()
        .map(|record| record.output_offset)
        .collect::<HashSet<_>>();
    let mut slots = Vec::new();
    for section in sections {
        if section.input_file != GENERATED_SECTION_INPUT_FILE
            || section.input != GENERATED_RELA_DYN_GENERAL
        {
            continue;
        }
        let mut output_offset = section.output_offset;
        let Some(end) = section.output_offset.checked_add(section.size) else {
            continue;
        };
        while output_offset
            .checked_add(crate::elf::RELA_ENTRY_SIZE)
            .is_some_and(|slot_end| slot_end <= end)
        {
            if !used.contains(&output_offset) {
                slots.push(output_offset);
            }
            output_offset += crate::elf::RELA_ENTRY_SIZE;
        }
    }
    slots.sort_unstable();
    slots
}

fn relocation_addend_patches_for_input(
    relocations: &mut [RelocationRecord],
    input: &FileState,
    bytes: &[u8],
    previous_bytes: Option<&[u8]>,
    dynamic_relocations: &[DynamicRelocationRecord],
) -> Result<std::result::Result<RelocationAddendPatches, String>> {
    let mut input_ranges = Vec::new();
    let mut output_patches = Vec::new();
    let dynamic_relocation_keys = dynamic_relocations
        .iter()
        .filter(|relocation| relocation.input_file == input.path)
        .map(|relocation| {
            (
                relocation.input.as_str(),
                relocation.section_index,
                relocation.relocation_offset,
            )
        })
        .collect::<HashSet<_>>();
    let mut relocations_by_input = HashMap::<String, Vec<usize>>::new();
    for (relocation_index, relocation) in relocations.iter().enumerate() {
        if relocation.input_file == input.path {
            relocations_by_input
                .entry(relocation.input.to_string())
                .or_default()
                .push(relocation_index);
        }
    }

    for (input_ref, relocation_indices) in relocations_by_input {
        let Some(input_bytes) = patch_input_bytes(bytes, input.path.as_str(), &input_ref)? else {
            continue;
        };
        let previous_input_bytes = if let Some(previous_bytes) = previous_bytes {
            patch_input_bytes(previous_bytes, input.path.as_str(), &input_ref)?
        } else {
            None
        };
        let Some(section_headers) = elf_section_headers(input_bytes.bytes) else {
            continue;
        };
        let previous_section_headers = previous_input_bytes
            .as_ref()
            .and_then(|input_bytes| elf_section_headers(input_bytes.bytes));
        let mut entries_by_section = HashMap::<u32, Vec<RelaPatchEntry>>::new();
        let mut previous_entries_by_section = HashMap::<u32, Vec<RelaPatchEntry>>::new();
        for relocation_index in relocation_indices {
            let section_index = relocations[relocation_index].section_index;
            let relocation_offset = relocations[relocation_index].relocation_offset;
            let entries = entries_by_section.entry(section_index).or_insert_with(|| {
                rela_entries_for_section(input_bytes.bytes, &section_headers, section_index)
                    .unwrap_or_default()
            });
            let Some(entry) = entries
                .iter()
                .find(|entry| entry.offset == relocation_offset)
            else {
                continue;
            };
            input_ranges.push(
                input_bytes.file_offset + entry.addend_range.start
                    ..input_bytes.file_offset + entry.addend_range.end,
            );
            let relocation = &mut relocations[relocation_index];
            if entry.addend == relocation.addend {
                continue;
            }
            let raw_addend_unchanged = previous_input_bytes
                .as_ref()
                .zip(previous_section_headers.as_ref())
                .is_some_and(|(previous_input_bytes, previous_section_headers)| {
                    let previous_entries = previous_entries_by_section
                        .entry(section_index)
                        .or_insert_with(|| {
                            rela_entries_for_section(
                                previous_input_bytes.bytes,
                                previous_section_headers,
                                section_index,
                            )
                            .unwrap_or_default()
                        });
                    previous_entries
                        .iter()
                        .find(|previous_entry| previous_entry.offset == relocation_offset)
                        .is_some_and(|previous_entry| previous_entry.addend == entry.addend)
                });
            if raw_addend_unchanged {
                continue;
            }
            if dynamic_relocation_keys.contains(&(
                input_ref.as_str(),
                section_index,
                relocation_offset,
            )) {
                relocation.addend = entry.addend;
                continue;
            }
            let Some(written_value) = relocation.written_value else {
                return Ok(Err(format!(
                    "missing written relocation value for addend change in {}",
                    display_hex_path(&input.path)
                )));
            };
            if relocation.size != 8 {
                return Ok(Err(format!(
                    "unsupported relocation addend patch size in {}",
                    display_hex_path(&input.path)
                )));
            }
            let delta = i128::from(entry.addend) - i128::from(relocation.addend);
            let Some(written_value) = add_signed_delta_u64(written_value, delta) else {
                return Ok(Err(format!(
                    "relocation addend patch overflowed in {}",
                    display_hex_path(&input.path)
                )));
            };
            output_patches.push(SectionPatch {
                output_offset: relocation.output_offset,
                size: relocation.size,
                data: written_value.to_le_bytes().to_vec(),
                deferred_relocation: None,
                preserve_ranges: Vec::new(),
                adjustments: Vec::new(),
            });
            relocation.written_value = Some(written_value);
            relocation.addend = entry.addend;
        }
    }
    dedup_ranges(&mut input_ranges);
    Ok(Ok(RelocationAddendPatches {
        input_ranges,
        output_patches,
    }))
}

#[cfg(test)]
fn relocation_addend_ranges_for_input<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a RelocationRecord>,
) -> Result<Vec<std::ops::Range<usize>>> {
    relocation_addend_ranges_for_input_with_lookup(
        bytes,
        input_file_path,
        records,
        PatchInputLookup::MatchArchiveMember,
    )
}

fn relocation_addend_ranges_for_current_records<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a RelocationRecord>,
) -> Result<Vec<std::ops::Range<usize>>> {
    relocation_addend_ranges_for_input_with_lookup(
        bytes,
        input_file_path,
        records,
        PatchInputLookup::CurrentRecordedRange,
    )
}

fn relocation_addend_ranges_for_input_with_lookup<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a RelocationRecord>,
    lookup: PatchInputLookup,
) -> Result<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::new();
    let mut records_by_input = HashMap::<&str, Vec<&RelocationRecord>>::new();
    for record in records {
        records_by_input
            .entry(record.input.as_str())
            .or_default()
            .push(record);
    }

    for (input_ref, records) in records_by_input {
        let Some(input_bytes) =
            patch_input_bytes_with_lookup(bytes, input_file_path, input_ref, lookup)?
        else {
            continue;
        };
        let Some(section_headers) = elf_section_headers(input_bytes.bytes) else {
            continue;
        };
        let mut entries_by_section = HashMap::<u32, Vec<RelaPatchEntry>>::new();
        for record in records {
            let entries = entries_by_section
                .entry(record.section_index)
                .or_insert_with(|| {
                    rela_entries_for_section(
                        input_bytes.bytes,
                        &section_headers,
                        record.section_index,
                    )
                    .unwrap_or_default()
                });
            let Some(entry) = entries
                .iter()
                .find(|entry| entry.offset == record.relocation_offset)
            else {
                continue;
            };
            ranges.push(
                input_bytes.file_offset + entry.addend_range.start
                    ..input_bytes.file_offset + entry.addend_range.end,
            );
        }
    }

    dedup_ranges(&mut ranges);
    Ok(ranges)
}

fn relocation_target_ranges_for_current_records<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a RelocationRecord>,
) -> Result<Vec<std::ops::Range<usize>>> {
    relocation_target_ranges_for_input_with_lookup(
        bytes,
        input_file_path,
        records,
        PatchInputLookup::CurrentRecordedRange,
    )
}

fn relocation_target_ranges_for_input_with_lookup<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a RelocationRecord>,
    lookup: PatchInputLookup,
) -> Result<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::new();
    let mut records_by_input = HashMap::<&str, Vec<&RelocationRecord>>::new();
    for record in records {
        let Some(target) = &record.target else {
            continue;
        };
        records_by_input
            .entry(target.input.as_str())
            .or_default()
            .push(record);
    }

    for (input_ref, records) in records_by_input {
        let Some(input_bytes) =
            patch_input_bytes_with_lookup(bytes, input_file_path, input_ref, lookup)?
        else {
            continue;
        };
        let file = object::File::parse(input_bytes.bytes)
            .context("Failed to parse incremental relocation target input")?;
        let mut seen_names = HashSet::new();
        for record in records {
            let Some(target_name) = record.target_name.as_deref() else {
                continue;
            };
            if !seen_names.insert(target_name) {
                continue;
            }
            let Some(symbol) = symbol_position_by_name(
                input_bytes.bytes,
                input_bytes.file_offset,
                &file,
                target_name,
            )?
            else {
                continue;
            };
            if let Some(value_range) = symbol.value_range {
                ranges.push(value_range);
            }
        }
    }

    dedup_ranges(&mut ranges);
    Ok(ranges)
}

fn dynamic_relocation_patches_for_input_bytes(
    bytes: &[u8],
    file_offset: usize,
    records: Vec<&DynamicRelocationRecord>,
) -> Result<Vec<DynamicRelocationPatch>> {
    let Some(section_headers) = elf_section_headers(bytes) else {
        return Ok(Vec::new());
    };

    let mut patches = Vec::new();
    for record in records {
        if record.size != crate::elf::RELA_ENTRY_SIZE {
            continue;
        }
        let Some(entry_range) = dynamic_relocation_entry_range(bytes, &section_headers, record)?
        else {
            patches.push(DynamicRelocationPatch {
                record: record.clone(),
                input_range: None,
                patch: SectionPatch {
                    output_offset: record.output_offset,
                    size: record.size,
                    data: vec![0; record.size as usize],
                    deferred_relocation: None,
                    preserve_ranges: Vec::new(),
                    adjustments: Vec::new(),
                },
                is_new: false,
            });
            continue;
        };
        let addend_start = entry_range.start + 16;
        let addend_end = addend_start + 8;
        let Some(addend) = bytes.get(addend_start..addend_end) else {
            continue;
        };
        let mut data = vec![0; record.size as usize];
        let preserve_ranges = if let (Some(output_r_offset), Some(output_r_info)) =
            (record.output_r_offset, record.output_r_info)
        {
            data[0..8].copy_from_slice(&output_r_offset.to_le_bytes());
            data[8..16].copy_from_slice(&output_r_info.to_le_bytes());
            Vec::new()
        } else {
            std::iter::once(0..16).collect()
        };
        data[16..24].copy_from_slice(addend);
        patches.push(DynamicRelocationPatch {
            record: record.clone(),
            input_range: Some(file_offset + addend_start..file_offset + addend_end),
            patch: SectionPatch {
                output_offset: record.output_offset,
                size: record.size,
                data,
                deferred_relocation: None,
                preserve_ranges,
                adjustments: Vec::new(),
            },
            is_new: false,
        });
    }
    Ok(patches)
}

fn object_diff_allows_dynamic_relocation_removal(
    previous_bytes: &[u8],
    current_bytes: &[u8],
    input_file_path: &str,
    matched_sections: &[MatchedPatchSection],
    dynamic_relocation_patches: &[DynamicRelocationPatch],
) -> Result<bool> {
    if dynamic_relocation_patches.is_empty()
        || dynamic_relocation_patches
            .iter()
            .all(|patch| patch.input_range.is_some())
        || dynamic_relocation_patches.iter().any(|patch| {
            patch.record.input_file != input_file_path || patch.record.input != input_file_path
        })
        || matched_sections.iter().any(|section| {
            section.previous.input != input_file_path
                || section.current.input != input_file_path
                || section.previous.section_index != section.current.section_index
        })
    {
        return Ok(false);
    }

    let Some(current_section_headers) = elf_section_headers(current_bytes) else {
        return Ok(false);
    };
    let mut kept_offsets_by_section = HashMap::<u32, HashSet<u64>>::new();
    let mut section_indices = HashSet::new();
    for patch in dynamic_relocation_patches {
        section_indices.insert(patch.record.section_index);
        if patch.input_range.is_some() {
            kept_offsets_by_section
                .entry(patch.record.section_index)
                .or_default()
                .insert(patch.record.relocation_offset);
        }
    }
    for section_index in section_indices {
        let kept_offsets = kept_offsets_by_section
            .get(&section_index)
            .cloned()
            .unwrap_or_default();
        let current_entries =
            rela_entries_for_section(current_bytes, &current_section_headers, section_index)
                .unwrap_or_default();
        if current_entries
            .iter()
            .any(|entry| !kept_offsets.contains(&entry.offset))
        {
            return Ok(false);
        }
    }

    let previous_file = object::File::parse(previous_bytes)
        .context("Failed to parse previous dynamic relocation removal input")?;
    let current_file = object::File::parse(current_bytes)
        .context("Failed to parse current dynamic relocation removal input")?;
    let previous_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.previous.section_index as usize))
        .collect::<HashSet<_>>();
    let current_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.current.section_index as usize))
        .collect::<HashSet<_>>();

    let mut previous_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in previous_file.sections() {
        if previous_patch_indices.contains(&section.index()) {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_dynamic_relocation_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read previous dynamic relocation removal section data")?;
        previous_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    let mut current_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in current_file.sections() {
        if current_patch_indices.contains(&section.index()) {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_dynamic_relocation_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read current dynamic relocation removal section data")?;
        current_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    Ok(previous_sections_by_name == current_sections_by_name)
}

fn object_diff_allows_dynamic_relocation_addition(
    previous_bytes: &[u8],
    current_bytes: &[u8],
    input_file_path: &str,
    matched_sections: &[MatchedPatchSection],
    dynamic_relocation_patches: &[DynamicRelocationPatch],
) -> Result<bool> {
    if dynamic_relocation_patches.is_empty()
        || dynamic_relocation_patches
            .iter()
            .all(|patch| patch.input_range.is_none())
        || dynamic_relocation_patches
            .iter()
            .any(|patch| patch.input_range.is_none())
        || dynamic_relocation_patches.iter().any(|patch| {
            patch.record.input_file != input_file_path
                || patch.record.input != input_file_path
                || !patch.record.has_restorable_rela_output_info()
        })
        || matched_sections.iter().any(|section| {
            section.previous.input != input_file_path || section.current.input != input_file_path
        })
    {
        return Ok(false);
    }

    let Some(previous_section_headers) = elf_section_headers(previous_bytes) else {
        return Ok(false);
    };
    let Some(current_section_headers) = elf_section_headers(current_bytes) else {
        return Ok(false);
    };
    let mut any_added = false;
    let section_indices = dynamic_relocation_patches
        .iter()
        .map(|patch| patch.record.section_index)
        .collect::<HashSet<_>>();
    for section_index in section_indices {
        let previous_entries =
            rela_entries_for_section(previous_bytes, &previous_section_headers, section_index)
                .unwrap_or_default();
        let current_entries =
            rela_entries_for_section(current_bytes, &current_section_headers, section_index)
                .unwrap_or_default();
        let previous_offsets = previous_entries
            .iter()
            .map(|entry| entry.offset)
            .collect::<HashSet<_>>();
        let current_offsets = current_entries
            .iter()
            .map(|entry| entry.offset)
            .collect::<HashSet<_>>();
        if !previous_offsets.is_subset(&current_offsets) {
            return Ok(false);
        }
        let added_offsets = dynamic_relocation_patches
            .iter()
            .filter(|patch| {
                patch.record.section_index == section_index
                    && !previous_offsets.contains(&patch.record.relocation_offset)
            })
            .map(|patch| patch.record.relocation_offset)
            .collect::<HashSet<_>>();
        any_added |= !added_offsets.is_empty();
        if current_offsets
            .difference(&previous_offsets)
            .any(|offset| !added_offsets.contains(offset))
        {
            return Ok(false);
        }
    }
    if !any_added {
        return Ok(false);
    }

    let previous_file = object::File::parse(previous_bytes)
        .context("Failed to parse previous dynamic relocation addition input")?;
    let current_file = object::File::parse(current_bytes)
        .context("Failed to parse current dynamic relocation addition input")?;
    let previous_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.previous.section_index as usize))
        .collect::<HashSet<_>>();
    let current_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.current.section_index as usize))
        .collect::<HashSet<_>>();

    let mut previous_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in previous_file.sections() {
        if previous_patch_indices.contains(&section.index()) {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_dynamic_relocation_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read previous dynamic relocation addition section data")?;
        previous_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    let mut current_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in current_file.sections() {
        if current_patch_indices.contains(&section.index()) {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_dynamic_relocation_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read current dynamic relocation addition section data")?;
        current_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    Ok(previous_sections_by_name == current_sections_by_name)
}

fn object_diff_allows_fde_removal(
    previous_bytes: &[u8],
    current_bytes: &[u8],
    input_file_path: &str,
    matched_sections: &[MatchedPatchSection],
    eh_frame_patches: &[FdeRelocationPatch],
) -> Result<bool> {
    let removed_fdes = eh_frame_patches
        .iter()
        .filter_map(|patch| match &patch.eh_frame_hdr_change {
            Some(EhFrameHdrChange::Remove(fde)) => Some(fde),
            Some(EhFrameHdrChange::Adjust(_)) | Some(EhFrameHdrChange::Add(_)) => None,
            None => None,
        })
        .collect::<Vec<_>>();
    if removed_fdes.is_empty()
        || eh_frame_patches
            .iter()
            .any(|patch| matches!(patch.eh_frame_hdr_change, Some(EhFrameHdrChange::Add(_))))
        || removed_fdes
            .iter()
            .any(|fde| fde.input_file != input_file_path || fde.input != input_file_path)
        || matched_sections.iter().any(|section| {
            section.previous.input != input_file_path || section.current.input != input_file_path
        })
    {
        return Ok(false);
    }

    let previous_file = object::File::parse(previous_bytes)
        .context("Failed to parse previous FDE removal input")?;
    let current_file =
        object::File::parse(current_bytes).context("Failed to parse current FDE removal input")?;
    let previous_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.previous.section_index as usize))
        .collect::<HashSet<_>>();
    let current_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.current.section_index as usize))
        .collect::<HashSet<_>>();
    let previous_removed_target_indices = removed_fdes
        .iter()
        .map(|fde| object::SectionIndex(fde.section_index as usize))
        .collect::<HashSet<_>>();

    let mut previous_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in previous_file.sections() {
        if previous_patch_indices.contains(&section.index())
            || previous_removed_target_indices.contains(&section.index())
        {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_fde_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read previous FDE removal section data")?;
        previous_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    let mut current_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in current_file.sections() {
        if current_patch_indices.contains(&section.index()) {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_fde_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read current FDE removal section data")?;
        current_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    Ok(previous_sections_by_name == current_sections_by_name)
}

fn object_diff_allows_fde_addition(
    previous_bytes: &[u8],
    current_bytes: &[u8],
    input_file_path: &str,
    matched_sections: &[MatchedPatchSection],
    added_fdes: &[FdeAddCandidate],
) -> Result<bool> {
    if added_fdes.is_empty()
        || added_fdes
            .iter()
            .any(|fde| fde.input_file != input_file_path || fde.input != input_file_path)
        || matched_sections.iter().any(|section| {
            section.previous.input != input_file_path || section.current.input != input_file_path
        })
    {
        return Ok(false);
    }

    let previous_file = object::File::parse(previous_bytes)
        .context("Failed to parse previous FDE addition input")?;
    let current_file =
        object::File::parse(current_bytes).context("Failed to parse current FDE addition input")?;
    let previous_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.previous.section_index as usize))
        .collect::<HashSet<_>>();
    let current_patch_indices = matched_sections
        .iter()
        .map(|section| object::SectionIndex(section.current.section_index as usize))
        .collect::<HashSet<_>>();

    let mut previous_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in previous_file.sections() {
        if previous_patch_indices.contains(&section.index()) {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_fde_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read previous FDE addition section data")?;
        previous_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    let mut current_sections_by_name = HashMap::<String, Vec<Vec<u8>>>::new();
    for section in current_file.sections() {
        if current_patch_indices.contains(&section.index()) {
            continue;
        }
        let name = section.name().unwrap_or_default();
        if section_name_is_metadata_for_fde_removal(name) {
            continue;
        }
        let data = section
            .data()
            .context("Failed to read current FDE addition section data")?;
        current_sections_by_name
            .entry(name.to_owned())
            .or_default()
            .push(data.to_vec());
    }

    Ok(previous_sections_by_name == current_sections_by_name)
}

fn section_name_is_metadata_for_dynamic_relocation_removal(name: &str) -> bool {
    name.is_empty()
        || name.starts_with(".rela")
        || name.starts_with(".rel")
        || matches!(
            name,
            ".symtab" | ".strtab" | ".shstrtab" | ".comment" | ".note.GNU-stack" | ".llvm_addrsig"
        )
}

fn section_name_is_metadata_for_fde_removal(name: &str) -> bool {
    name.is_empty()
        || name.starts_with(".rela")
        || name.starts_with(".rel")
        || name == ".eh_frame"
        || matches!(
            name,
            ".symtab" | ".strtab" | ".shstrtab" | ".comment" | ".note.GNU-stack" | ".llvm_addrsig"
        )
}

#[cfg(test)]
fn fde_patch_input_ranges_for_input<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a FdeRecord>,
) -> Result<Vec<std::ops::Range<usize>>> {
    fde_patch_input_ranges_for_input_with_lookup(
        bytes,
        input_file_path,
        records,
        PatchInputLookup::MatchArchiveMember,
    )
}

fn fde_patch_input_ranges_for_current_records<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a FdeRecord>,
) -> Result<Vec<std::ops::Range<usize>>> {
    fde_patch_input_ranges_for_input_with_lookup(
        bytes,
        input_file_path,
        records,
        PatchInputLookup::CurrentRecordedRange,
    )
}

fn fde_patch_input_ranges_for_input_with_lookup<'a>(
    bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a FdeRecord>,
    lookup: PatchInputLookup,
) -> Result<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::new();
    let mut records_by_input = HashMap::<&str, Vec<&FdeRecord>>::new();
    for record in records {
        records_by_input
            .entry(record.input.as_str())
            .or_default()
            .push(record);
    }

    for (input_ref, records) in records_by_input {
        let Some(input_bytes) =
            patch_input_bytes_with_lookup(bytes, input_file_path, input_ref, lookup)?
        else {
            continue;
        };
        ranges.extend(fde_patch_input_ranges_for_input_bytes(
            input_bytes.bytes,
            input_bytes.file_offset,
            records,
        )?);
    }
    Ok(ranges)
}

fn fde_patch_input_ranges_for_input_bytes(
    bytes: &[u8],
    file_offset: usize,
    records: Vec<&FdeRecord>,
) -> Result<Vec<std::ops::Range<usize>>> {
    let Some(section_headers) = elf_section_headers(bytes) else {
        return Ok(Vec::new());
    };
    let file = object::File::parse(bytes).context("Failed to parse .eh_frame relocation input")?;
    let mut ranges = Vec::new();
    for record in records {
        let relocation_sizes =
            eh_frame_relocation_sizes(&file, record.eh_frame_section_index, record)?;
        let Some(fde_range) = fde_input_range(bytes.len(), &section_headers, record) else {
            continue;
        };
        let Some(entries) =
            rela_entries_for_section(bytes, &section_headers, record.eh_frame_section_index)
        else {
            continue;
        };
        let mut record_ranges = Vec::with_capacity(1);
        record_ranges.push(file_offset + fde_range.start..file_offset + fde_range.end);
        let mut supported = true;
        for entry in entries
            .into_iter()
            .filter(|entry| fde_contains_relocation(record, entry.offset))
        {
            if !relocation_sizes.contains_key(&entry.offset) {
                supported = false;
                break;
            }
            record_ranges
                .push(file_offset + entry.addend_range.start..file_offset + entry.addend_range.end);
        }
        if supported {
            ranges.extend(record_ranges);
        }
    }
    Ok(ranges)
}

fn fde_relocation_patches_for_input<'a>(
    current_bytes: &[u8],
    previous_bytes: &[u8],
    input_file_path: &str,
    records: impl IntoIterator<Item = &'a FdeRecord>,
) -> Result<Vec<FdeRelocationPatch>> {
    let records = records.into_iter().collect::<Vec<_>>();
    let mut patches = Vec::new();
    let mut records_by_input = HashMap::<&str, Vec<&FdeRecord>>::new();
    for record in records {
        records_by_input
            .entry(record.input.as_str())
            .or_default()
            .push(record);
    }

    for (input_ref, records) in records_by_input {
        let Some(current_input_bytes) =
            patch_input_bytes(current_bytes, input_file_path, input_ref)?
        else {
            continue;
        };
        let Some(previous_input_bytes) =
            patch_input_bytes(previous_bytes, input_file_path, input_ref)?
        else {
            continue;
        };
        patches.extend(fde_relocation_patches_for_input_bytes(
            current_input_bytes.bytes,
            current_input_bytes.file_offset,
            previous_input_bytes.bytes,
            records,
        )?);
    }
    Ok(patches)
}

fn added_fde_candidates_for_input<'a>(
    current_bytes: &[u8],
    previous_bytes: &[u8],
    input_file_path: &str,
    matched_sections: &[MatchedPatchSection],
    records: impl IntoIterator<Item = &'a FdeRecord>,
) -> Result<Vec<FdeAddCandidate>> {
    let records = records.into_iter().collect::<Vec<_>>();
    let mut records_by_input = HashMap::<&str, Vec<&FdeRecord>>::new();
    for record in records {
        records_by_input
            .entry(record.input.as_str())
            .or_default()
            .push(record);
    }

    let mut candidates = Vec::new();
    for (input_ref, records) in records_by_input {
        let Some(current_input_bytes) =
            patch_input_bytes(current_bytes, input_file_path, input_ref)?
        else {
            continue;
        };
        let Some(previous_input_bytes) =
            patch_input_bytes(previous_bytes, input_file_path, input_ref)?
        else {
            continue;
        };
        candidates.extend(added_fde_candidates_for_input_bytes(
            current_input_bytes.bytes,
            current_input_bytes.file_offset,
            previous_input_bytes.bytes,
            input_file_path,
            input_ref,
            matched_sections,
            records,
        )?);
    }
    Ok(candidates)
}

fn added_fde_candidates_for_input_bytes(
    current_bytes: &[u8],
    current_file_offset: usize,
    previous_bytes: &[u8],
    input_file_path: &str,
    input_ref: &str,
    matched_sections: &[MatchedPatchSection],
    records: Vec<&FdeRecord>,
) -> Result<Vec<FdeAddCandidate>> {
    let Some(current_section_headers) = elf_section_headers(current_bytes) else {
        return Ok(Vec::new());
    };
    let current_file =
        object::File::parse(current_bytes).context("Failed to parse current FDE addition input")?;
    let previous_file = object::File::parse(previous_bytes)
        .context("Failed to parse previous FDE addition input")?;
    let current_eh_frame_section_index = current_file
        .section_by_name(".eh_frame")
        .map(|section| section.index().0 as u32);

    let mut matched_current_fdes = HashSet::<(u32, u64)>::new();
    let mut cie_reference_fde_output_offsets = HashMap::<u64, u64>::new();
    for record in records {
        let Some(current_record) = current_fde_record_for_previous_record(
            &previous_file,
            &current_file,
            record,
            current_eh_frame_section_index,
        )?
        else {
            continue;
        };
        let mut current_record = current_record;
        let current_input_offset = fde_input_range_for_target_section_at_offset(
            current_bytes,
            &current_section_headers,
            current_record.eh_frame_section_index,
            current_record.section_index,
            current_record.input_offset,
        )
        .or_else(|| {
            fde_input_range_for_target_section(
                current_bytes,
                &current_section_headers,
                current_record.eh_frame_section_index,
                current_record.section_index,
            )
        })
        .map_or(current_record.input_offset, |(_, input_offset)| {
            input_offset
        });
        current_record.input_offset = current_input_offset;
        matched_current_fdes.insert((current_record.section_index, current_input_offset));
        if let Some(current_fde_range) = fde_input_range(
            current_bytes.len(),
            &current_section_headers,
            &current_record,
        ) && let Some(cie_input_offset) = fde_cie_input_offset(
            &current_bytes[current_fde_range],
            current_record.input_offset,
        ) {
            cie_reference_fde_output_offsets
                .entry(cie_input_offset)
                .or_insert(record.output_offset);
        }
    }

    let target_output_offsets = matched_sections
        .iter()
        .filter(|section| section.current.input == input_ref)
        .map(|section| (section.current.section_index, section.current.output_offset))
        .collect::<HashMap<_, _>>();

    let mut candidates = Vec::new();
    for candidate in fde_candidates_for_input_bytes(current_bytes, current_file_offset)? {
        if matched_current_fdes.contains(&(candidate.target_section_index, candidate.input_offset))
        {
            continue;
        }
        let Some(target_output_offset) = target_output_offsets
            .get(&candidate.target_section_index)
            .copied()
        else {
            continue;
        };
        let Some(cie_reference_fde_output_offset) = cie_reference_fde_output_offsets
            .get(&candidate.cie_input_offset)
            .copied()
        else {
            continue;
        };
        candidates.push(FdeAddCandidate {
            target_output_offset,
            cie_reference_fde_output_offset,
            input_file: input_file_path.to_owned(),
            input: input_ref.to_owned(),
            ..candidate
        });
    }
    Ok(candidates)
}

fn fde_candidates_for_input_bytes(
    bytes: &[u8],
    file_offset: usize,
) -> Result<Vec<FdeAddCandidate>> {
    let Some(section_headers) = elf_section_headers(bytes) else {
        return Ok(Vec::new());
    };
    let file = object::File::parse(bytes).context("Failed to parse FDE addition input")?;
    let Some(eh_frame_section) = file.section_by_name(".eh_frame") else {
        return Ok(Vec::new());
    };
    let eh_frame_section_index = eh_frame_section.index().0 as u32;
    let data = eh_frame_section
        .data()
        .context("Failed to read FDE addition .eh_frame data")?;
    let Some(section_header) = section_headers.get(eh_frame_section_index as usize) else {
        return Ok(Vec::new());
    };
    let section_start = usize::try_from(section_header.sh_offset)
        .context("FDE addition .eh_frame offset is too large")?;
    let relas = rela_entries_for_section(bytes, &section_headers, eh_frame_section_index)
        .unwrap_or_default();

    let mut candidates = Vec::new();
    let mut offset = 0usize;
    while offset + 8 <= data.len() {
        let length = usize::try_from(
            read_u32_le(&data[offset..offset + 4])
                .context("FDE addition .eh_frame entry length is truncated")?,
        )
        .context("FDE addition .eh_frame entry length is too large")?;
        if length == 0 {
            break;
        }
        let Some(size) = length.checked_add(4) else {
            break;
        };
        let Some(next_offset) = offset.checked_add(size) else {
            break;
        };
        if next_offset > data.len() {
            break;
        }
        let entry = &data[offset..next_offset];
        let cie_id =
            read_u32_le(&entry[4..8]).context("FDE addition .eh_frame CIE pointer is truncated")?;
        if cie_id != 0 {
            let input_offset = offset as u64;
            let pc_begin_offset = input_offset + crate::elf::FDE_PC_BEGIN_OFFSET as u64;
            let relas_in_fde = relas
                .iter()
                .filter(|entry| {
                    entry.offset >= input_offset && entry.offset < input_offset + size as u64
                })
                .collect::<Vec<_>>();
            let Some(pc_begin_rela) = relas_in_fde
                .iter()
                .find(|entry| entry.offset == pc_begin_offset)
            else {
                offset = next_offset;
                continue;
            };
            if relas_in_fde.len() != 1 {
                offset = next_offset;
                continue;
            }
            let Some((target_section_index, target_section_offset, pc_begin_range)) =
                fde_pc_begin_target(&file, eh_frame_section.index(), pc_begin_rela)?
            else {
                offset = next_offset;
                continue;
            };
            let cie_pointer_pos = input_offset + 4;
            let Some(cie_input_offset) = cie_pointer_pos.checked_sub(u64::from(cie_id)) else {
                offset = next_offset;
                continue;
            };
            let entry_start = section_start + offset;
            candidates.push(FdeAddCandidate {
                input_ranges: std::iter::once(
                    file_offset + entry_start..file_offset + section_start + next_offset,
                )
                .chain(relas_in_fde.iter().map(|entry| {
                    file_offset + entry.addend_range.start..file_offset + entry.addend_range.end
                }))
                .collect(),
                input_file: String::new(),
                input: String::new(),
                target_section_index: target_section_index.0 as u32,
                eh_frame_section_index,
                input_offset,
                target_section_offset,
                target_output_offset: 0,
                fde_data: entry.to_vec(),
                pc_begin_range,
                cie_input_offset,
                cie_reference_fde_output_offset: 0,
            });
        }
        offset = next_offset;
    }
    Ok(candidates)
}

fn fde_pc_begin_target(
    file: &object::File<'_>,
    eh_frame_section_index: object::SectionIndex,
    entry: &RelaPatchEntry,
) -> Result<Option<(object::SectionIndex, u64, std::ops::Range<usize>)>> {
    let eh_frame = file
        .section_by_index(eh_frame_section_index)
        .context("Missing FDE addition .eh_frame section")?;
    let relocation = eh_frame
        .relocations()
        .find_map(|(offset, relocation)| (offset == entry.offset).then_some(relocation));
    let Some(relocation) = relocation else {
        return Ok(None);
    };
    if relocation.kind() != object::RelocationKind::Relative
        || relocation.encoding() != object::RelocationEncoding::Generic
        || relocation.size() != 32
        || relocation.has_implicit_addend()
    {
        return Ok(None);
    }
    let pc_begin_range = crate::elf::FDE_PC_BEGIN_OFFSET..crate::elf::FDE_PC_BEGIN_OFFSET + 4;

    match relocation.target() {
        object::RelocationTarget::Symbol(symbol_index) => {
            let symbol = file
                .symbol_by_index(symbol_index)
                .context("Missing FDE addition pc-begin symbol")?;
            let Some(section_index) = symbol.section_index() else {
                return Ok(None);
            };
            let Some(offset) = i128::from(symbol.address())
                .checked_add(i128::from(relocation.addend()))
                .and_then(|value| u64::try_from(value).ok())
            else {
                return Ok(None);
            };
            Ok(Some((section_index, offset, pc_begin_range)))
        }
        object::RelocationTarget::Section(section_index) => {
            let Some(offset) = u64::try_from(relocation.addend()).ok() else {
                return Ok(None);
            };
            Ok(Some((section_index, offset, pc_begin_range)))
        }
        _ => Ok(None),
    }
}

fn fde_add_patches_for_output(
    output: &[u8],
    candidates: &[FdeAddCandidate],
    existing_fdes: &[FdeRecord],
) -> Result<std::result::Result<Vec<ResolvedFdeAdd>, String>> {
    if candidates.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    let file = object::File::parse(output)
        .context("Failed to parse output for incremental FDE addition")?;
    let Some(eh_frame) = file.section_by_name(".eh_frame") else {
        return Ok(Err(
            "output has no .eh_frame for incremental FDE addition".to_owned()
        ));
    };
    let Some((eh_frame_offset, eh_frame_size)) = eh_frame.file_range() else {
        return Ok(Err("output .eh_frame has no file range".to_owned()));
    };
    let Ok(eh_frame_start) = usize::try_from(eh_frame_offset) else {
        return Ok(Err("output .eh_frame offset is too large".to_owned()));
    };
    let Ok(eh_frame_size) = usize::try_from(eh_frame_size) else {
        return Ok(Err("output .eh_frame size is too large".to_owned()));
    };
    let Some(eh_frame_end) = eh_frame_start.checked_add(eh_frame_size) else {
        return Ok(Err("output .eh_frame range overflowed".to_owned()));
    };
    let Some(eh_frame_bytes) = output.get(eh_frame_start..eh_frame_end) else {
        return Ok(Err("output .eh_frame range is out of bounds".to_owned()));
    };
    let Some(terminator_offset_in_section) = eh_frame_terminator_offset(eh_frame_bytes) else {
        return Ok(Err(
            "output .eh_frame has no terminator for FDE addition".to_owned()
        ));
    };

    let eh_frame_hdr_address = file
        .section_by_name(".eh_frame_hdr")
        .map(|section| section.address());
    let Some(mut output_offset) = eh_frame_offset.checked_add(terminator_offset_in_section as u64)
    else {
        return Ok(Err(
            "output .eh_frame terminator offset overflowed".to_owned()
        ));
    };
    let mut resolved = Vec::new();
    for candidate in candidates {
        let target_section_address =
            match output_address_for_file_offset(&file, candidate.target_output_offset) {
                Some(address) => address,
                None => {
                    return Ok(Err(
                        "could not resolve FDE addition target output address".to_owned()
                    ));
                }
            };
        let Some(target_address) =
            target_section_address.checked_add(candidate.target_section_offset)
        else {
            return Ok(Err(
                "FDE addition target output address overflowed".to_owned()
            ));
        };
        let Some(fde_address) = output_offset
            .checked_sub(eh_frame_offset)
            .and_then(|section_offset| eh_frame.address().checked_add(section_offset))
        else {
            return Ok(Err("FDE addition output address overflowed".to_owned()));
        };
        let output_cie_offset =
            match output_cie_offset_for_fde(output, candidate.cie_reference_fde_output_offset) {
                Some(offset) => offset,
                None => {
                    return Ok(Err(
                        "could not resolve FDE addition CIE output offset".to_owned()
                    ));
                }
            };
        let Some(cie_pointer) = output_offset
            .checked_add(4)
            .and_then(|pointer_pos| pointer_pos.checked_sub(output_cie_offset))
            .and_then(|value| u32::try_from(value).ok())
        else {
            return Ok(Err("FDE addition CIE pointer overflowed".to_owned()));
        };

        let mut data = candidate.fde_data.clone();
        data[4..8].copy_from_slice(&cie_pointer.to_le_bytes());
        let field_address = fde_address + crate::elf::FDE_PC_BEGIN_OFFSET as u64;
        let Some(pc_begin) = i128::from(target_address)
            .checked_sub(i128::from(field_address))
            .and_then(|value| i32::try_from(value).ok())
        else {
            return Ok(Err("FDE addition pc-begin relocation overflowed".to_owned()));
        };
        let Some(pc_begin_range) = data.get_mut(candidate.pc_begin_range.clone()) else {
            return Ok(Err(
                "FDE addition pc-begin range is out of bounds".to_owned()
            ));
        };
        pc_begin_range.copy_from_slice(&pc_begin.to_le_bytes());

        let record = FdeRecord {
            input_file: candidate.input_file.clone().into(),
            input: candidate.input.clone().into(),
            section_index: candidate.target_section_index,
            eh_frame_section_index: candidate.eh_frame_section_index,
            input_offset: candidate.input_offset,
            output_offset,
            size: data.len() as u64,
        };
        let eh_frame_hdr_change = if let Some(eh_frame_hdr_address) = eh_frame_hdr_address {
            let Some(frame_ptr) = i128::from(target_address)
                .checked_sub(i128::from(eh_frame_hdr_address))
                .and_then(|value| i32::try_from(value).ok())
            else {
                return Ok(Err(
                    "FDE addition .eh_frame_hdr frame pointer overflowed".to_owned()
                ));
            };
            let Some(frame_info_ptr) = i128::from(fde_address)
                .checked_sub(i128::from(eh_frame_hdr_address))
                .and_then(|value| i32::try_from(value).ok())
            else {
                return Ok(Err(
                    "FDE addition .eh_frame_hdr frame-info pointer overflowed".to_owned(),
                ));
            };
            Some(EhFrameHdrChange::Add(EhFrameHdrEntryPatch {
                frame_ptr,
                frame_info_ptr,
            }))
        } else {
            None
        };

        resolved.push(ResolvedFdeAdd {
            patch: SectionPatch {
                output_offset,
                size: data.len() as u64,
                data,
                deferred_relocation: None,
                preserve_ranges: Vec::new(),
                adjustments: Vec::new(),
            },
            record,
            eh_frame_hdr_change,
        });
        let Some(next_output_offset) =
            output_offset.checked_add(resolved.last().unwrap().patch.size)
        else {
            return Ok(Err("FDE addition output offset overflowed".to_owned()));
        };
        output_offset = next_output_offset;
    }

    let Some(required_end) = output_offset.checked_add(4) else {
        return Ok(Err("FDE addition terminator offset overflowed".to_owned()));
    };
    if required_end > eh_frame_offset + eh_frame_size as u64 {
        return Ok(Err("no free .eh_frame space for FDE addition".to_owned()));
    }
    if !eh_frame_bytes[terminator_offset_in_section..required_end as usize - eh_frame_start]
        .iter()
        .all(|byte| *byte == 0)
    {
        return Ok(Err("FDE addition .eh_frame space is not empty".to_owned()));
    }

    let mut combined_data = Vec::new();
    for resolved in &resolved {
        combined_data.extend(&resolved.patch.data);
    }
    combined_data.extend(0_u32.to_le_bytes());
    if let Some(first) = resolved.first_mut() {
        first.patch = SectionPatch {
            output_offset: eh_frame_offset + terminator_offset_in_section as u64,
            size: combined_data.len() as u64,
            data: combined_data,
            deferred_relocation: None,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        };
    }
    for extra in resolved.iter_mut().skip(1) {
        extra.patch = SectionPatch {
            output_offset: extra.record.output_offset,
            size: 0,
            data: Vec::new(),
            deferred_relocation: None,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        };
    }

    let mut previous = existing_fdes.iter().cloned().collect::<HashSet<_>>();
    for resolved_fde in &resolved {
        if !previous.insert(resolved_fde.record.clone()) {
            return Ok(Err("duplicate incremental FDE addition record".to_owned()));
        }
    }
    Ok(Ok(resolved))
}

fn eh_frame_terminator_offset(eh_frame: &[u8]) -> Option<usize> {
    let mut offset = 0usize;
    while offset + 4 <= eh_frame.len() {
        let length = usize::try_from(read_u32_le(eh_frame.get(offset..offset + 4)?)?).ok()?;
        if length == 0 {
            return Some(offset);
        }
        offset = offset.checked_add(length.checked_add(4)?)?;
    }
    None
}

fn output_cie_offset_for_fde(output: &[u8], fde_output_offset: u64) -> Option<u64> {
    let pointer_pos = usize::try_from(fde_output_offset.checked_add(4)?).ok()?;
    let cie_pointer = u64::from(read_u32_le(output.get(pointer_pos..pointer_pos + 4)?)?);
    fde_output_offset.checked_add(4)?.checked_sub(cie_pointer)
}

fn output_address_for_file_offset(file: &object::File<'_>, file_offset: u64) -> Option<u64> {
    for section in file.sections() {
        let Some((section_offset, section_size)) = section.file_range() else {
            continue;
        };
        if file_offset >= section_offset
            && file_offset < section_offset.saturating_add(section_size)
        {
            return Some(section.address() + (file_offset - section_offset));
        }
    }
    None
}

fn fde_relocation_patches_for_input_bytes(
    current_bytes: &[u8],
    current_file_offset: usize,
    previous_bytes: &[u8],
    records: Vec<&FdeRecord>,
) -> Result<Vec<FdeRelocationPatch>> {
    let Some(current_section_headers) = elf_section_headers(current_bytes) else {
        return Ok(Vec::new());
    };
    let Some(previous_section_headers) = elf_section_headers(previous_bytes) else {
        return Ok(Vec::new());
    };
    let current_file = object::File::parse(current_bytes)
        .context("Failed to parse changed .eh_frame relocation input")?;
    let previous_file = object::File::parse(previous_bytes)
        .context("Failed to parse previous .eh_frame relocation input")?;
    let current_eh_frame_section_index = current_file
        .section_by_name(".eh_frame")
        .map(|section| section.index().0 as u32);

    let mut patches = Vec::new();
    for record in records {
        let Some(current_record) = current_fde_record_for_previous_record(
            &previous_file,
            &current_file,
            record,
            current_eh_frame_section_index,
        )?
        else {
            patches.push(FdeRelocationPatch {
                input_ranges: Vec::new(),
                patch: None,
                eh_frame_hdr_change: Some(EhFrameHdrChange::Remove(record.clone())),
                record_update: None,
            });
            continue;
        };
        let Some((current_fde_range, current_fde_input_offset)) =
            fde_input_range_for_target_section_at_offset(
                current_bytes,
                &current_section_headers,
                current_record.eh_frame_section_index,
                current_record.section_index,
                current_record.input_offset,
            )
            .or_else(|| {
                fde_input_range_for_target_section(
                    current_bytes,
                    &current_section_headers,
                    current_record.eh_frame_section_index,
                    current_record.section_index,
                )
            })
            .or_else(|| {
                fde_input_range(
                    current_bytes.len(),
                    &current_section_headers,
                    &current_record,
                )
                .map(|range| (range, current_record.input_offset))
            })
        else {
            patches.push(FdeRelocationPatch {
                input_ranges: Vec::new(),
                patch: None,
                eh_frame_hdr_change: Some(EhFrameHdrChange::Remove(record.clone())),
                record_update: None,
            });
            continue;
        };
        let mut current_record = current_record;
        current_record.input_offset = current_fde_input_offset;
        let Some(previous_fde_range) =
            fde_input_range(previous_bytes.len(), &previous_section_headers, record)
        else {
            continue;
        };
        let current_fde_data = &current_bytes[current_fde_range.clone()];
        let previous_fde_data = &previous_bytes[previous_fde_range];
        if current_fde_data.get(4..8) != previous_fde_data.get(4..8)
            && !fde_bytes_match_ignoring_cie_pointer(current_fde_data, previous_fde_data)
        {
            patches.push(FdeRelocationPatch {
                input_ranges: Vec::new(),
                patch: None,
                eh_frame_hdr_change: None,
                record_update: None,
            });
            continue;
        }
        let current_entries = rela_entries_for_section(
            current_bytes,
            &current_section_headers,
            current_record.eh_frame_section_index,
        );
        let previous_entries = rela_entries_for_section(
            previous_bytes,
            &previous_section_headers,
            record.eh_frame_section_index,
        );
        let Some(current_entries) = current_entries else {
            continue;
        };
        let Some(previous_entries) = previous_entries else {
            continue;
        };
        let current_entries = current_entries
            .into_iter()
            .filter(|entry| fde_contains_relocation(&current_record, entry.offset))
            .collect::<Vec<_>>();
        let previous_entries = previous_entries
            .into_iter()
            .filter(|entry| fde_contains_relocation(record, entry.offset))
            .collect::<Vec<_>>();
        if current_entries.len() != previous_entries.len() {
            continue;
        }
        let relocation_sizes = eh_frame_relocation_sizes(
            &current_file,
            current_record.eh_frame_section_index,
            &current_record,
        )?;

        let mut input_ranges = Vec::with_capacity(1);
        input_ranges.push(
            current_file_offset + current_fde_range.start
                ..current_file_offset + current_fde_range.end,
        );
        let mut preserve_ranges = Vec::with_capacity(1);
        preserve_ranges.push(4..8);
        let mut adjustments = Vec::new();
        let mut eh_frame_hdr_change = None;
        for (current, previous) in current_entries.iter().zip(&previous_entries) {
            let Some(current_field_offset) =
                current.offset.checked_sub(current_record.input_offset)
            else {
                input_ranges.clear();
                preserve_ranges.clear();
                adjustments.clear();
                eh_frame_hdr_change = None;
                break;
            };
            let Some(previous_field_offset) = previous.offset.checked_sub(record.input_offset)
            else {
                input_ranges.clear();
                preserve_ranges.clear();
                adjustments.clear();
                eh_frame_hdr_change = None;
                break;
            };
            if current_field_offset != previous_field_offset || current.info != previous.info {
                input_ranges.clear();
                preserve_ranges.clear();
                adjustments.clear();
                eh_frame_hdr_change = None;
                break;
            }
            let Some(field_size) = relocation_sizes.get(&current.offset).copied() else {
                input_ranges.clear();
                preserve_ranges.clear();
                adjustments.clear();
                eh_frame_hdr_change = None;
                break;
            };
            input_ranges.push(
                current_file_offset + current.addend_range.start
                    ..current_file_offset + current.addend_range.end,
            );
            let field_start = usize::try_from(current_field_offset)
                .context("Incremental .eh_frame relocation offset is too large")?;
            let field_end = field_start
                .checked_add(usize::from(field_size))
                .context("Incremental .eh_frame relocation range overflow")?;
            if field_end > record.size as usize {
                input_ranges.clear();
                preserve_ranges.clear();
                adjustments.clear();
                eh_frame_hdr_change = None;
                break;
            }
            preserve_ranges.push(field_start..field_end);
            let Some(addend_delta) = current.addend.checked_sub(previous.addend) else {
                input_ranges.clear();
                preserve_ranges.clear();
                adjustments.clear();
                eh_frame_hdr_change = None;
                break;
            };
            if addend_delta == 0 {
                continue;
            }
            if field_start == crate::elf::FDE_PC_BEGIN_OFFSET {
                eh_frame_hdr_change = Some(EhFrameHdrChange::Adjust(EhFrameHdrDelta {
                    fde_output_offset: record.output_offset,
                    frame_ptr_delta: addend_delta,
                }));
            }
            adjustments.push(PatchAdjustment {
                range: field_start..field_end,
                addend_delta,
            });
        }
        preserve_ranges.sort_by_key(|range| (range.start, range.end));
        preserve_ranges.dedup_by(|left, right| left.start == right.start && left.end == right.end);
        let needs_patch = !input_ranges.is_empty()
            && (current_fde_data != previous_fde_data || !adjustments.is_empty());
        let record_update =
            (!input_ranges.is_empty() && current_record != *record).then(|| FdeRecordUpdate {
                previous: record.clone(),
                current: current_record.clone(),
            });
        patches.push(FdeRelocationPatch {
            input_ranges,
            patch: needs_patch.then(|| SectionPatch {
                output_offset: record.output_offset,
                size: record.size,
                data: current_fde_data.to_vec(),
                deferred_relocation: None,
                preserve_ranges,
                adjustments,
            }),
            eh_frame_hdr_change,
            record_update,
        });
    }
    Ok(patches)
}

fn update_fde_records(fdes: &mut Vec<FdeRecord>, updates: Vec<FdeRecordUpdate>) {
    for update in updates {
        if let Some(record) = fdes.iter_mut().find(|record| **record == update.previous) {
            *record = update.current;
        } else {
            fdes.push(update.current);
        }
    }
    fdes.sort();
    fdes.dedup();
}

fn eh_frame_hdr_patches_for_fde_changes(
    output: &[u8],
    changes: &[EhFrameHdrChange],
) -> Result<std::result::Result<Vec<SectionPatch>, String>> {
    if changes.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    let has_removal = changes
        .iter()
        .any(|change| matches!(change, EhFrameHdrChange::Remove(_)));

    let file = object::File::parse(output)
        .context("Failed to parse output for incremental .eh_frame_hdr patching")?;
    let Some(eh_frame_hdr) = file.section_by_name(".eh_frame_hdr") else {
        if has_removal {
            return Ok(Err(
                "output has no .eh_frame_hdr for incremental FDE removal".to_owned(),
            ));
        }
        return Ok(Ok(Vec::new()));
    };
    let Some(eh_frame) = file.section_by_name(".eh_frame") else {
        return Ok(Err(
            "output has .eh_frame_hdr but no .eh_frame section".to_owned()
        ));
    };
    let Some((eh_frame_offset, eh_frame_size)) = eh_frame.file_range() else {
        return Ok(Err("output .eh_frame has no file range".to_owned()));
    };
    let Some((eh_frame_hdr_offset, eh_frame_hdr_size)) = eh_frame_hdr.file_range() else {
        return Ok(Err("output .eh_frame_hdr has no file range".to_owned()));
    };

    let header_size = std::mem::size_of::<crate::elf::EhFrameHdr>();
    let entry_size = std::mem::size_of::<crate::elf::EhFrameHdrEntry>();
    let Ok(eh_frame_hdr_start) = usize::try_from(eh_frame_hdr_offset) else {
        return Ok(Err("output .eh_frame_hdr offset is too large".to_owned()));
    };
    let Ok(eh_frame_hdr_size) = usize::try_from(eh_frame_hdr_size) else {
        return Ok(Err("output .eh_frame_hdr size is too large".to_owned()));
    };
    if eh_frame_hdr_size < header_size
        || !(eh_frame_hdr_size - header_size).is_multiple_of(entry_size)
    {
        return Ok(Err("output .eh_frame_hdr has an invalid size".to_owned()));
    }
    let Some(eh_frame_hdr_end) = eh_frame_hdr_start.checked_add(eh_frame_hdr_size) else {
        return Ok(Err("output .eh_frame_hdr range overflowed".to_owned()));
    };
    let Some(eh_frame_hdr_bytes) = output.get(eh_frame_hdr_start..eh_frame_hdr_end) else {
        return Ok(Err("output .eh_frame_hdr range is out of bounds".to_owned()));
    };

    let Some(entry_count_bytes) = eh_frame_hdr_bytes.get(8..12) else {
        return Ok(Err(
            "output .eh_frame_hdr entry count is truncated".to_owned()
        ));
    };
    let Some(entry_count) = read_u32_le(entry_count_bytes) else {
        return Ok(Err(
            "output .eh_frame_hdr entry count is truncated".to_owned()
        ));
    };
    let Ok(entry_count) = usize::try_from(entry_count) else {
        return Ok(Err(
            "output .eh_frame_hdr entry count is too large".to_owned()
        ));
    };
    let entry_capacity = (eh_frame_hdr_size - header_size) / entry_size;
    if entry_count > entry_capacity {
        return Ok(Err(
            "output .eh_frame_hdr entry count exceeds capacity".to_owned()
        ));
    }

    let mut entries = eh_frame_hdr_bytes[header_size..]
        .chunks_exact(entry_size)
        .take(entry_count)
        .map(|entry| {
            let frame_ptr = read_i32_le(&entry[0..4])
                .ok_or_else(|| "output .eh_frame_hdr entry is truncated".to_owned())?;
            let frame_info_ptr = read_i32_le(&entry[4..8])
                .ok_or_else(|| "output .eh_frame_hdr entry is truncated".to_owned())?;
            Ok(EhFrameHdrEntryPatch {
                frame_ptr,
                frame_info_ptr,
            })
        })
        .collect::<std::result::Result<Vec<_>, String>>()?;

    let mut changed_indices = Vec::new();
    let mut removed_indices = Vec::new();
    let mut added_entries = Vec::new();
    for change in changes {
        match change {
            EhFrameHdrChange::Adjust(delta) => {
                let index = match eh_frame_hdr_index_for_fde(
                    &entries,
                    eh_frame_offset,
                    eh_frame_size,
                    eh_frame.address(),
                    eh_frame_hdr.address(),
                    delta.fde_output_offset,
                ) {
                    Ok(index) => index,
                    Err(reason) => return Ok(Err(reason)),
                };
                let Some(adjusted) = i64::from(entries[index].frame_ptr)
                    .checked_add(delta.frame_ptr_delta)
                    .and_then(|value| i32::try_from(value).ok())
                else {
                    return Ok(Err(
                        "changed .eh_frame_hdr frame pointer overflowed".to_owned()
                    ));
                };
                entries[index].frame_ptr = adjusted;
                changed_indices.push(index);
            }
            EhFrameHdrChange::Remove(fde) => {
                let index = match eh_frame_hdr_index_for_fde(
                    &entries,
                    eh_frame_offset,
                    eh_frame_size,
                    eh_frame.address(),
                    eh_frame_hdr.address(),
                    fde.output_offset,
                ) {
                    Ok(index) => index,
                    Err(reason) => return Ok(Err(reason)),
                };
                removed_indices.push(index);
            }
            EhFrameHdrChange::Add(entry) => {
                added_entries.push(entry.clone());
            }
        }
    }

    if !entries
        .windows(2)
        .all(|window| window[0].frame_ptr <= window[1].frame_ptr)
    {
        return Ok(Err(
            "changed .eh_frame_hdr entries would no longer be sorted".to_owned(),
        ));
    }

    changed_indices.sort_unstable();
    changed_indices.dedup();
    removed_indices.sort_unstable();
    removed_indices.dedup();

    if !added_entries.is_empty() {
        for index in removed_indices.iter().rev() {
            entries.remove(*index);
        }
        if entries.len() + added_entries.len() > entry_capacity {
            return Ok(Err(
                "no free .eh_frame_hdr entries for FDE addition".to_owned()
            ));
        }
        entries.extend(added_entries);
        entries.sort_by_key(|entry| entry.frame_ptr);
        if !entries
            .windows(2)
            .all(|window| window[0].frame_ptr <= window[1].frame_ptr)
        {
            return Ok(Err(
                "changed .eh_frame_hdr entries would no longer be sorted".to_owned(),
            ));
        }
        let entry_count = u32::try_from(entries.len())
            .map_err(|_| "changed .eh_frame_hdr entry count overflowed".to_owned())?;
        let mut patches = vec![SectionPatch {
            output_offset: (eh_frame_hdr_start + 8) as u64,
            size: 4,
            data: entry_count.to_le_bytes().to_vec(),
            deferred_relocation: None,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        }];
        let mut data = Vec::with_capacity(entries.len() * entry_size);
        for entry in &entries {
            data.extend(entry.frame_ptr.to_le_bytes());
            data.extend(entry.frame_info_ptr.to_le_bytes());
        }
        data.resize(entry_capacity * entry_size, 0);
        patches.push(SectionPatch {
            output_offset: (eh_frame_hdr_start + header_size) as u64,
            size: data.len() as u64,
            data,
            deferred_relocation: None,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        });
        return Ok(Ok(patches));
    }

    if removed_indices.is_empty() {
        return Ok(Ok(changed_indices
            .into_iter()
            .map(|index| SectionPatch {
                output_offset: (eh_frame_hdr_start + header_size + index * entry_size) as u64,
                size: 4,
                data: entries[index].frame_ptr.to_le_bytes().to_vec(),
                deferred_relocation: None,
                preserve_ranges: Vec::new(),
                adjustments: Vec::new(),
            })
            .collect()));
    }

    let original_entry_count = entries.len();
    for index in removed_indices.iter().rev() {
        entries.remove(*index);
    }
    if !entries
        .windows(2)
        .all(|window| window[0].frame_ptr <= window[1].frame_ptr)
    {
        return Ok(Err(
            "changed .eh_frame_hdr entries would no longer be sorted".to_owned(),
        ));
    }

    let first_removed = *removed_indices.first().unwrap();
    let mut patches = changed_indices
        .into_iter()
        .filter(|index| *index < first_removed)
        .map(|index| SectionPatch {
            output_offset: (eh_frame_hdr_start + header_size + index * entry_size) as u64,
            size: 4,
            data: entries[index].frame_ptr.to_le_bytes().to_vec(),
            deferred_relocation: None,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        })
        .collect::<Vec<_>>();

    let entry_count = u32::try_from(entries.len())
        .map_err(|_| "changed .eh_frame_hdr entry count overflowed".to_owned())?;
    patches.push(SectionPatch {
        output_offset: (eh_frame_hdr_start + 8) as u64,
        size: 4,
        data: entry_count.to_le_bytes().to_vec(),
        deferred_relocation: None,
        preserve_ranges: Vec::new(),
        adjustments: Vec::new(),
    });

    let mut data = Vec::new();
    for entry in &entries[first_removed..] {
        data.extend(entry.frame_ptr.to_le_bytes());
        data.extend(entry.frame_info_ptr.to_le_bytes());
    }
    data.resize((original_entry_count - first_removed) * entry_size, 0);
    patches.push(SectionPatch {
        output_offset: (eh_frame_hdr_start + header_size + first_removed * entry_size) as u64,
        size: data.len() as u64,
        data,
        deferred_relocation: None,
        preserve_ranges: Vec::new(),
        adjustments: Vec::new(),
    });

    Ok(Ok(patches))
}

fn eh_frame_hdr_index_for_fde(
    entries: &[EhFrameHdrEntryPatch],
    eh_frame_offset: u64,
    eh_frame_size: u64,
    eh_frame_address: u64,
    eh_frame_hdr_address: u64,
    fde_output_offset: u64,
) -> std::result::Result<usize, String> {
    let Some(fde_offset_in_section) = fde_output_offset.checked_sub(eh_frame_offset) else {
        return Err("incremental FDE output offset is outside .eh_frame".to_owned());
    };
    if fde_offset_in_section >= eh_frame_size {
        return Err("incremental FDE output offset is outside .eh_frame".to_owned());
    }
    let Some(fde_address) = eh_frame_address.checked_add(fde_offset_in_section) else {
        return Err("incremental FDE address overflowed".to_owned());
    };
    let Ok(frame_info_ptr) =
        i32::try_from(i128::from(fde_address) - i128::from(eh_frame_hdr_address))
    else {
        return Err("incremental .eh_frame_hdr frame-info pointer overflowed".to_owned());
    };
    let mut matching_indices = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| (entry.frame_info_ptr == frame_info_ptr).then_some(index));
    let Some(index) = matching_indices.next() else {
        return Err("could not find .eh_frame_hdr entry for changed FDE".to_owned());
    };
    if matching_indices.next().is_some() {
        return Err("found multiple .eh_frame_hdr entries for changed FDE".to_owned());
    }
    Ok(index)
}

#[derive(Clone)]
struct EhFrameHdrEntryPatch {
    frame_ptr: i32,
    frame_info_ptr: i32,
}

fn eh_frame_relocation_sizes(
    file: &object::File<'_>,
    eh_frame_section_index: u32,
    record: &FdeRecord,
) -> Result<HashMap<u64, u8>> {
    let section = file
        .section_by_index(object::SectionIndex(eh_frame_section_index as usize))
        .context("Missing changed .eh_frame section")?;
    let mut sizes = HashMap::new();
    for (offset, relocation) in section.relocations() {
        if !fde_contains_relocation(record, offset)
            || relocation.has_implicit_addend()
            || relocation.encoding() != object::RelocationEncoding::Generic
            || relocation.size() == 0
            || relocation.size() % 8 != 0
        {
            continue;
        }
        sizes.insert(offset, relocation.size() / 8);
    }
    Ok(sizes)
}

fn current_fde_record_for_previous_record(
    previous_file: &object::File<'_>,
    current_file: &object::File<'_>,
    record: &FdeRecord,
    current_eh_frame_section_index: Option<u32>,
) -> Result<Option<FdeRecord>> {
    let Some(eh_frame_section_index) = current_eh_frame_section_index else {
        return Ok(None);
    };
    let mut current_record = record.clone();
    current_record.eh_frame_section_index = eh_frame_section_index;

    let previous_section = previous_file
        .section_by_index(object::SectionIndex(record.section_index as usize))
        .context("Missing previous FDE target section")?;
    let Ok(section_name) = previous_section.name() else {
        return Ok(Some(current_record));
    };
    let Some(current_section) = current_file.section_by_name(section_name) else {
        return Ok(None);
    };
    current_record.section_index = current_section.index().0 as u32;
    Ok(Some(current_record))
}

fn fde_input_range_for_target_section(
    bytes: &[u8],
    section_headers: &[ElfSectionHeader],
    eh_frame_section_index: u32,
    target_section_index: u32,
) -> Option<(std::ops::Range<usize>, u64)> {
    fde_input_range_for_target_section_matching(
        bytes,
        section_headers,
        eh_frame_section_index,
        target_section_index,
        None,
    )
}

fn fde_input_range_for_target_section_at_offset(
    bytes: &[u8],
    section_headers: &[ElfSectionHeader],
    eh_frame_section_index: u32,
    target_section_index: u32,
    target_input_offset: u64,
) -> Option<(std::ops::Range<usize>, u64)> {
    fde_input_range_for_target_section_matching(
        bytes,
        section_headers,
        eh_frame_section_index,
        target_section_index,
        Some(target_input_offset),
    )
}

fn fde_input_range_for_target_section_matching(
    bytes: &[u8],
    section_headers: &[ElfSectionHeader],
    eh_frame_section_index: u32,
    target_section_index: u32,
    target_input_offset: Option<u64>,
) -> Option<(std::ops::Range<usize>, u64)> {
    let section = section_headers.get(eh_frame_section_index as usize)?;
    let section_start = usize::try_from(section.sh_offset).ok()?;
    let section_size = usize::try_from(section.sh_size).ok()?;
    let section_end = section_start.checked_add(section_size)?;
    let entries = rela_entries_for_section(bytes, section_headers, eh_frame_section_index)?;
    let symbol_section_indices = elf_symbol_section_indices(bytes, section_headers)?;

    let mut entry_offset = 0usize;
    while entry_offset.checked_add(8)? <= section_size {
        let entry_start = section_start.checked_add(entry_offset)?;
        let length =
            usize::try_from(read_u32_le(bytes.get(entry_start..entry_start + 4)?)?).ok()?;
        if length == 0 {
            break;
        }
        let entry_size = length.checked_add(4)?;
        let entry_end_offset = entry_offset.checked_add(entry_size)?;
        let entry_end = section_start.checked_add(entry_end_offset)?;
        if entry_end > section_end {
            break;
        }
        let input_offset = u64::try_from(entry_offset).ok()?;
        if target_input_offset.is_some_and(|target| target != input_offset) {
            entry_offset = entry_end_offset;
            continue;
        }
        let pc_begin_offset = input_offset + crate::elf::FDE_PC_BEGIN_OFFSET as u64;
        let has_target_pc_begin = entries.iter().any(|entry| {
            entry.offset == pc_begin_offset
                && symbol_section_indices
                    .get(&(entry.info >> 32))
                    .is_some_and(|section_index| *section_index == target_section_index)
        });
        if has_target_pc_begin {
            return Some((entry_start..entry_end, input_offset));
        }
        entry_offset = entry_end_offset;
    }
    None
}

fn elf_symbol_section_indices(
    bytes: &[u8],
    section_headers: &[ElfSectionHeader],
) -> Option<HashMap<u64, u32>> {
    let mut sections = HashMap::new();
    for section in section_headers {
        if section.sh_type != u64::from(object::elf::SHT_SYMTAB) || section.sh_entsize < 24 {
            continue;
        }
        let start = usize::try_from(section.sh_offset).ok()?;
        let size = usize::try_from(section.sh_size).ok()?;
        let entsize = usize::try_from(section.sh_entsize).ok()?;
        let end = start.checked_add(size)?;
        let section_bytes = bytes.get(start..end)?;
        for (symbol_index, symbol) in section_bytes.chunks_exact(entsize).enumerate() {
            let section_index = read_u16_le(symbol.get(6..8)?)?;
            sections.insert(u64::try_from(symbol_index).ok()?, u32::from(section_index));
        }
        return Some(sections);
    }
    None
}

fn fde_bytes_match_ignoring_cie_pointer(current: &[u8], previous: &[u8]) -> bool {
    current.len() == previous.len()
        && current.get(..4) == previous.get(..4)
        && current.get(8..) == previous.get(8..)
}

fn fde_cie_input_offset(fde_data: &[u8], fde_input_offset: u64) -> Option<u64> {
    let cie_id = read_u32_le(fde_data.get(4..8)?)?;
    (cie_id != 0).then_some(
        fde_input_offset
            .checked_add(4)?
            .checked_sub(u64::from(cie_id))?,
    )
}

fn fde_contains_relocation(record: &FdeRecord, relocation_offset: u64) -> bool {
    relocation_offset >= record.input_offset
        && relocation_offset < record.input_offset.saturating_add(record.size)
}

fn fde_input_range(
    input_len: usize,
    section_headers: &[ElfSectionHeader],
    record: &FdeRecord,
) -> Option<std::ops::Range<usize>> {
    let section = section_headers.get(record.eh_frame_section_index as usize)?;
    let section_start = usize::try_from(section.sh_offset).ok()?;
    let section_size = usize::try_from(section.sh_size).ok()?;
    let section_end = section_start.checked_add(section_size)?;
    let start = section_start.checked_add(usize::try_from(record.input_offset).ok()?)?;
    let size = usize::try_from(record.size).ok()?;
    let end = start.checked_add(size)?;
    (end <= input_len && end <= section_end && size >= 8).then_some(start..end)
}

struct RelaPatchEntry {
    offset: u64,
    info: u64,
    addend: i64,
    addend_range: std::ops::Range<usize>,
}

fn rela_entries_for_section(
    bytes: &[u8],
    section_headers: &[ElfSectionHeader],
    section_index: u32,
) -> Option<Vec<RelaPatchEntry>> {
    let mut entries = Vec::new();
    for section in section_headers {
        if section.sh_type != u64::from(object::elf::SHT_RELA)
            || section.sh_info != u64::from(section_index)
            || section.sh_entsize != crate::elf::RELA_ENTRY_SIZE
        {
            continue;
        }
        let start = usize::try_from(section.sh_offset).ok()?;
        let size = usize::try_from(section.sh_size).ok()?;
        let end = start.checked_add(size)?;
        let section_bytes = bytes.get(start..end)?;
        for (entry_index, entry) in section_bytes
            .chunks_exact(crate::elf::RELA_ENTRY_SIZE as usize)
            .enumerate()
        {
            let entry_start =
                start.checked_add(entry_index * crate::elf::RELA_ENTRY_SIZE as usize)?;
            let addend_start = entry_start + 16;
            entries.push(RelaPatchEntry {
                offset: read_u64_le(entry.get(0..8)?)?,
                info: read_u64_le(entry.get(8..16)?)?,
                addend: read_i64_le(entry.get(16..24)?)?,
                addend_range: addend_start..addend_start + 8,
            });
        }
    }
    Some(entries)
}

fn dynamic_relocation_entry_range(
    bytes: &[u8],
    section_headers: &[ElfSectionHeader],
    record: &DynamicRelocationRecord,
) -> Result<Option<std::ops::Range<usize>>> {
    for section in section_headers {
        if section.sh_type != u64::from(object::elf::SHT_RELA)
            || section.sh_info != u64::from(record.section_index)
            || section.sh_entsize != crate::elf::RELA_ENTRY_SIZE
        {
            continue;
        }
        let start = usize::try_from(section.sh_offset)
            .context("Incremental dynamic relocation section offset is too large")?;
        let size = usize::try_from(section.sh_size)
            .context("Incremental dynamic relocation section size is too large")?;
        let end = start
            .checked_add(size)
            .context("Incremental dynamic relocation section range overflow")?;
        let Some(section_bytes) = bytes.get(start..end) else {
            continue;
        };
        for (entry_index, entry) in section_bytes
            .chunks_exact(crate::elf::RELA_ENTRY_SIZE as usize)
            .enumerate()
        {
            let Some(offset) = read_u64_le(entry.get(0..8).unwrap_or_default()) else {
                continue;
            };
            if offset == record.relocation_offset {
                let entry_start = start
                    .checked_add(entry_index * crate::elf::RELA_ENTRY_SIZE as usize)
                    .context("Incremental dynamic relocation entry range overflow")?;
                let entry_end = entry_start + crate::elf::RELA_ENTRY_SIZE as usize;
                return Ok(Some(entry_start..entry_end));
            }
        }
    }
    Ok(None)
}

struct ElfSectionHeader {
    sh_type: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_info: u64,
    sh_entsize: u64,
}

fn elf_section_headers(bytes: &[u8]) -> Option<Vec<ElfSectionHeader>> {
    if bytes.len() < 0x34 || bytes.get(0..4)? != b"\x7fELF" || *bytes.get(5)? != 1 {
        return None;
    }

    let (section_header_offset, section_header_size, section_count, class) = match *bytes.get(4)? {
        1 => (
            read_u32_le(bytes.get(0x20..0x24)?)? as usize,
            read_u16_le(bytes.get(0x2e..0x30)?)? as usize,
            read_u16_le(bytes.get(0x30..0x32)?)? as usize,
            1_u8,
        ),
        2 => {
            if bytes.len() < 0x40 {
                return None;
            }
            (
                read_u64_le(bytes.get(0x28..0x30)?)? as usize,
                read_u16_le(bytes.get(0x3a..0x3c)?)? as usize,
                read_u16_le(bytes.get(0x3c..0x3e)?)? as usize,
                2_u8,
            )
        }
        _ => return None,
    };

    let mut sections = Vec::with_capacity(section_count);
    for section_index in 0..section_count {
        let start =
            section_header_offset.checked_add(section_index.checked_mul(section_header_size)?)?;
        let header = bytes.get(start..start.checked_add(section_header_size)?)?;
        let section = match class {
            1 => {
                if header.len() < 40 {
                    return None;
                }
                ElfSectionHeader {
                    sh_type: u64::from(read_u32_le(header.get(4..8)?)?),
                    sh_offset: u64::from(read_u32_le(header.get(16..20)?)?),
                    sh_size: u64::from(read_u32_le(header.get(20..24)?)?),
                    sh_info: u64::from(read_u32_le(header.get(28..32)?)?),
                    sh_entsize: u64::from(read_u32_le(header.get(36..40)?)?),
                }
            }
            2 => {
                if header.len() < 64 {
                    return None;
                }
                ElfSectionHeader {
                    sh_type: u64::from(read_u32_le(header.get(4..8)?)?),
                    sh_offset: read_u64_le(header.get(24..32)?)?,
                    sh_size: read_u64_le(header.get(32..40)?)?,
                    sh_info: u64::from(read_u32_le(header.get(44..48)?)?),
                    sh_entsize: read_u64_le(header.get(56..64)?)?,
                }
            }
            _ => return None,
        };
        sections.push(section);
    }
    Some(sections)
}

fn symbol_value_field_range(
    bytes: &[u8],
    symbol_index: object::SymbolIndex,
) -> Option<std::ops::Range<usize>> {
    elf_symbol_value_field_range(bytes, symbol_index)
        .or_else(|| macho_symbol_value_field_range(bytes, symbol_index))
}

fn macho_symbol_value_field_range(
    bytes: &[u8],
    symbol_index: object::SymbolIndex,
) -> Option<std::ops::Range<usize>> {
    use object::read::macho::MachHeader as _;

    let header = object::macho::MachHeader64::<object::Endianness>::parse(bytes, 0).ok()?;
    let endian = header.endian().ok()?;
    if endian != object::Endianness::Little {
        return None;
    }
    let mut commands = header.load_commands(endian, bytes, 0).ok()?;
    let mut symtab = None;
    while let Some(command) = commands.next().ok()? {
        let Some(command) = command.symtab().ok()? else {
            continue;
        };
        if symtab.replace(command).is_some() {
            return None;
        }
    }
    let symtab = symtab?;
    if symbol_index.0 >= symtab.nsyms.get(endian) as usize {
        return None;
    }

    const ENTRY_SIZE: usize = std::mem::size_of::<object::macho::Nlist64<object::Endianness>>();
    const VALUE_OFFSET: usize =
        std::mem::offset_of!(object::macho::Nlist64<object::Endianness>, n_value);
    let start = (symtab.symoff.get(endian) as usize)
        .checked_add(symbol_index.0.checked_mul(ENTRY_SIZE)?)?
        .checked_add(VALUE_OFFSET)?;
    let end = start.checked_add(std::mem::size_of::<u64>())?;
    bytes.get(start..end)?;
    Some(start..end)
}

fn elf_symbol_value_field_range(
    bytes: &[u8],
    symbol_index: object::SymbolIndex,
) -> Option<std::ops::Range<usize>> {
    if bytes.len() < 0x34 || bytes.get(0..4)? != b"\x7fELF" || *bytes.get(5)? != 1 {
        return None;
    }

    let (entry_size, value_offset, value_size) = match *bytes.get(4)? {
        1 => (16usize, 4usize, 4usize),
        2 => (24usize, 8usize, 8usize),
        _ => return None,
    };
    let symbol_offset = symbol_index.0.checked_mul(entry_size)?;
    let symbol_end = symbol_offset.checked_add(entry_size)?;
    for section in elf_section_headers(bytes)? {
        if section.sh_type != u64::from(object::elf::SHT_SYMTAB)
            || section.sh_entsize != entry_size as u64
            || symbol_end > usize::try_from(section.sh_size).ok()?
        {
            continue;
        }
        let section_start = usize::try_from(section.sh_offset).ok()?;
        let field_start = section_start
            .checked_add(symbol_offset)?
            .checked_add(value_offset)?;
        let field_end = field_start.checked_add(value_size)?;
        if field_end <= bytes.len() {
            return Some(field_start..field_end);
        }
    }
    None
}

#[cfg(test)]
fn patch_ranges(
    bytes: &[u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
) -> Result<Option<Vec<std::ops::Range<usize>>>> {
    let resolver = PatchInputResolver::new(bytes, true)?;
    patch_ranges_with_resolver(
        bytes,
        input_file_path,
        sections,
        &resolver,
        PatchInputLookup::MatchArchiveMember,
    )
}

fn patch_ranges_with_resolver<'data>(
    bytes: &'data [u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    resolver: &PatchInputResolver<'data>,
    lookup: PatchInputLookup,
) -> Result<Option<Vec<std::ops::Range<usize>>>> {
    patch_ranges_with_resolution(bytes, sections, |input_ref| {
        resolver.resolve(input_file_path, input_ref, lookup)
    })
}

fn patch_ranges_with_lookup<'data>(
    bytes: &'data [u8],
    input_file_path: &str,
    sections: impl IntoIterator<Item = PatchSection>,
    lookup: PatchInputLookup,
) -> Result<Option<Vec<std::ops::Range<usize>>>> {
    patch_ranges_with_resolution(bytes, sections, |input_ref| {
        patch_input_bytes_with_lookup(bytes, input_file_path, input_ref, lookup)
    })
}

fn patch_ranges_with_resolution<'data>(
    bytes: &'data [u8],
    sections: impl IntoIterator<Item = PatchSection>,
    mut resolve: impl FnMut(&str) -> Result<Option<PatchInputBytes<'data>>>,
) -> Result<Option<Vec<std::ops::Range<usize>>>> {
    let mut ranges = Vec::new();
    let sections = sections.into_iter().collect::<Vec<_>>();
    let mut sections_by_input = HashMap::<&str, Vec<&PatchSection>>::new();
    for section in &sections {
        sections_by_input
            .entry(section.input.as_str())
            .or_default()
            .push(section);
    }

    for (input_ref, sections) in sections_by_input {
        let Some(input_bytes) = resolve(input_ref)? else {
            return Ok(None);
        };
        let file = object::File::parse(input_bytes.bytes)
            .context("Failed to parse incremental patch input")?;
        for patch_section in sections {
            let Some(section_index) = patch_section_index(&file, patch_section)? else {
                return Ok(None);
            };
            let section = file
                .section_by_index(section_index)
                .context("Missing incremental patch input section")?;
            let Some((offset, size)) = section.file_range() else {
                return Ok(None);
            };
            if size > patch_section.output_size {
                return Ok(None);
            }
            let start = input_bytes
                .file_offset
                .checked_add(offset as usize)
                .context("Incremental patch input range overflow")?;
            let end = start
                .checked_add(size as usize)
                .context("Incremental patch input range overflow")?;
            if end > bytes.len() {
                return Ok(None);
            }
            ranges.push(start..end);
            if let Some(size_range) =
                elf_section_size_field_range(input_bytes.bytes, section_index.0)
            {
                ranges.push(
                    input_bytes.file_offset + size_range.start
                        ..input_bytes.file_offset + size_range.end,
                );
            }
        }
    }

    ranges.sort_by_key(|range| range.start);
    let mut previous_end = 0;
    for range in &ranges {
        if range.start < previous_end {
            return Ok(None);
        }
        previous_end = range.end;
    }

    if ranges.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ranges))
    }
}

fn patch_section_object_index(
    file: &object::File<'_>,
    section_index: u32,
) -> Result<object::SectionIndex> {
    let section_index = section_index as usize;
    let section_index = match file {
        object::File::MachO32(_) | object::File::MachO64(_) => section_index
            .checked_add(1)
            .context("Mach-O incremental section index overflow")?,
        _ => section_index,
    };
    Ok(object::SectionIndex(section_index))
}

fn update_hash_with_zeroes(hasher: &mut blake3::Hasher, mut len: usize) {
    const ZEROES: [u8; 4096] = [0; 4096];
    while len > 0 {
        let chunk_len = len.min(ZEROES.len());
        hasher.update(&ZEROES[..chunk_len]);
        len -= chunk_len;
    }
}

fn elf_section_size_field_range(
    bytes: &[u8],
    section_index: usize,
) -> Option<std::ops::Range<usize>> {
    if bytes.len() < 0x34 || bytes.get(0..4)? != b"\x7fELF" || *bytes.get(5)? != 1 {
        return None;
    }

    match *bytes.get(4)? {
        1 => {
            let section_header_offset = read_u32_le(bytes.get(0x20..0x24)?)? as usize;
            let section_header_size = read_u16_le(bytes.get(0x2e..0x30)?)? as usize;
            let section_count = read_u16_le(bytes.get(0x30..0x32)?)? as usize;
            elf_section_header_field_range(
                bytes,
                section_index,
                section_header_offset,
                section_header_size,
                section_count,
                0x14,
                4,
            )
        }
        2 => {
            if bytes.len() < 0x40 {
                return None;
            }
            let section_header_offset = read_u64_le(bytes.get(0x28..0x30)?)? as usize;
            let section_header_size = read_u16_le(bytes.get(0x3a..0x3c)?)? as usize;
            let section_count = read_u16_le(bytes.get(0x3c..0x3e)?)? as usize;
            elf_section_header_field_range(
                bytes,
                section_index,
                section_header_offset,
                section_header_size,
                section_count,
                0x20,
                8,
            )
        }
        _ => None,
    }
}

fn elf_section_header_field_range(
    bytes: &[u8],
    section_index: usize,
    section_header_offset: usize,
    section_header_size: usize,
    section_count: usize,
    field_offset: usize,
    field_size: usize,
) -> Option<std::ops::Range<usize>> {
    if section_index >= section_count || section_header_size < field_offset + field_size {
        return None;
    }
    let section_start =
        section_header_offset.checked_add(section_index.checked_mul(section_header_size)?)?;
    let field_start = section_start.checked_add(field_offset)?;
    let field_end = field_start.checked_add(field_size)?;
    (field_end <= bytes.len()).then_some(field_start..field_end)
}

fn read_u16_le(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u32_le(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u64_le(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

fn read_i32_le(bytes: &[u8]) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_i64_le(bytes: &[u8]) -> Option<i64> {
    Some(i64::from_le_bytes(bytes.try_into().ok()?))
}

fn build_id_note_range(bytes: &[u8]) -> Result<Option<std::ops::Range<usize>>> {
    let file =
        object::File::parse(bytes).context("Failed to parse output for build ID patching")?;
    for section in file.sections() {
        if section.name_bytes()? != b".note.gnu.build-id" {
            continue;
        }
        let Some((offset, size)) = section.file_range() else {
            return Ok(None);
        };
        let start = offset as usize;
        let end = start
            .checked_add(size as usize)
            .context("Incremental build ID range overflow")?;
        if end > bytes.len() {
            return Ok(None);
        }
        return Ok(Some(start..end));
    }
    Ok(None)
}

fn write_fast_build_id_from_state(
    output: &mut [u8],
    range: std::ops::Range<usize>,
    state: &mut BuildIdHashState,
    tree: &mut [[u8; blake3::OUT_LEN]],
    changed_ranges: &[std::ops::Range<usize>],
) -> Result {
    validate_fast_build_id_range(&range)?;
    output[range.clone()].fill(0);
    let mut hash_ranges = changed_ranges.to_owned();
    hash_ranges.push(range.clone());
    let changed_chunks = touched_build_id_chunks(&hash_ranges, output.len())?;
    if !update_build_id_hash_tree(state, tree, output, &range, &changed_chunks) {
        return Err(crate::error!(
            "Incremental build ID hash state is incompatible with the output"
        ));
    }
    let build_id = build_id_from_hash_tree(state, tree)?;
    write_fast_build_id_note(output, range, &build_id);
    Ok(())
}

fn validate_fast_build_id_range(range: &std::ops::Range<usize>) -> Result {
    const GNU_NOTE_NAME: &[u8] = b"GNU\0";
    let expected_len = 12 + GNU_NOTE_NAME.len() + blake3::OUT_LEN;
    if range.end - range.start != expected_len {
        return Err(crate::error!(
            "Incremental patching only supports fast 32-byte build IDs"
        ));
    }
    Ok(())
}

fn write_fast_build_id_note(
    output: &mut [u8],
    range: std::ops::Range<usize>,
    build_id: &blake3::Hash,
) {
    const GNU_NOTE_NAME: &[u8] = b"GNU\0";
    let note = &mut output[range];
    note[0..4].copy_from_slice(&(GNU_NOTE_NAME.len() as u32).to_le_bytes());
    note[4..8].copy_from_slice(&(blake3::OUT_LEN as u32).to_le_bytes());
    note[8..12].copy_from_slice(&object::elf::NT_GNU_BUILD_ID.to_le_bytes());
    note[12..16].copy_from_slice(GNU_NOTE_NAME);
    note[16..].copy_from_slice(build_id.as_bytes());
}

fn build_id_hash_state_from_output(bytes: &[u8]) -> Result<BuildIdHashStateAndTree> {
    let Some(range) = build_id_note_range(bytes)? else {
        return Ok((None, None));
    };
    validate_fast_build_id_range(&range)?;
    let Some(tree) = build_id_hash_tree(bytes, &range) else {
        return Ok((None, None));
    };
    let nodes = tree.len();
    Ok((
        Some(BuildIdHashState {
            output_len: bytes.len() as u64,
            nodes,
            tree_hash: Some(build_id_hash_tree_hash(&tree)),
        }),
        Some(tree),
    ))
}

fn build_id_hash_node_count(len: usize) -> Option<usize> {
    if len <= BUILD_ID_HASH_GROUP_LEN {
        return None;
    }
    let left_len = blake3::hazmat::left_subtree_len(len as u64) as usize;
    Some(build_id_subtree_node_count(left_len) + build_id_subtree_node_count(len - left_len))
}

fn build_id_hash_tree(
    bytes: &[u8],
    zero_range: &std::ops::Range<usize>,
) -> Option<Vec<[u8; blake3::OUT_LEN]>> {
    let nodes = build_id_hash_node_count(bytes.len())?;
    let mut tree = vec![[0; blake3::OUT_LEN]; nodes];
    let left_len = blake3::hazmat::left_subtree_len(bytes.len() as u64) as usize;
    let left_nodes = build_id_subtree_node_count(left_len);
    let (left_tree, right_tree) = tree.split_at_mut(left_nodes);
    rayon::join(
        || build_id_subtree_hash(bytes, 0, left_len, zero_range, left_tree),
        || {
            build_id_subtree_hash(
                bytes,
                left_len,
                bytes.len() - left_len,
                zero_range,
                right_tree,
            )
        },
    );
    Some(tree)
}

fn build_id_subtree_node_count(len: usize) -> usize {
    2 * len.div_ceil(BUILD_ID_HASH_GROUP_LEN) - 1
}

fn build_id_subtree_hash(
    bytes: &[u8],
    start: usize,
    len: usize,
    zero_range: &std::ops::Range<usize>,
    tree: &mut [[u8; blake3::OUT_LEN]],
) -> [u8; blake3::OUT_LEN] {
    debug_assert_eq!(tree.len(), build_id_subtree_node_count(len));
    let hash = if len <= BUILD_ID_HASH_GROUP_LEN {
        build_id_leaf_hash(bytes, start, len, zero_range)
    } else {
        let left_len = blake3::hazmat::left_subtree_len(len as u64) as usize;
        let left_nodes = build_id_subtree_node_count(left_len);
        let (root, children) = tree.split_first_mut().unwrap();
        let (left_tree, right_tree) = children.split_at_mut(left_nodes);
        let mut compute_left =
            || build_id_subtree_hash(bytes, start, left_len, zero_range, left_tree);
        let mut compute_right = || {
            build_id_subtree_hash(
                bytes,
                start + left_len,
                len - left_len,
                zero_range,
                right_tree,
            )
        };
        let (left, right) = if len >= BUILD_ID_HASH_PARALLEL_THRESHOLD {
            rayon::join(compute_left, compute_right)
        } else {
            (compute_left(), compute_right())
        };
        let hash =
            blake3::hazmat::merge_subtrees_non_root(&left, &right, blake3::hazmat::Mode::Hash);
        *root = hash;
        return hash;
    };
    tree[0] = hash;
    hash
}

fn update_build_id_hash_tree(
    state: &mut BuildIdHashState,
    tree: &mut [[u8; blake3::OUT_LEN]],
    output: &[u8],
    zero_range: &std::ops::Range<usize>,
    changed_chunks: &[usize],
) -> bool {
    if state.output_len != output.len() as u64 {
        return false;
    }
    if Some(state.nodes) != build_id_hash_node_count(output.len()) {
        return false;
    }
    if tree.len() != state.nodes {
        return false;
    }
    if output.len() <= BUILD_ID_HASH_GROUP_LEN {
        return false;
    }
    let left_len = blake3::hazmat::left_subtree_len(output.len() as u64) as usize;
    update_build_id_subtree_hash(tree, 0, output, 0, left_len, zero_range, changed_chunks);
    let right_index = build_id_subtree_node_count(left_len);
    update_build_id_subtree_hash(
        tree,
        right_index,
        output,
        left_len,
        output.len() - left_len,
        zero_range,
        changed_chunks,
    );
    state.tree_hash = Some(build_id_hash_tree_hash(tree));
    true
}

fn update_build_id_subtree_hash(
    tree: &mut [[u8; blake3::OUT_LEN]],
    index: usize,
    output: &[u8],
    start: usize,
    len: usize,
    zero_range: &std::ops::Range<usize>,
    changed_chunks: &[usize],
) -> bool {
    if !touched_chunks_overlap(changed_chunks, start, len) {
        return false;
    }
    if len <= BUILD_ID_HASH_GROUP_LEN {
        tree[index] = build_id_leaf_hash(output, start, len, zero_range);
        return true;
    }

    let left_len = blake3::hazmat::left_subtree_len(len as u64) as usize;
    let left_index = index + 1;
    let right_index = left_index + build_id_subtree_node_count(left_len);
    let left_changed = update_build_id_subtree_hash(
        tree,
        left_index,
        output,
        start,
        left_len,
        zero_range,
        changed_chunks,
    );
    let right_changed = update_build_id_subtree_hash(
        tree,
        right_index,
        output,
        start + left_len,
        len - left_len,
        zero_range,
        changed_chunks,
    );
    if left_changed || right_changed {
        tree[index] = blake3::hazmat::merge_subtrees_non_root(
            &tree[left_index],
            &tree[right_index],
            blake3::hazmat::Mode::Hash,
        );
    }
    left_changed || right_changed
}

fn build_id_leaf_hash(
    bytes: &[u8],
    start: usize,
    len: usize,
    zero_range: &std::ops::Range<usize>,
) -> [u8; blake3::OUT_LEN] {
    let end = start + len;
    let chunk = &bytes[start..end];
    if let Some(overlap) = intersect_ranges(start..end, zero_range.clone()) {
        let mut zeroed = chunk.to_vec();
        zeroed[overlap.start - start..overlap.end - start].fill(0);
        build_id_leaf_hash_bytes(&zeroed, start)
    } else {
        build_id_leaf_hash_bytes(chunk, start)
    }
}

fn build_id_leaf_hash_bytes(bytes: &[u8], start: usize) -> [u8; blake3::OUT_LEN] {
    use blake3::hazmat::HasherExt as _;
    blake3::Hasher::new()
        .set_input_offset(start as u64)
        .update(bytes)
        .finalize_non_root()
}

fn build_id_from_hash_tree(
    state: &BuildIdHashState,
    tree: &[[u8; blake3::OUT_LEN]],
) -> Result<blake3::Hash> {
    let len = usize::try_from(state.output_len)
        .context("Incremental build ID hash output length is too large")?;
    if Some(state.nodes) != build_id_hash_node_count(len) {
        return Err(crate::error!(
            "Incremental build ID hash state does not match output length"
        ));
    }
    if tree.len() != state.nodes {
        return Err(crate::error!(
            "Incremental build ID hash tree size does not match state"
        ));
    }
    let left_len = blake3::hazmat::left_subtree_len(len as u64);
    let right_index = build_id_subtree_node_count(left_len as usize);
    Ok(blake3::hazmat::merge_subtrees_root(
        &tree[0],
        &tree[right_index],
        blake3::hazmat::Mode::Hash,
    ))
}

fn build_id_hash_tree_hash(tree: &[[u8; blake3::OUT_LEN]]) -> String {
    let mut hasher = blake3::Hasher::new();
    for node in tree {
        hasher.update(node);
    }
    hasher.finalize().to_hex().to_string()
}

fn touched_build_id_chunks(
    ranges: &[std::ops::Range<usize>],
    output_len: usize,
) -> Result<Vec<usize>> {
    let mut chunks = Vec::new();
    for range in ranges {
        if range.start > range.end || range.end > output_len {
            return Err(crate::error!("Incremental build ID patch range is invalid"));
        }
        if range.is_empty() {
            continue;
        }
        let first = range.start / BUILD_ID_HASH_GROUP_LEN;
        let last = (range.end - 1) / BUILD_ID_HASH_GROUP_LEN;
        chunks.extend(first..=last);
    }
    chunks.sort_unstable();
    chunks.dedup();
    Ok(chunks)
}

fn touched_chunks_overlap(chunks: &[usize], start: usize, len: usize) -> bool {
    if len == 0 {
        return false;
    }
    let first = start / BUILD_ID_HASH_GROUP_LEN;
    let last = (start + len - 1) / BUILD_ID_HASH_GROUP_LEN;
    chunks.iter().any(|chunk| (first..=last).contains(chunk))
}

fn intersect_ranges(
    left: std::ops::Range<usize>,
    right: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    let start = left.start.max(right.start);
    let end = left.end.min(right.end);
    (start < end).then_some(start..end)
}

fn read_build_id_hash_tree(
    state_dir: &Path,
    state: &BuildIdHashState,
) -> Result<Vec<[u8; blake3::OUT_LEN]>> {
    let len = usize::try_from(state.output_len)
        .context("Incremental build ID hash output length is too large")?;
    if Some(state.nodes) != build_id_hash_node_count(len) {
        return Err(crate::error!(
            "Incremental build ID hash state does not match output length"
        ));
    }
    let path = state_dir.join(BUILD_ID_HASH_FILE);
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "Failed to read incremental build ID hash `{}`",
            path.display()
        )
    })?;
    let expected_len = state.nodes * blake3::OUT_LEN;
    if bytes.len() != expected_len {
        return Err(crate::error!(
            "Incremental build ID hash tree has {} bytes, expected {expected_len}",
            bytes.len()
        ));
    }
    let tree = bytes
        .chunks_exact(blake3::OUT_LEN)
        .map(|chunk| chunk.try_into().unwrap())
        .collect::<Vec<_>>();
    if let Some(expected_hash) = state.tree_hash.as_deref()
        && build_id_hash_tree_hash(&tree) != expected_hash
    {
        return Err(crate::error!(
            "Incremental build ID hash tree does not match its recorded hash"
        ));
    }
    Ok(tree)
}

fn write_build_id_hash_tree(state_dir: &Path, tree: Option<&[[u8; blake3::OUT_LEN]]>) -> Result {
    let path = state_dir.join(BUILD_ID_HASH_FILE);
    let Some(tree) = tree else {
        let _ = std::fs::remove_file(path);
        return Ok(());
    };
    std::fs::create_dir_all(state_dir).with_context(|| {
        format!(
            "Failed to create incremental state directory `{}`",
            state_dir.display()
        )
    })?;
    let tmp_path = state_dir.join(format!("{BUILD_ID_HASH_FILE}.tmp"));
    let mut bytes = Vec::with_capacity(tree.len() * blake3::OUT_LEN);
    for node in tree {
        bytes.extend_from_slice(node);
    }
    std::fs::write(&tmp_path, bytes).with_context(|| {
        format!(
            "Failed to write incremental build ID hash `{}`",
            tmp_path.display()
        )
    })?;
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&tmp_path, &path).with_context(|| {
        format!(
            "Failed to install incremental build ID hash `{}`",
            path.display()
        )
    })?;
    Ok(())
}

fn fingerprint_loaded_files(
    file_loader: &FileLoader<'_>,
    previous: Option<&PersistedState>,
) -> Vec<FileState> {
    let previous_by_path = previous.map(|previous| {
        previous
            .input_files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect::<HashMap<_, _>>()
    });

    let mut files = file_loader
        .loaded_files
        .iter()
        .map(|input_file| {
            let path = encode_path(&input_file.filename);
            let previous = previous_by_path
                .as_ref()
                .and_then(|previous| previous.get(path.as_str()).copied());
            let content =
                FileContentState::from_input_file(input_file, previous.map(|file| &file.content));
            let snapshot_identity = previous
                .filter(|previous| previous.content == content)
                .and_then(|previous| previous.snapshot_identity.clone());
            let patch = previous
                .filter(|previous| previous.content == content)
                .and_then(|previous| previous.patch.clone());
            FileState {
                path,
                content,
                snapshot_identity,
                patch,
            }
        })
        .collect::<Vec<_>>();

    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

fn parse_prefixed_line<'a>(line: Option<&'a str>, expected_prefix: &str) -> Result<&'a str> {
    let line = line.context("Missing incremental state line")?;
    let (prefix, rest) = line
        .split_once('\t')
        .context("Malformed incremental state line")?;
    if prefix != expected_prefix {
        return Err(crate::error!(
            "Expected incremental state line `{expected_prefix}`, got `{prefix}`"
        ));
    }
    Ok(rest)
}

fn parse_link_start_line(line: Option<&str>) -> Result<Option<FileIdentity>> {
    let rest = parse_prefixed_line(line, "link-start")?;
    FileIdentity::parse(rest)
}

fn parse_content_line(line: Option<&str>, expected_prefix: &str) -> Result<FileContentState> {
    let rest = parse_prefixed_line(line, expected_prefix)?;
    let mut parts = rest.split('\t');
    let len = parts
        .next()
        .context("Malformed incremental content length")?;
    let hash = parts.next().context("Malformed incremental content hash")?;
    let identity = parts.next().map(FileIdentity::parse).transpose()?.flatten();
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental content record"));
    }
    Ok(FileContentState {
        len: len.parse().context("Invalid incremental content length")?,
        hash: hash.to_owned(),
        identity,
    })
}

fn parse_build_id_hash_line(line: Option<&str>) -> Result<Option<BuildIdHashState>> {
    let rest = parse_prefixed_line(line, "build-id-hash")?;
    if rest == ABSENT_FIELD {
        return Ok(None);
    }
    let mut parts = rest.split('\t');
    let output_len = parts
        .next()
        .context("Malformed incremental build ID hash output length")?
        .parse()
        .context("Invalid incremental build ID hash output length")?;
    let nodes = parts
        .next()
        .context("Malformed incremental build ID hash node count")?
        .parse()
        .context("Invalid incremental build ID hash node count")?;
    let tree_hash = parts
        .next()
        .filter(|tree_hash| *tree_hash != ABSENT_FIELD)
        .map(|tree_hash| tree_hash.to_owned());
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental build ID hash record"));
    }
    if nodes == 0 {
        return Err(crate::error!("Missing incremental build ID hash nodes"));
    }
    Ok(Some(BuildIdHashState {
        output_len,
        nodes,
        tree_hash,
    }))
}

fn parse_patch_record_location_line(line: Option<&str>) -> Result<PatchRecordLocation> {
    let rest = parse_prefixed_line(line, "patch-record")?;
    let mut parts = rest.split('\t');
    let input_file = parts
        .next()
        .context("Malformed incremental patch record input")?
        .to_owned();
    let offset = parts
        .next()
        .context("Malformed incremental patch record offset")?
        .parse()
        .context("Invalid incremental patch record offset")?;
    let len = parts
        .next()
        .context("Malformed incremental patch record length")?
        .parse()
        .context("Invalid incremental patch record length")?;
    let hash = parts
        .next()
        .context("Malformed incremental patch record hash")?
        .to_owned();
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental patch record location"));
    }
    Ok(PatchRecordLocation {
        input_file,
        offset,
        len,
        hash,
    })
}

fn parse_patch_record_location_table<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    parse_locations: bool,
) -> Result<(Vec<PatchRecordLocation>, Option<String>)> {
    let count_line = lines
        .next()
        .context("Missing incremental patch record location count")?;
    let location_count: usize = parse_prefixed_line(Some(count_line), "patch-records")?
        .parse()
        .context("Invalid incremental patch record location count")?;
    if parse_locations {
        let mut locations = Vec::with_capacity(location_count);
        for _ in 0..location_count {
            locations.push(parse_patch_record_location_line(lines.next())?);
        }
        return Ok((locations, None));
    }

    let mut raw = String::new();
    writeln!(&mut raw, "{count_line}").unwrap();
    for _ in 0..location_count {
        let line = lines
            .next()
            .context("Missing incremental patch record location")?;
        writeln!(&mut raw, "{line}").unwrap();
    }
    Ok((Vec::new(), Some(raw)))
}

fn render_patch_record_location_table(
    out: &mut String,
    patch_record_locations: &[PatchRecordLocation],
    raw_patch_record_locations: Option<&str>,
) {
    if let Some(raw) = raw_patch_record_locations {
        out.push_str(raw);
        return;
    }
    writeln!(out, "patch-records\t{}", patch_record_locations.len()).unwrap();
    for location in patch_record_locations {
        writeln!(
            out,
            "patch-record\t{}\t{}\t{}\t{}",
            location.input_file, location.offset, location.len, location.hash
        )
        .unwrap();
    }
}

#[derive(Default)]
struct CompactRecords {
    sections: Vec<SectionRecord>,
    relocations: Vec<RelocationRecord>,
    fdes: Vec<FdeRecord>,
    dynamic_relocations: Vec<DynamicRelocationRecord>,
}

#[derive(Default)]
struct CompactRecordRefs<'a> {
    sections: Vec<&'a SectionRecord>,
    relocations: Vec<&'a RelocationRecord>,
    fdes: Vec<&'a FdeRecord>,
    dynamic_relocations: Vec<&'a DynamicRelocationRecord>,
}

fn parse_compact_records_block<'a>(
    mut lines: impl Iterator<Item = &'a str>,
) -> Result<CompactRecords> {
    let section_input_count: usize = parse_prefixed_line(lines.next(), "section-inputs")?
        .parse()
        .context("Invalid incremental section input count")?;
    let mut section_inputs = Vec::with_capacity(section_input_count);
    for _ in 0..section_input_count {
        let line = lines
            .next()
            .context("Missing incremental section input record")?;
        section_inputs.push(parse_section_input_line(line)?);
    }

    let section_count: usize = parse_prefixed_line(lines.next(), "sections")?
        .parse()
        .context("Invalid incremental section count")?;
    let mut sections = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        let line = lines.next().context("Missing incremental section record")?;
        sections.push(parse_compact_section_line(line, &section_inputs)?);
    }
    let mut relocations = Vec::new();
    let mut fdes = Vec::new();
    let mut dynamic_relocations = Vec::new();
    let mut next_line = lines.next();
    if let Some(line) = next_line
        && line.starts_with("relocs\t")
    {
        let relocation_count: usize = parse_prefixed_line(Some(line), "relocs")?
            .parse()
            .context("Invalid incremental relocation count")?;
        relocations = Vec::with_capacity(relocation_count);
        for _ in 0..relocation_count {
            let line = lines
                .next()
                .context("Missing incremental relocation record")?;
            relocations.push(parse_compact_relocation_line(line, &section_inputs)?);
        }
        next_line = lines.next();
    }
    if let Some(line) = next_line
        && line.starts_with("fdes\t")
    {
        let fde_count: usize = parse_prefixed_line(Some(line), "fdes")?
            .parse()
            .context("Invalid incremental FDE count")?;
        fdes = Vec::with_capacity(fde_count);
        for _ in 0..fde_count {
            let line = lines.next().context("Missing incremental FDE record")?;
            fdes.push(parse_compact_fde_line(line, &section_inputs)?);
        }
        next_line = lines.next();
    }
    if let Some(line) = next_line
        && line.starts_with("dynrels\t")
    {
        let relocation_count: usize = parse_prefixed_line(Some(line), "dynrels")?
            .parse()
            .context("Invalid incremental dynamic relocation count")?;
        dynamic_relocations = Vec::with_capacity(relocation_count);
        for _ in 0..relocation_count {
            let line = lines
                .next()
                .context("Missing incremental dynamic relocation record")?;
            dynamic_relocations.push(parse_compact_dynamic_relocation_line(
                line,
                &section_inputs,
            )?);
        }
        next_line = lines.next();
    }
    if next_line.is_some() || lines.next().is_some() {
        return Err(crate::error!(
            "Unexpected trailing incremental section data"
        ));
    }
    Ok(CompactRecords {
        sections,
        relocations,
        fdes,
        dynamic_relocations,
    })
}

fn parse_compact_records_block_for_input_files<'a>(
    mut lines: impl Iterator<Item = &'a str>,
    input_files: &HashSet<String>,
) -> Result<CompactRecords> {
    let section_input_count: usize = parse_prefixed_line(lines.next(), "section-inputs")?
        .parse()
        .context("Invalid incremental section input count")?;
    let mut section_inputs = Vec::with_capacity(section_input_count);
    for _ in 0..section_input_count {
        let line = lines
            .next()
            .context("Missing incremental section input record")?;
        section_inputs.push(parse_section_input_line(line)?);
    }
    let matched_section_input_ids = section_inputs
        .iter()
        .enumerate()
        .filter_map(|(index, (input_file, _))| input_files.contains(input_file).then_some(index))
        .collect::<HashSet<_>>();

    let section_count: usize = parse_prefixed_line(lines.next(), "sections")?
        .parse()
        .context("Invalid incremental section count")?;
    let mut sections = Vec::new();
    for _ in 0..section_count {
        let line = lines.next().context("Missing incremental section record")?;
        if compact_record_matches_input(
            line,
            "section",
            &matched_section_input_ids,
            section_inputs.len(),
        )? {
            sections.push(parse_compact_section_line(line, &section_inputs)?);
        }
    }

    let mut relocations = Vec::new();
    let mut fdes = Vec::new();
    let mut dynamic_relocations = Vec::new();
    let mut next_line = lines.next();
    if let Some(line) = next_line
        && line.starts_with("relocs\t")
    {
        let relocation_count: usize = parse_prefixed_line(Some(line), "relocs")?
            .parse()
            .context("Invalid incremental relocation count")?;
        for _ in 0..relocation_count {
            let line = lines
                .next()
                .context("Missing incremental relocation record")?;
            if compact_relocation_record_matches_input(
                line,
                &matched_section_input_ids,
                input_files,
                section_inputs.len(),
            )? {
                relocations.push(parse_compact_relocation_line(line, &section_inputs)?);
            }
        }
        next_line = lines.next();
    }
    if let Some(line) = next_line
        && line.starts_with("fdes\t")
    {
        let fde_count: usize = parse_prefixed_line(Some(line), "fdes")?
            .parse()
            .context("Invalid incremental FDE count")?;
        for _ in 0..fde_count {
            let line = lines.next().context("Missing incremental FDE record")?;
            if compact_record_matches_input(
                line,
                "fde",
                &matched_section_input_ids,
                section_inputs.len(),
            )? {
                fdes.push(parse_compact_fde_line(line, &section_inputs)?);
            }
        }
        next_line = lines.next();
    }
    if let Some(line) = next_line
        && line.starts_with("dynrels\t")
    {
        let relocation_count: usize = parse_prefixed_line(Some(line), "dynrels")?
            .parse()
            .context("Invalid incremental dynamic relocation count")?;
        for _ in 0..relocation_count {
            let line = lines
                .next()
                .context("Missing incremental dynamic relocation record")?;
            if compact_record_matches_input(
                line,
                "dynrel",
                &matched_section_input_ids,
                section_inputs.len(),
            )? {
                dynamic_relocations.push(parse_compact_dynamic_relocation_line(
                    line,
                    &section_inputs,
                )?);
            }
        }
        next_line = lines.next();
    }
    if next_line.is_some() || lines.next().is_some() {
        return Err(crate::error!(
            "Unexpected trailing incremental section data"
        ));
    }
    Ok(CompactRecords {
        sections,
        relocations,
        fdes,
        dynamic_relocations,
    })
}

fn compact_record_matches_input(
    line: &str,
    prefix: &str,
    matched_section_input_ids: &HashSet<usize>,
    section_input_count: usize,
) -> Result<bool> {
    let rest = parse_prefixed_line(Some(line), prefix)?;
    let section_input_id = compact_record_section_input_id(rest, prefix)?;
    if section_input_id >= section_input_count {
        return Err(crate::error!(
            "Incremental {prefix} input index out of bounds"
        ));
    }
    Ok(matched_section_input_ids.contains(&section_input_id))
}

fn compact_relocation_record_matches_input(
    line: &str,
    matched_section_input_ids: &HashSet<usize>,
    input_files: &HashSet<String>,
    section_input_count: usize,
) -> Result<bool> {
    if line.starts_with("reloc3\t") || line.starts_with("reloc2\t") {
        let (prefix, expected_len, target_input_index) = if line.starts_with("reloc3\t") {
            ("reloc3", 15, 12)
        } else {
            ("reloc2", 14, 11)
        };
        let rest = parse_prefixed_line(Some(line), prefix)?;
        if compact_record_matches_input(
            line,
            prefix,
            matched_section_input_ids,
            section_input_count,
        )? {
            return Ok(true);
        }
        let parts = rest.split('\t').collect::<Vec<_>>();
        if parts.len() != expected_len || parts[target_input_index] == ABSENT_FIELD {
            return Ok(false);
        }
        let target_section_input_id: usize = parts[target_input_index]
            .parse()
            .context("Invalid incremental relocation target input index")?;
        if target_section_input_id >= section_input_count {
            return Err(crate::error!(
                "Incremental relocation target input index out of bounds"
            ));
        }
        return Ok(matched_section_input_ids.contains(&target_section_input_id));
    }

    if compact_record_matches_input(
        line,
        "reloc",
        matched_section_input_ids,
        section_input_count,
    )? {
        return Ok(true);
    }
    Ok(input_files
        .iter()
        .any(|input_file| line.contains(input_file)))
}

fn compact_record_section_input_id(rest: &str, prefix: &str) -> Result<usize> {
    rest.split('\t')
        .next()
        .context("Malformed incremental record input index")?
        .parse()
        .with_context(|| format!("Invalid incremental {prefix} input index"))
}

fn parse_input_line(line: &str, patch_section_mode: PatchSectionReadMode) -> Result<FileState> {
    let rest = parse_prefixed_line(Some(line), "input")?;
    let mut parts = rest.split('\t');
    let path = parts
        .next()
        .context("Malformed incremental input path")?
        .to_owned();
    let len = parts
        .next()
        .context("Malformed incremental input length")?
        .parse()
        .context("Invalid incremental input length")?;
    let hash = parts
        .next()
        .context("Malformed incremental input hash")?
        .to_owned();
    let identity = parts.next().map(FileIdentity::parse).transpose()?.flatten();
    let patch_fingerprint = parts
        .next()
        .filter(|fingerprint| *fingerprint != ABSENT_FIELD);
    let patch_sections = parts.next().filter(|sections| *sections != ABSENT_FIELD);
    let snapshot_identity = parts.next().map(FileIdentity::parse).transpose()?.flatten();
    let archive_member_set_proof = parts
        .next()
        .filter(|proof| *proof != ABSENT_FIELD)
        .map(parse_archive_member_set_proof)
        .transpose()?;
    let archive_member_patch_fingerprints = parts
        .next()
        .filter(|fingerprints| *fingerprints != ABSENT_FIELD)
        .map(parse_archive_member_patch_fingerprints)
        .transpose()?;
    let patch = match patch_fingerprint.zip(patch_sections) {
        Some((fingerprint, raw_sections)) => {
            let sections = match patch_section_mode {
                PatchSectionReadMode::Parse => parse_patch_sections(&path, raw_sections)?,
                PatchSectionReadMode::PreserveRaw => Vec::new(),
            };
            Some(FilePatchState {
                fingerprint: fingerprint.to_owned(),
                archive_member_set_proof,
                archive_member_patch_fingerprints,
                sections,
                raw_sections: matches!(patch_section_mode, PatchSectionReadMode::PreserveRaw)
                    .then(|| raw_sections.to_owned()),
            })
        }
        None => None,
    };
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental input record"));
    }
    Ok(FileState {
        path,
        content: FileContentState {
            len,
            hash,
            identity,
        },
        snapshot_identity,
        patch,
    })
}

fn render_patch_sections(patch: &FilePatchState) -> String {
    if patch.sections.is_empty()
        && let Some(raw_sections) = &patch.raw_sections
    {
        return raw_sections.clone();
    }

    patch
        .sections
        .iter()
        .map(|section| {
            let cstring_nul_boundaries_hash = section
                .cstring_nul_boundaries_hash
                .as_ref()
                .map(|hash| format!(":{hash}"))
                .unwrap_or_default();
            format!(
                "{}:{}:{}:{}:{}:{}:{}{}",
                section.input,
                section.section_index,
                section.input_size,
                section.output_offset,
                section.output_size,
                section.section_name.as_ref().map_or_else(
                    || ABSENT_FIELD.to_owned(),
                    |name| hex::encode(name.as_bytes())
                ),
                section.data_hash.as_deref().unwrap_or(ABSENT_FIELD),
                cstring_nul_boundaries_hash,
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn render_input_line_rest(input: &FileState) -> String {
    let archive_member_patch_fingerprints = input
        .patch
        .as_ref()
        .and_then(|patch| patch.archive_member_patch_fingerprints.as_deref())
        .map(|fingerprints| {
            format!(
                "\t{}",
                render_archive_member_patch_fingerprints(fingerprints)
            )
        })
        .unwrap_or_default();
    let archive_member_set_proof = input
        .patch
        .as_ref()
        .and_then(|patch| patch.archive_member_set_proof.as_ref())
        .map(|proof| format!("\t{}", render_archive_member_set_proof(proof)))
        .or_else(|| (!archive_member_patch_fingerprints.is_empty()).then(|| "\t-".to_owned()))
        .unwrap_or_default();
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}{}{}",
        input.path,
        input.content.len,
        input.content.hash,
        input.content.render_identity(),
        input
            .patch
            .as_ref()
            .map_or(ABSENT_FIELD, |patch| patch.fingerprint.as_str()),
        input
            .patch
            .as_ref()
            .map_or_else(|| ABSENT_FIELD.to_owned(), render_patch_sections),
        input
            .snapshot_identity
            .as_ref()
            .map_or_else(|| ABSENT_FIELD.to_owned(), FileIdentity::render),
        archive_member_set_proof,
        archive_member_patch_fingerprints,
    )
}

fn render_archive_member_patch_fingerprints(
    fingerprints: &[ArchiveMemberPatchFingerprint],
) -> String {
    fingerprints
        .iter()
        .map(|fingerprint| {
            format!(
                "{}:{}:{}:{}:{}:{}",
                fingerprint.archive_member_index,
                hex::encode(&fingerprint.identifier),
                fingerprint.data_len,
                fingerprint.ranges_hash,
                fingerprint.rustc_object_digest,
                fingerprint.fingerprint,
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_archive_member_patch_fingerprints(
    fingerprints: &str,
) -> Result<Vec<ArchiveMemberPatchFingerprint>> {
    if fingerprints.is_empty() {
        return Err(crate::error!(
            "Malformed incremental archive member patch fingerprints"
        ));
    }
    let mut parsed = Vec::new();
    let mut indices = HashSet::new();
    for fingerprint in fingerprints.split(',') {
        let mut parts = fingerprint.split(':');
        let archive_member_index = parts
            .next()
            .context("Malformed incremental archive member patch fingerprint")?
            .parse()
            .context("Invalid incremental archive member patch fingerprint index")?;
        let identifier = hex::decode(
            parts
                .next()
                .context("Malformed incremental archive member patch fingerprint")?,
        )
        .context("Invalid incremental archive member patch fingerprint identifier")?;
        let data_len = parts
            .next()
            .context("Malformed incremental archive member patch fingerprint")?
            .parse()
            .context("Invalid incremental archive member patch fingerprint data length")?;
        let ranges_hash = parts
            .next()
            .context("Malformed incremental archive member patch fingerprint")?
            .to_owned();
        let rustc_object_digest = parts
            .next()
            .context("Malformed incremental archive member patch fingerprint")?
            .to_owned();
        let member_fingerprint = parts
            .next()
            .context("Malformed incremental archive member patch fingerprint")?
            .to_owned();
        if identifier.is_empty()
            || !is_blake3_hex_digest(&ranges_hash)
            || !is_blake3_hex_digest(&rustc_object_digest)
            || !is_blake3_hex_digest(&member_fingerprint)
            || parts.next().is_some()
            || !indices.insert(archive_member_index)
        {
            return Err(crate::error!(
                "Malformed incremental archive member patch fingerprint"
            ));
        }
        parsed.push(ArchiveMemberPatchFingerprint {
            archive_member_index,
            identifier,
            data_len,
            ranges_hash,
            rustc_object_digest,
            fingerprint: member_fingerprint,
        });
    }
    Ok(parsed)
}

fn render_archive_member_set_proof(proof: &ArchiveMemberSetProof) -> String {
    format!(
        "{}:{}:{}:{}",
        proof.raw_ordered_hash,
        proof.normalized_ordered_hash,
        proof.member_count,
        proof
            .rustc_link_content_digest
            .as_deref()
            .unwrap_or(ABSENT_FIELD)
    )
}

fn parse_archive_member_set_proof(proof: &str) -> Result<ArchiveMemberSetProof> {
    let mut parts = proof.split(':');
    let raw_ordered_hash = parts
        .next()
        .context("Malformed incremental archive member-set proof")?
        .to_owned();
    let normalized_ordered_hash = parts
        .next()
        .context("Malformed incremental archive member-set proof")?
        .to_owned();
    let member_count = parts
        .next()
        .context("Malformed incremental archive member-set proof")?
        .parse()
        .context("Invalid incremental archive member-set count")?;
    let rustc_link_content_digest = parts
        .next()
        .filter(|digest| *digest != ABSENT_FIELD)
        .map(|digest| {
            if !is_blake3_hex_digest(digest) {
                return Err(crate::error!(
                    "Invalid incremental rustc rlib link-content digest"
                ));
            }
            Ok(digest.to_owned())
        })
        .transpose()?;
    if parts.next().is_some() {
        return Err(crate::error!(
            "Malformed incremental archive member-set proof"
        ));
    }
    Ok(ArchiveMemberSetProof {
        raw_ordered_hash,
        normalized_ordered_hash,
        member_count,
        rustc_link_content_digest,
    })
}

fn render_build_id_hash_state(state: &BuildIdHashState) -> String {
    format!(
        "{}\t{}\t{}",
        state.output_len,
        state.nodes,
        state.tree_hash.as_deref().unwrap_or(ABSENT_FIELD)
    )
}

fn parse_patch_sections(default_input: &str, sections: &str) -> Result<Vec<FilePatchSectionState>> {
    if sections.is_empty() {
        return Ok(Vec::new());
    }
    let mut parsed = Vec::new();
    for section in sections.split(',') {
        let parts = section.split(':').collect::<Vec<_>>();
        let (input, parts, data_hash, cstring_nul_boundaries_hash) = match parts.len() {
            4 | 5 => (default_input.to_owned(), parts.as_slice(), None, None),
            6 => (parts[0].to_owned(), &parts[1..], None, None),
            7 => (
                parts[0].to_owned(),
                &parts[1..6],
                (parts[6] != ABSENT_FIELD).then(|| parts[6].to_owned()),
                None,
            ),
            8 => (
                parts[0].to_owned(),
                &parts[1..6],
                (parts[6] != ABSENT_FIELD).then(|| parts[6].to_owned()),
                (parts[7] != ABSENT_FIELD).then(|| parts[7].to_owned()),
            ),
            _ => return Ok(Vec::new()),
        };
        if parts.len() != 4 && parts.len() != 5 {
            return Ok(Vec::new());
        }
        let section_name = parts
            .get(4)
            .copied()
            .filter(|name| *name != ABSENT_FIELD)
            .map(|name| {
                let bytes =
                    hex::decode(name).context("Malformed incremental patch section name")?;
                String::from_utf8(bytes).context("Invalid incremental patch section name")
            })
            .transpose()?;
        parsed.push(FilePatchSectionState {
            input,
            section_index: parts[0]
                .parse()
                .context("Invalid incremental patch section index")?,
            section_name,
            input_size: parts[1]
                .parse()
                .context("Invalid incremental patch section input size")?,
            output_offset: parts[2]
                .parse()
                .context("Invalid incremental patch section output offset")?,
            output_size: parts[3]
                .parse()
                .context("Invalid incremental patch section output size")?,
            data_hash,
            cstring_nul_boundaries_hash,
        });
    }
    Ok(parsed)
}

fn parse_section_line(line: &str) -> Result<SectionRecord> {
    let rest = parse_prefixed_line(Some(line), "section")?;
    let mut parts = rest.split('\t');
    let input_file = parts
        .next()
        .context("Malformed incremental section input file")?
        .to_owned();
    let input = parts
        .next()
        .context("Malformed incremental section input")?
        .to_owned();
    let section_index = parts
        .next()
        .context("Malformed incremental section index")?
        .parse()
        .context("Invalid incremental section index")?;
    let output_offset = parts
        .next()
        .context("Malformed incremental section output offset")?
        .parse()
        .context("Invalid incremental section output offset")?;
    let size = parts
        .next()
        .context("Malformed incremental section size")?
        .parse()
        .context("Invalid incremental section size")?;
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental section record"));
    }
    Ok(SectionRecord {
        input_file: input_file.into(),
        input: input.into(),
        section_index,
        output_offset,
        size,
    })
}

fn parse_section_input_line(line: &str) -> Result<(String, String)> {
    let rest = parse_prefixed_line(Some(line), "section-input")?;
    let mut parts = rest.split('\t');
    let input_file = parts
        .next()
        .context("Malformed incremental section input file")?
        .to_owned();
    let input = parts
        .next()
        .context("Malformed incremental section input")?
        .to_owned();
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental section input record"));
    }
    Ok((input_file, input))
}

fn parse_compact_section_line(
    line: &str,
    section_inputs: &[(String, String)],
) -> Result<SectionRecord> {
    let rest = parse_prefixed_line(Some(line), "section")?;
    let mut parts = rest.split('\t');
    let section_input_id: usize = parts
        .next()
        .context("Malformed incremental section input index")?
        .parse()
        .context("Invalid incremental section input index")?;
    let section_index = parts
        .next()
        .context("Malformed incremental section index")?
        .parse()
        .context("Invalid incremental section index")?;
    let output_offset = parts
        .next()
        .context("Malformed incremental section output offset")?
        .parse()
        .context("Invalid incremental section output offset")?;
    let size = parts
        .next()
        .context("Malformed incremental section size")?
        .parse()
        .context("Invalid incremental section size")?;
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental section record"));
    }
    let (input_file, input) = section_inputs
        .get(section_input_id)
        .context("Incremental section input index out of bounds")?;
    Ok(SectionRecord {
        input_file: input_file.clone().into(),
        input: input.clone().into(),
        section_index,
        output_offset,
        size,
    })
}

fn parse_compact_relocation_line(
    line: &str,
    section_inputs: &[(String, String)],
) -> Result<RelocationRecord> {
    let (compact_version, rest) = if line.starts_with("reloc3\t") {
        (3, parse_prefixed_line(Some(line), "reloc3")?)
    } else if line.starts_with("reloc2\t") {
        (2, parse_prefixed_line(Some(line), "reloc2")?)
    } else {
        (1, parse_prefixed_line(Some(line), "reloc")?)
    };
    let parts = rest.split('\t').collect::<Vec<_>>();
    let section_input_id: usize = parts
        .first()
        .copied()
        .context("Malformed incremental relocation input index")?
        .parse()
        .context("Invalid incremental relocation input index")?;
    let section_index = parts
        .get(1)
        .copied()
        .context("Malformed incremental relocation section index")?
        .parse()
        .context("Invalid incremental relocation section index")?;
    let target_symbol_id = parts
        .get(2)
        .copied()
        .context("Malformed incremental relocation target symbol")?
        .parse()
        .context("Invalid incremental relocation target symbol")?;
    let relocation_offset = parts
        .get(3)
        .copied()
        .context("Malformed incremental relocation input offset")?
        .parse()
        .context("Invalid incremental relocation input offset")?;
    let output_offset = parts
        .get(4)
        .copied()
        .context("Malformed incremental relocation output offset")?
        .parse()
        .context("Invalid incremental relocation output offset")?;
    let size = parts
        .get(5)
        .copied()
        .context("Malformed incremental relocation size")?
        .parse()
        .context("Invalid incremental relocation size")?;
    let kind = parts
        .get(6)
        .copied()
        .context("Malformed incremental relocation kind")?
        .parse()
        .context("Invalid incremental relocation kind")?;
    let addend = parts
        .get(7)
        .copied()
        .context("Malformed incremental relocation addend")?
        .parse()
        .context("Invalid incremental relocation addend")?;
    let (written_value, target_value, applied_target_value, target_name, target) =
        if compact_version >= 2 {
            let expected_len = if compact_version == 3 { 15 } else { 14 };
            if parts.len() != expected_len {
                return Err(crate::error!("Malformed incremental relocation record"));
            }
            let written_value = (parts[8] != ABSENT_FIELD)
                .then(|| {
                    parts[8]
                        .parse()
                        .context("Invalid incremental written relocation value")
                })
                .transpose()?;
            let target_value = parts[9]
                .parse()
                .context("Invalid incremental relocation target value")?;
            let (applied_target_value, target_name_index, target_input_index) =
                if compact_version == 3 {
                    (
                        (parts[10] != ABSENT_FIELD)
                            .then(|| {
                                parts[10]
                                    .parse()
                                    .context("Invalid incremental applied relocation target value")
                            })
                            .transpose()?,
                        11,
                        12,
                    )
                } else {
                    (None, 10, 11)
                };
            let target_name = (parts[target_name_index] != ABSENT_FIELD)
                .then(|| parts[target_name_index].to_owned());
            let target = if parts[target_input_index] == ABSENT_FIELD
                && parts[target_input_index + 1] == ABSENT_FIELD
                && parts[target_input_index + 2] == ABSENT_FIELD
            {
                None
            } else {
                let target_section_input_id: usize = parts[target_input_index]
                    .parse()
                    .context("Invalid incremental relocation target input index")?;
                let (target_input_file, target_input) = section_inputs
                    .get(target_section_input_id)
                    .context("Incremental relocation target input index out of bounds")?;
                Some(RelocationTargetRecord {
                    input_file: target_input_file.clone().into(),
                    input: target_input.clone().into(),
                    section_index: parts[target_input_index + 1]
                        .parse()
                        .context("Invalid incremental relocation target section index")?,
                    section_offset: parts[target_input_index + 2]
                        .parse()
                        .context("Invalid incremental relocation target section offset")?,
                })
            };
            (
                written_value,
                target_value,
                applied_target_value,
                target_name,
                target,
            )
        } else {
            match parts.len() {
                8 => (None, 0, None, None, None),
                10 => {
                    let target_value = parts[8]
                        .parse()
                        .context("Invalid incremental relocation target value")?;
                    let target_name = (parts[9] != ABSENT_FIELD).then(|| parts[9].to_owned());
                    (None, target_value, None, target_name, None)
                }
                14 => {
                    let target_value = parts[8]
                        .parse()
                        .context("Invalid incremental relocation target value")?;
                    let target_name = (parts[9] != ABSENT_FIELD).then(|| parts[9].to_owned());
                    let target = if parts[10] == ABSENT_FIELD
                        && parts[11] == ABSENT_FIELD
                        && parts[12] == ABSENT_FIELD
                        && parts[13] == ABSENT_FIELD
                    {
                        None
                    } else {
                        Some(RelocationTargetRecord {
                            input_file: parts[10].to_owned().into(),
                            input: parts[11].to_owned().into(),
                            section_index: parts[12]
                                .parse()
                                .context("Invalid incremental relocation target section index")?,
                            section_offset: parts[13]
                                .parse()
                                .context("Invalid incremental relocation target section offset")?,
                        })
                    };
                    (None, target_value, None, target_name, target)
                }
                15 => {
                    let written_value = (parts[8] != ABSENT_FIELD)
                        .then(|| {
                            parts[8]
                                .parse()
                                .context("Invalid incremental written relocation value")
                        })
                        .transpose()?;
                    let target_value = parts[9]
                        .parse()
                        .context("Invalid incremental relocation target value")?;
                    let target_name = (parts[10] != ABSENT_FIELD).then(|| parts[10].to_owned());
                    let target = if parts[11] == ABSENT_FIELD
                        && parts[12] == ABSENT_FIELD
                        && parts[13] == ABSENT_FIELD
                        && parts[14] == ABSENT_FIELD
                    {
                        None
                    } else {
                        Some(RelocationTargetRecord {
                            input_file: parts[11].to_owned().into(),
                            input: parts[12].to_owned().into(),
                            section_index: parts[13]
                                .parse()
                                .context("Invalid incremental relocation target section index")?,
                            section_offset: parts[14]
                                .parse()
                                .context("Invalid incremental relocation target section offset")?,
                        })
                    };
                    (written_value, target_value, None, target_name, target)
                }
                _ => return Err(crate::error!("Malformed incremental relocation record")),
            }
        };
    let (input_file, input) = section_inputs
        .get(section_input_id)
        .context("Incremental relocation input index out of bounds")?;
    Ok(RelocationRecord {
        target_symbol_id,
        written_value,
        target_value,
        applied_target_value,
        target_name: target_name.map(Into::into),
        target,
        input_file: input_file.clone().into(),
        input: input.clone().into(),
        section_index,
        relocation_offset,
        output_offset,
        size,
        kind,
        addend,
    })
}

fn parse_compact_fde_line(line: &str, section_inputs: &[(String, String)]) -> Result<FdeRecord> {
    let rest = parse_prefixed_line(Some(line), "fde")?;
    let mut parts = rest.split('\t');
    let section_input_id: usize = parts
        .next()
        .context("Malformed incremental FDE input index")?
        .parse()
        .context("Invalid incremental FDE input index")?;
    let section_index = parts
        .next()
        .context("Malformed incremental FDE section index")?
        .parse()
        .context("Invalid incremental FDE section index")?;
    let eh_frame_section_index = parts
        .next()
        .context("Malformed incremental FDE .eh_frame section index")?
        .parse()
        .context("Invalid incremental FDE .eh_frame section index")?;
    let input_offset = parts
        .next()
        .context("Malformed incremental FDE input offset")?
        .parse()
        .context("Invalid incremental FDE input offset")?;
    let output_offset = parts
        .next()
        .context("Malformed incremental FDE output offset")?
        .parse()
        .context("Invalid incremental FDE output offset")?;
    let size = parts
        .next()
        .context("Malformed incremental FDE size")?
        .parse()
        .context("Invalid incremental FDE size")?;
    if parts.next().is_some() {
        return Err(crate::error!("Malformed incremental FDE record"));
    }
    let (input_file, input) = section_inputs
        .get(section_input_id)
        .context("Incremental FDE input index out of bounds")?;
    Ok(FdeRecord {
        input_file: input_file.clone().into(),
        input: input.clone().into(),
        section_index,
        eh_frame_section_index,
        input_offset,
        output_offset,
        size,
    })
}

fn parse_compact_dynamic_relocation_line(
    line: &str,
    section_inputs: &[(String, String)],
) -> Result<DynamicRelocationRecord> {
    let rest = parse_prefixed_line(Some(line), "dynrel")?;
    let parts = rest.split('\t').collect::<Vec<_>>();
    let section_input_id: usize = parts
        .first()
        .copied()
        .context("Malformed incremental dynamic relocation input index")?
        .parse()
        .context("Invalid incremental dynamic relocation input index")?;
    let section_index = parts
        .get(1)
        .copied()
        .context("Malformed incremental dynamic relocation section index")?
        .parse()
        .context("Invalid incremental dynamic relocation section index")?;
    let (relocation_offset, output_offset_index, size_index, output_info_indices) =
        match parts.len() {
            4 => (0, 2, 3, None),
            5 => {
                let relocation_offset = parts[2]
                    .parse()
                    .context("Invalid incremental dynamic relocation input offset")?;
                (relocation_offset, 3, 4, None)
            }
            7 => {
                let relocation_offset = parts[2]
                    .parse()
                    .context("Invalid incremental dynamic relocation input offset")?;
                (relocation_offset, 3, 4, Some((5, 6)))
            }
            _ => {
                return Err(crate::error!(
                    "Malformed incremental dynamic relocation record"
                ));
            }
        };
    let output_offset = parts[output_offset_index]
        .parse()
        .context("Invalid incremental dynamic relocation output offset")?;
    let size = parts[size_index]
        .parse()
        .context("Invalid incremental dynamic relocation size")?;
    let (output_r_offset, output_r_info) =
        if let Some((r_offset_index, r_info_index)) = output_info_indices {
            (
                Some(
                    parts[r_offset_index]
                        .parse()
                        .context("Invalid incremental dynamic relocation output r_offset")?,
                ),
                Some(
                    parts[r_info_index]
                        .parse()
                        .context("Invalid incremental dynamic relocation output r_info")?,
                ),
            )
        } else {
            (None, None)
        };
    let (input_file, input) = section_inputs
        .get(section_input_id)
        .context("Incremental dynamic relocation input index out of bounds")?;
    Ok(DynamicRelocationRecord {
        input_file: input_file.clone().into(),
        input: input.clone().into(),
        section_index,
        relocation_offset,
        output_offset,
        size,
        output_r_offset,
        output_r_info,
    })
}

fn snapshot_loaded_files(
    state_dir: &Path,
    file_loader: &FileLoader<'_>,
    input_files: &mut [FileState],
    sections: &[SectionRecord],
) -> Result<usize> {
    snapshot_loaded_input_files(
        state_dir,
        &file_loader.loaded_files,
        input_files,
        sections,
        true,
    )
}

fn snapshot_loaded_input_files(
    state_dir: &Path,
    loaded_files: &[&InputFile],
    input_files: &mut [FileState],
    sections: &[SectionRecord],
    hash_inputs: bool,
) -> Result<usize> {
    let patchable_inputs = sections
        .par_iter()
        .fold(HashSet::new, |mut inputs, section| {
            inputs.insert(section.input_file.as_str());
            inputs
        })
        .reduce(HashSet::new, |mut inputs, shard| {
            inputs.extend(shard);
            inputs
        });
    let input_indices = input_files
        .iter()
        .enumerate()
        .map(|(index, input)| (input.path.clone(), index))
        .collect::<HashMap<_, _>>();
    for input in input_files.iter_mut() {
        input.patch = None;
    }

    let mut seen = HashSet::new();
    let mut tasks = Vec::new();
    for input_file in loaded_files {
        let path = encode_path(&input_file.filename);
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(index) = input_indices.get(&path).copied() else {
            continue;
        };
        let is_patchable = patchable_inputs.contains(path.as_str());
        tasks.push((
            index,
            *input_file,
            is_patchable,
            input_files[index].content.hash.is_empty(),
        ));
    }

    let results = tasks
        .into_par_iter()
        .map(|(index, input_file, is_patchable, should_hash)| {
            let should_hash = should_hash && hash_inputs;
            if !is_patchable {
                return Ok((
                    index,
                    false,
                    should_hash.then(|| hash_loaded_input_bytes(input_file.full_data())),
                    None,
                ));
            }
            let (did_snapshot, hash, snapshot_identity) = snapshot_loaded_input_file(
                state_dir,
                &input_file.filename,
                input_file.full_data(),
                should_hash,
            )?;
            Ok((index, did_snapshot, hash, snapshot_identity))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut snapshotted = 0;
    for (index, did_snapshot, hash, snapshot_identity) in results {
        if let Some(hash) = hash {
            input_files[index].content.hash = hash;
        }
        if did_snapshot {
            snapshotted += 1;
        }
        input_files[index].snapshot_identity = snapshot_identity;
    }
    Ok(snapshotted)
}

fn hash_loaded_input_files(loaded_files: &[&InputFile], input_files: &mut [FileState]) {
    let input_indices = input_files
        .iter()
        .enumerate()
        .map(|(index, input)| (input.path.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let tasks = loaded_files
        .iter()
        .filter_map(|input_file| {
            let path = encode_path(&input_file.filename);
            if !seen.insert(path.clone()) {
                return None;
            }
            let index = input_indices.get(&path).copied()?;
            input_files[index]
                .content
                .hash
                .is_empty()
                .then_some((index, *input_file))
        })
        .collect::<Vec<_>>();
    let hashes = tasks
        .into_par_iter()
        .map(|(index, input_file)| (index, hash_loaded_input_bytes(input_file.full_data())))
        .collect::<Vec<_>>();
    for (index, hash) in hashes {
        input_files[index].content.hash = hash;
    }
}

fn hash_pending_reuse_input_files(
    loaded_files: &[&InputFile],
    input_files: &mut [FileState],
    link_start: Option<&FileIdentity>,
) {
    let input_indices = input_files
        .iter()
        .enumerate()
        .map(|(index, input)| (input.path.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let tasks = loaded_files
        .iter()
        .filter_map(|input_file| {
            let path = encode_path(&input_file.filename);
            if !seen.insert(path.clone()) {
                return None;
            }
            let index = input_indices.get(&path).copied()?;
            let input = &input_files[index];
            (input.content.hash.is_empty()
                && !input_content_is_anchored_before_link_start(input, link_start)
                && input.content.identity_is_ambiguous_since(link_start))
            .then_some((index, *input_file))
        })
        .collect::<Vec<_>>();
    let hashes = tasks
        .into_par_iter()
        .map(|(index, input_file)| (index, hash_loaded_input_bytes(input_file.full_data())))
        .collect::<Vec<_>>();
    for (index, hash) in hashes {
        input_files[index].content.hash = hash;
    }
}

fn input_content_matches_previous(
    _state_dir: &Path,
    previous_input: &FileState,
    current_path: &Path,
) -> Result<bool> {
    if previous_input.content.hash.is_empty() {
        // Filesystem timestamps cannot reliably distinguish a rapid same-size in-place mutation
        // of a snapshot or a hardlinked source. Legacy hashless states must relink once.
        return Ok(false);
    }
    let bytes = match std::fs::read(current_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(content_state_matches_previous(
        &previous_input.content,
        &FileContentState::from_bytes(&bytes),
    ))
}

fn content_state_matches_previous(previous: &FileContentState, current: &FileContentState) -> bool {
    !previous.hash.is_empty() && previous.len == current.len && previous.hash == current.hash
}

fn output_content_matches_previous(
    previous: &FileContentState,
    path: &Path,
    trust_persistent_output_data_identity: bool,
) -> Result<bool> {
    if previous.identity_matches_path(path)? {
        return Ok(true);
    }
    if previous.hash.is_empty() {
        return if trust_persistent_output_data_identity {
            previous.data_identity_matches_path(path)
        } else {
            Ok(false)
        };
    }
    Ok(FileContentState::from_path(path)? == *previous)
}

fn install_output_snapshot(state_dir: &Path, output: &Path) -> Result {
    std::fs::create_dir_all(state_dir).with_context(|| {
        format!(
            "Failed to create incremental output snapshot directory `{}`",
            state_dir.display()
        )
    })?;
    install_isolated_output_copy(output, &output_snapshot_path(state_dir)).with_context(|| {
        format!(
            "Failed to retain incremental output snapshot for `{}`",
            output.display()
        )
    })
}

fn update_output_snapshot_from_ranges(
    state_dir: &Path,
    output: &Path,
    ranges: &[std::ops::Range<usize>],
) -> Result {
    let snapshot = output_snapshot_path(state_dir);
    let output_len = std::fs::metadata(output)
        .with_context(|| format!("Failed to read incremental output `{}`", output.display()))?
        .len();
    let Ok(mut snapshot_file) = OpenOptions::new().read(true).write(true).open(&snapshot) else {
        return install_output_snapshot(state_dir, output);
    };
    if snapshot_file.metadata()?.len() != output_len {
        return install_output_snapshot(state_dir, output);
    }

    let mut output_file = std::fs::File::open(output)
        .with_context(|| format!("Failed to read incremental output `{}`", output.display()))?;
    for range in ranges {
        let start = u64::try_from(range.start).context("Incremental snapshot range overflow")?;
        let end = u64::try_from(range.end).context("Incremental snapshot range overflow")?;
        if start > end || end > output_len {
            return Err(crate::error!("Incremental snapshot range is invalid"));
        }
        let mut bytes = vec![0; range.len()];
        output_file.seek(SeekFrom::Start(start))?;
        output_file.read_exact(&mut bytes)?;
        snapshot_file.seek(SeekFrom::Start(start))?;
        snapshot_file.write_all(&bytes)?;
    }
    Ok(())
}

fn restore_missing_output_snapshot(
    state_dir: &Path,
    previous: &FileContentState,
    output: &Path,
) -> Result<bool> {
    if !retained_output_snapshot_matches_previous(state_dir, previous, output)? {
        return Ok(false);
    }
    let snapshot = output_snapshot_path(state_dir);
    install_isolated_output_copy(&snapshot, output).with_context(|| {
        format!(
            "Failed to restore incremental output snapshot to `{}`",
            output.display()
        )
    })?;
    append_log(state_dir, "restored missing output from retained snapshot")?;
    Ok(true)
}

fn retained_output_snapshot_matches_previous(
    state_dir: &Path,
    previous: &FileContentState,
    output: &Path,
) -> Result<bool> {
    if output.try_exists().unwrap_or(false) || previous.hash.is_empty() {
        return Ok(false);
    }
    let snapshot = output_snapshot_path(state_dir);
    Ok(snapshot.try_exists().unwrap_or(false)
        && FileContentState::from_path(&snapshot)? == *previous)
}

fn install_isolated_output_copy(source: &Path, target: &Path) -> Result {
    let tmp = target.with_file_name(format!(
        "{}.{}.tmp",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);

    if !clone_snapshot_bytes(source, &tmp) {
        std::fs::copy(source, &tmp).with_context(|| {
            format!(
                "Failed to copy incremental output snapshot `{}` to `{}`",
                source.display(),
                tmp.display()
            )
        })?;
    }
    let permissions = std::fs::metadata(source)?.permissions();
    std::fs::set_permissions(&tmp, permissions)?;
    let _ = std::fs::remove_file(target);
    std::fs::rename(&tmp, target).with_context(|| {
        format!(
            "Failed to install incremental output snapshot `{}`",
            target.display()
        )
    })
}

fn read_verified_input_snapshot(
    state_dir: &Path,
    previous_input: &FileState,
) -> Result<Option<Vec<u8>>> {
    let snapshot = input_snapshot_path_for_encoded_path(state_dir, &previous_input.path);
    let bytes = match std::fs::read(&snapshot) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let compressed_snapshot =
                compressed_input_snapshot_path_for_encoded_path(state_dir, &previous_input.path);
            let bytes = match std::fs::read(&compressed_snapshot) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            zstd::stream::decode_all(bytes.as_slice()).with_context(|| {
                format!(
                    "Failed to decompress incremental input snapshot `{}`",
                    compressed_snapshot.display()
                )
            })?
        }
        Err(error) => return Err(error.into()),
    };
    if !snapshot_bytes_match_previous_content(previous_input, &bytes)
        && !snapshot_identity_matches_previous_atomic_replacement_input(
            previous_input,
            &snapshot,
            rustc_work_product_provenance_enabled(),
        )
    {
        return Ok(None);
    }
    Ok(Some(
        crate::file_kind::FileKind::select_input_bytes(&bytes)?.to_vec(),
    ))
}

fn snapshot_identity_matches_previous_atomic_replacement_input(
    previous_input: &FileState,
    snapshot: &Path,
    trust_rustc_work_product_provenance: bool,
) -> bool {
    if !trust_rustc_work_product_provenance || !previous_input.content.hash.is_empty() {
        return false;
    }
    let Ok(path) = decode_path(&previous_input.path) else {
        return false;
    };
    if !is_atomic_replacement_rust_input(&path) {
        return false;
    }
    // rustc publishes these inputs by rename. A retained same-data inode therefore keeps the
    // immutable bytes used by the prior patch without needing a second full-file hash.
    previous_input
        .snapshot_identity
        .as_ref()
        .zip(previous_input.content.identity.as_ref())
        .zip(FileIdentity::from_path(snapshot).ok().flatten().as_ref())
        .is_some_and(|((expected, input), current)| {
            expected == current && expected.matches_same_data_ignoring_change_time(input)
        })
}

fn snapshot_bytes_match_previous_content(previous_input: &FileState, bytes: &[u8]) -> bool {
    let previous = &previous_input.content;
    if previous.len != bytes.len() as u64 {
        return false;
    }
    if previous.hash.is_empty() {
        return false;
    }
    previous.hash == hash_bytes(bytes)
}

fn read_file_with_stable_identity(path: &Path) -> Result<Option<(Vec<u8>, FileContentState)>> {
    read_file_with_stable_identity_and_hashing(path, true)
}

fn read_file_with_stable_identity_and_hashing(
    path: &Path,
    should_hash: bool,
) -> Result<Option<(Vec<u8>, FileContentState)>> {
    let before = FileIdentity::from_path(path)?;
    let full_bytes = {
        verbose_timing_phase!("Read stable input bytes");
        std::fs::read(path).with_context(|| format!("Failed to read `{}`", path.display()))?
    };
    let after = FileIdentity::from_path(path)?;
    if before != after {
        return Ok(None);
    }
    let Some(identity) = after else {
        let mut content = FileContentState {
            len: full_bytes.len() as u64,
            hash: String::new(),
            identity: None,
        };
        let bytes = crate::file_kind::FileKind::select_input_bytes(&full_bytes)?.to_vec();
        if should_hash || bytes.len() != full_bytes.len() {
            ensure_loaded_input_content_hash(&full_bytes, &mut content);
        }
        return Ok(Some((bytes, content)));
    };
    if full_bytes.len() as u64 != identity.len {
        return Ok(None);
    }
    let mut content = FileContentState {
        len: identity.len,
        hash: String::new(),
        identity: Some(identity),
    };
    let bytes = crate::file_kind::FileKind::select_input_bytes(&full_bytes)?.to_vec();
    if should_hash || bytes.len() != full_bytes.len() {
        ensure_loaded_input_content_hash(&full_bytes, &mut content);
    }
    Ok(Some((bytes, content)))
}

fn ensure_loaded_input_content_hash(bytes: &[u8], content: &mut FileContentState) {
    if content.hash.is_empty() {
        verbose_timing_phase!("Hash stable input bytes");
        content.hash = hash_loaded_input_bytes(bytes);
    }
}

fn hash_deferred_loaded_input_contents(
    inputs: &[(usize, usize, Vec<u8>)],
    allow_parallel_hashing: bool,
) -> Vec<(usize, usize, String)> {
    inputs
        .iter()
        .map(|(input_index, expected_input_index, bytes)| {
            verbose_timing_phase!("Hash stable input bytes");
            let hash = if allow_parallel_hashing {
                hash_loaded_input_bytes(bytes)
            } else {
                hash_bytes(bytes)
            };
            (*input_index, *expected_input_index, hash)
        })
        .collect()
}

fn install_deferred_loaded_input_content_hashes(
    input_files: &mut [FileState],
    expected_inputs: &mut [ExpectedInputContent],
    hashes: Vec<(usize, usize, String)>,
) -> Result {
    for (input_index, expected_input_index, hash) in hashes {
        let input = input_files
            .get_mut(input_index)
            .context("Missing deferred incremental input hash target")?;
        input.content.hash.clone_from(&hash);
        let expected = expected_inputs
            .get_mut(expected_input_index)
            .context("Missing deferred incremental input validation target")?;
        expected.hash = hash;
    }
    Ok(())
}

fn input_content_mismatch_reason(
    expected_inputs: &[ExpectedInputContent],
    installed_snapshot_state_dir: Option<&Path>,
) -> Option<String> {
    for expected in expected_inputs {
        // Rust installs these artifacts by atomic replacement. A matching identity still names
        // the bytes already used for the patch, avoiding two large redundant reads per edit.
        if expected.matches_unchanged_atomic_replacement_input()
            || installed_snapshot_state_dir.is_some_and(|state_dir| {
                expected.matches_installed_atomic_replacement_snapshot(state_dir)
            })
        {
            continue;
        }
        let current = match read_file_with_stable_identity(&expected.path) {
            Ok(Some((_, content))) => content,
            Ok(None) => {
                return Some(format!(
                    "input file changed while incremental fast path was running: {}",
                    expected.path.display()
                ));
            }
            Err(error) => {
                return Some(format!(
                    "input file could not be rechecked while incremental fast path was running: {} ({error:?})",
                    expected.path.display()
                ));
            }
        };
        if current.len != expected.len || current.hash != expected.hash {
            return Some(format!(
                "input file changed while incremental fast path was running: {}",
                expected.path.display()
            ));
        }
    }
    None
}

fn input_identity_mismatch_reason(input_files: &[FileState]) -> Result<Option<String>> {
    input_files
        .par_iter()
        .map(|input| {
            let path = decode_path(&input.path)?;
            match input.content.identity_matches_path(&path) {
                Ok(true) => Ok(None),
                Ok(false) => Ok(Some(format!(
                    "input file changed while incremental fast path was running: {}",
                    path.display()
                ))),
                Err(error) => Ok(Some(format!(
                    "input file could not be rechecked while incremental fast path was running: {} ({error:?})",
                    path.display()
                ))),
            }
        })
        .try_reduce(|| None, |left, right| Ok(left.or(right)))
}

fn refresh_input_file_identities(input_files: &mut [FileState]) {
    for input in input_files {
        refresh_input_file_identity(input);
    }
}

fn refresh_input_file_identities_at_indices(
    input_files: &mut [FileState],
    indices: impl IntoIterator<Item = usize>,
) {
    let mut seen = HashSet::new();
    for index in indices {
        if !seen.insert(index) {
            continue;
        }
        let Some(input) = input_files.get_mut(index) else {
            continue;
        };
        refresh_input_file_identity(input);
    }
}

fn refresh_input_snapshot_identities_at_indices(
    state_dir: &Path,
    input_files: &mut [FileState],
    indices: impl IntoIterator<Item = usize>,
) {
    let mut seen = HashSet::new();
    for index in indices {
        if !seen.insert(index) {
            continue;
        }
        let Some(input) = input_files.get_mut(index) else {
            continue;
        };
        let snapshot = input_snapshot_path_for_encoded_path(state_dir, &input.path);
        input.snapshot_identity = FileIdentity::from_path(&snapshot).ok().flatten();
    }
}

fn refresh_input_file_identity(input: &mut FileState) {
    let Ok(path) = decode_path(&input.path) else {
        return;
    };
    let Ok(Some(identity)) = FileIdentity::from_path(&path) else {
        return;
    };
    input.content.len = identity.len;
    input.content.identity = Some(identity);
}

fn snapshot_input_paths<'a>(
    state_dir: &Path,
    paths: impl IntoIterator<Item = &'a Path>,
) -> Result<usize> {
    let mut seen = HashSet::new();
    let mut snapshotted = 0;
    for path in paths {
        if !seen.insert(encode_path(path)) {
            continue;
        }
        if snapshot_input_path(state_dir, path)? {
            snapshotted += 1;
        }
    }
    Ok(snapshotted)
}

fn changed_inputs_requiring_snapshot<'a>(
    changed_inputs: &'a [(usize, PathBuf)],
    rustc_link_content_digest_unchanged_input_indices: &'a HashSet<usize>,
) -> impl Iterator<Item = &'a (usize, PathBuf)> {
    // Matching producer digests prove link equivalence without proving byte equality. Keep the old
    // snapshot untouched: the persisted empty content hash and cleared snapshot identity prevent
    // its stale bytes from being trusted by a later patch.
    changed_inputs.iter().filter(|(input_index, _)| {
        !rustc_link_content_digest_unchanged_input_indices.contains(input_index)
    })
}

fn snapshot_input_path(state_dir: &Path, path: &Path) -> Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(false),
    };
    if !metadata.is_file()
        || (metadata.permissions().readonly() && !is_protected_stable_rustc_input(state_dir, path))
    {
        return Ok(false);
    }

    let snapshot_dir = input_snapshot_dir(state_dir);
    std::fs::create_dir_all(&snapshot_dir).with_context(|| {
        format!(
            "Failed to create incremental input snapshot directory `{}`",
            snapshot_dir.display()
        )
    })?;

    let target = input_snapshot_path(state_dir, path);
    let tmp = target.with_file_name(format!(
        "{}.{}.tmp",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("input"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);

    copy_snapshot_bytes(path, &tmp)?;

    let _ = std::fs::remove_file(&target);
    std::fs::rename(&tmp, &target).with_context(|| {
        format!(
            "Failed to install incremental input snapshot `{}`",
            target.display()
        )
    })?;
    let _ = std::fs::remove_file(compressed_input_snapshot_path(state_dir, path));
    Ok(true)
}

fn snapshot_loaded_input_file(
    state_dir: &Path,
    path: &Path,
    bytes: &[u8],
    should_hash: bool,
) -> Result<(bool, Option<String>, Option<FileIdentity>)> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok((false, should_hash.then(|| hash_bytes(bytes)), None)),
    };
    if !metadata.is_file()
        || (metadata.permissions().readonly() && !is_protected_stable_rustc_input(state_dir, path))
    {
        return Ok((false, should_hash.then(|| hash_bytes(bytes)), None));
    }

    let snapshot_dir = input_snapshot_dir(state_dir);
    std::fs::create_dir_all(&snapshot_dir).with_context(|| {
        format!(
            "Failed to create incremental input snapshot directory `{}`",
            snapshot_dir.display()
        )
    })?;

    let target = input_snapshot_path(state_dir, path);
    // Fresh Rust seeds do not need a temporary snapshot name. If a prior snapshot already
    // exists, this hardlink attempt fails and we retain the atomic replacement path below.
    if hardlink_rust_snapshot_bytes(path, &target) {
        let _ = std::fs::remove_file(compressed_input_snapshot_path(state_dir, path));
        let snapshot_identity = FileIdentity::from_path(&target)?;
        let hash = should_hash.then(|| hash_loaded_input_bytes(bytes));
        return Ok((true, hash, snapshot_identity));
    }
    let tmp = target.with_file_name(format!(
        "{}.{}.tmp",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("input"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);

    if !(hardlink_rust_snapshot_bytes(path, &tmp) || clone_snapshot_bytes(path, &tmp)) {
        let compressed_target = compressed_input_snapshot_path(state_dir, path);
        let compressed_tmp = compressed_target.with_file_name(format!(
            "{}.{}.tmp",
            compressed_target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("input.zstd"),
            std::process::id()
        ));
        let _ = std::fs::remove_file(&compressed_tmp);
        let hash = write_compressed_loaded_snapshot(bytes, &compressed_tmp, should_hash)?;
        let _ = std::fs::remove_file(&compressed_target);
        std::fs::rename(&compressed_tmp, &compressed_target).with_context(|| {
            format!(
                "Failed to install compressed incremental input snapshot `{}`",
                compressed_target.display()
            )
        })?;
        let _ = std::fs::remove_file(&target);
        return Ok((true, hash, None));
    }
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&tmp, &target).with_context(|| {
        format!(
            "Failed to install incremental input snapshot `{}`",
            target.display()
        )
    })?;
    let _ = std::fs::remove_file(compressed_input_snapshot_path(state_dir, path));
    let snapshot_identity = FileIdentity::from_path(&target)?;
    // Filesystem identities cannot reliably detect rapid same-size writes. Snapshot-backed
    // states must keep a content hash even when the bytes are cloned or hardlinked.
    let hash = should_hash.then(|| hash_loaded_input_bytes(bytes));
    Ok((true, hash, snapshot_identity))
}

fn copy_snapshot_bytes(source: &Path, target: &Path) -> Result {
    if hardlink_rust_snapshot_bytes(source, target) || clone_snapshot_bytes(source, target) {
        return Ok(());
    }
    copy_file_bytes(source, target)
}

fn copy_isolated_snapshot_bytes(source: &Path, target: &Path) -> Result {
    if !clone_snapshot_bytes(source, target) {
        copy_file_bytes(source, target)?;
    }
    let mut permissions = std::fs::metadata(target)
        .with_context(|| {
            format!(
                "Failed to read isolated incremental input snapshot `{}`",
                target.display()
            )
        })?
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(target, permissions).with_context(|| {
        format!(
            "Failed to protect isolated incremental input snapshot `{}`",
            target.display()
        )
    })
}

fn copy_file_bytes(source: &Path, target: &Path) -> Result {
    let mut input = std::fs::File::open(source)
        .with_context(|| format!("Failed to read incremental input `{}`", source.display()))?;
    let mut output = std::fs::File::create(target).with_context(|| {
        format!(
            "Failed to create incremental input snapshot `{}`",
            target.display()
        )
    })?;
    std::io::copy(&mut input, &mut output).with_context(|| {
        format!(
            "Failed to copy incremental input snapshot `{}` to `{}`",
            source.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn write_compressed_loaded_snapshot(
    bytes: &[u8],
    target: &Path,
    should_hash: bool,
) -> Result<Option<String>> {
    let output = std::fs::File::create(target).with_context(|| {
        format!(
            "Failed to create compressed incremental input snapshot `{}`",
            target.display()
        )
    })?;
    let mut encoder = zstd::stream::Encoder::new(output, INPUT_SNAPSHOT_COMPRESSION_LEVEL)
        .context("Failed to initialize incremental input snapshot compression")?;
    let mut hasher = should_hash.then(blake3::Hasher::new);
    for chunk in bytes.chunks(64 * 1024) {
        encoder.write_all(chunk).with_context(|| {
            format!(
                "Failed to write compressed incremental input snapshot `{}`",
                target.display()
            )
        })?;
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(chunk);
        }
    }
    encoder
        .finish()
        .context("Failed to finish incremental input snapshot compression")?;
    Ok(hasher.map(|hasher| hasher.finalize().to_hex().to_string()))
}

fn hash_loaded_input_bytes(bytes: &[u8]) -> String {
    const PARALLEL_HASH_THRESHOLD: usize = 256 * 1024;
    if bytes.len() < PARALLEL_HASH_THRESHOLD {
        return hash_bytes(bytes);
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update_rayon(bytes);
    hasher.finalize().to_hex().to_string()
}

fn hardlink_rust_snapshot_bytes(source: &Path, target: &Path) -> bool {
    if !is_atomic_replacement_rust_input(source) {
        return false;
    }
    // rustc installs these outputs by replacement, preserving the linked inode as old input bytes.
    // If a producer mutates one in place instead, the saved content hash rejects reuse.
    std::fs::hard_link(source, target).is_ok()
}

fn is_atomic_replacement_rust_input(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "rlib")
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(".rcgu.o"))
}

fn is_protected_stable_rustc_input(state_dir: &Path, path: &Path) -> bool {
    is_atomic_replacement_rust_input(path)
        && path
            .parent()
            .is_some_and(|parent| parent == state_dir.join(STABLE_RUSTC_INPUT_DIR))
}

#[cfg(target_vendor = "apple")]
fn clone_snapshot_bytes(source: &Path, target: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(source) = CString::new(source.as_os_str().as_bytes()) else {
        return false;
    };
    let Ok(target) = CString::new(target.as_os_str().as_bytes()) else {
        return false;
    };

    // `clonefile` creates an APFS copy-on-write clone, so the snapshot keeps
    // copy semantics without paying to duplicate every input byte up front.
    unsafe { libc::clonefile(source.as_ptr(), target.as_ptr(), 0) == 0 }
}

#[cfg(target_os = "linux")]
fn clone_snapshot_bytes(source: &Path, target: &Path) -> bool {
    const FICLONE: libc::Ioctl = 0x4004_9409;

    let Ok(source) = std::fs::File::open(source) else {
        return false;
    };
    let Ok(output) = OpenOptions::new().create_new(true).write(true).open(target) else {
        return false;
    };
    // SAFETY: Both file descriptors are live for the duration of this ioctl, and FICLONE
    // copies data into `output` without retaining either descriptor.
    if unsafe { libc::ioctl(output.as_raw_fd(), FICLONE, source.as_raw_fd()) } == 0 {
        return true;
    }
    drop(output);
    let _ = std::fs::remove_file(target);
    false
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn clone_snapshot_bytes(_source: &Path, _target: &Path) -> bool {
    false
}

fn input_snapshot_path(state_dir: &Path, path: &Path) -> PathBuf {
    input_snapshot_path_for_encoded_path(state_dir, &encode_path(path))
}

fn input_snapshot_path_for_encoded_path(state_dir: &Path, encoded_path: &str) -> PathBuf {
    input_snapshot_dir(state_dir).join(hash_text(encoded_path))
}

fn compressed_input_snapshot_path(state_dir: &Path, path: &Path) -> PathBuf {
    compressed_input_snapshot_path_for_encoded_path(state_dir, &encode_path(path))
}

fn compressed_input_snapshot_path_for_encoded_path(
    state_dir: &Path,
    encoded_path: &str,
) -> PathBuf {
    let mut path = input_snapshot_path_for_encoded_path(state_dir, encoded_path).into_os_string();
    path.push(COMPRESSED_INPUT_SNAPSHOT_SUFFIX);
    PathBuf::from(path)
}

fn input_snapshot_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(INPUT_SNAPSHOT_DIR)
}

fn output_snapshot_path(state_dir: &Path) -> PathBuf {
    state_dir.join(OUTPUT_SNAPSHOT_FILE)
}

fn interrupted_update_relink_reason(state_dir: &Path) -> Option<String> {
    match update_marker_path(state_dir).try_exists() {
        Ok(true) => Some("previous incremental update did not complete".to_owned()),
        Ok(false) => None,
        Err(error) => Some(format!(
            "previous incremental update status could not be checked: {error:?}"
        )),
    }
}

fn acquire_incremental_state_lock(state_dir: &Path) -> Result<IncrementalStateLock> {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(STATE_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("Failed to open incremental state lock `{}`", path.display()))?;
    #[cfg(unix)]
    // A child may keep publishing state after its parent has reported a usable output.
    // Serialize state readers and writers so a following link cannot observe partial state.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to lock incremental state `{}`", path.display()));
    }
    Ok(IncrementalStateLock { _file: file })
}

fn mark_incremental_update_started(state_dir: &Path, operation: &str) -> Result {
    std::fs::create_dir_all(state_dir)?;
    let path = update_marker_path(state_dir);
    std::fs::write(&path, format!("{operation}\n")).with_context(|| {
        format!(
            "Failed to write incremental update marker `{}`",
            path.display()
        )
    })
}

fn clear_incremental_update_marker(state_dir: &Path) -> Result {
    let path = update_marker_path(state_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "Failed to remove incremental update marker `{}`",
                path.display()
            )
        }),
    }
}

fn remove_incremental_index(state_dir: &Path) -> Result {
    let path = state_dir.join(INDEX_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "Failed to remove interrupted incremental state `{}`",
                path.display()
            )
        }),
    }
}

fn write_link_start_marker(state_dir: &Path) -> Result<Option<FileIdentity>> {
    std::fs::create_dir_all(state_dir)?;
    let path = link_start_marker_path(state_dir);
    std::fs::write(&path, b"link started\n").with_context(|| {
        format!(
            "Failed to write incremental link-start marker `{}`",
            path.display()
        )
    })?;
    Ok(link_start_marker_identity(state_dir))
}

fn link_start_marker_identity(state_dir: &Path) -> Option<FileIdentity> {
    FileIdentity::from_path(&link_start_marker_path(state_dir))
        .ok()
        .flatten()
}

fn link_start_marker_path(state_dir: &Path) -> PathBuf {
    state_dir.join(LINK_START_FILE)
}

fn update_marker_path(state_dir: &Path) -> PathBuf {
    state_dir.join(UPDATE_MARKER_FILE)
}

fn append_log(state_dir: &Path, message: &str) -> Result {
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(LOG_FILE);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open incremental log `{}`", path.display()))?;
    writeln!(file, "{message}")?;
    let _ = append_global_log(state_dir, message);
    Ok(())
}

fn log_incremental_link_options_if_requested(
    args: &impl platform::Args,
    state_dir: &Path,
    log_link_options: bool,
    log_exact_args: bool,
) -> Result<()> {
    if log_link_options {
        append_log(
            state_dir,
            &format!(
                "incremental link options: {}",
                args.incremental_link_options()
            ),
        )?;
    }
    if log_exact_args {
        append_log(state_dir, &format!("incremental exact args: {args:?}"))?;
    }
    Ok(())
}

fn append_global_log(state_dir: &Path, message: &str) -> Result {
    let Some(log_dir) = user_state_dir() else {
        return Ok(());
    };
    append_global_log_to(&log_dir, state_dir, message)
}

fn append_global_log_to(log_dir: &Path, state_dir: &Path, message: &str) -> Result {
    std::fs::create_dir_all(log_dir)?;
    let path = global_log_path_in(log_dir);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open incremental global log `{}`", path.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    writeln!(file, "{timestamp}\t{}\t{message}", state_dir.display())?;
    Ok(())
}

pub(crate) fn print_global_log(mut writer: impl std::io::Write) -> Result {
    let Some(log_dir) = user_state_dir() else {
        return Ok(());
    };
    print_global_log_from(&log_dir, &mut writer)
}

fn print_global_log_from(log_dir: &Path, writer: &mut impl std::io::Write) -> Result {
    let path = global_log_path_in(log_dir);
    let contents = match std::fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read incremental log `{}`", path.display()));
        }
    };
    writer
        .write_all(&contents)
        .with_context(|| format!("Failed to write incremental log `{}`", path.display()))?;
    Ok(())
}

fn global_log_path_in(log_dir: &Path) -> PathBuf {
    log_dir.join(GLOBAL_LOG_FILE)
}

fn metadata_update_path(state_dir: &Path) -> PathBuf {
    state_dir.join(METADATA_UPDATE_FILE)
}

fn should_filter_sections_sidecar(state_dir: &Path, sections_file: &str) -> bool {
    const LARGE_SECTIONS_SIDECAR: u64 = 64 * 1024 * 1024;
    if validate_sections_file_name(sections_file).is_err() {
        return false;
    }
    std::fs::metadata(state_dir.join(sections_file))
        .is_ok_and(|metadata| metadata.len() > LARGE_SECTIONS_SIDECAR)
}

fn user_state_dir() -> Option<PathBuf> {
    user_state_dir_from_env(|name| std::env::var_os(name))
}

fn user_state_dir_from_env(mut env: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = env(USER_STATE_DIR_ENV) {
        return Some(PathBuf::from(path));
    }

    #[cfg(target_os = "macos")]
    {
        env("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("sld")
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(path) = env("XDG_STATE_HOME") {
            return Some(PathBuf::from(path).join("sld"));
        }
        env("HOME").map(|home| PathBuf::from(home).join(".local").join("state").join("sld"))
    }
}

fn state_dir_for_output(output: &Path) -> PathBuf {
    append_suffix(output, ".incr")
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn encode_path(path: &Path) -> String {
    hex::encode(path.as_os_str().as_encoded_bytes())
}

#[cfg(unix)]
fn decode_path(path: &str) -> Result<PathBuf> {
    let bytes = hex::decode(path).context("Malformed incremental path encoding")?;
    Ok(std::ffi::OsString::from_vec(bytes).into())
}

#[cfg(not(unix))]
fn decode_path(path: &str) -> Result<PathBuf> {
    let bytes = hex::decode(path).context("Malformed incremental path encoding")?;
    Ok(String::from_utf8_lossy(&bytes).into_owned().into())
}

fn encode_input_ref(input: InputRef<'_>) -> String {
    let mut bytes = input.file.filename.as_os_str().as_encoded_bytes().to_vec();
    if let Some(entry) = input.entry {
        bytes.push(0);
        bytes.extend_from_slice(entry.identifier.as_slice());
        bytes.push(0);
        bytes.extend_from_slice(entry.start_offset.to_string().as_bytes());
        bytes.push(b':');
        bytes.extend_from_slice(entry.end_offset.to_string().as_bytes());
    }
    hex::encode(bytes)
}

fn display_hex_path(path: &str) -> String {
    display_hex_text(path)
}

fn display_hex_text(text: &str) -> String {
    let bytes = hex::decode(text).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hash_text(text: &str) -> String {
    hash_bytes(text.as_bytes())
}

fn section_sidecar_file_name(contents: &str) -> String {
    format!("{SECTIONS_FILE_PREFIX}{}", hash_text(contents))
}

fn compressed_section_sidecar_file_name(contents: &[u8]) -> String {
    format!("{COMPRESSED_SECTIONS_FILE_PREFIX}{}", hash_bytes(contents))
}

fn compressed_macho_resolutions_sidecar_file_name(contents: &[u8]) -> String {
    format!(
        "{COMPRESSED_MACHO_RESOLUTIONS_FILE_PREFIX}{}",
        hash_bytes(contents)
    )
}

fn args_hash(args: &impl platform::Args) -> String {
    hash_text(&format!("{args:?}"))
}

fn link_options_hash(args: &impl platform::Args) -> String {
    hash_text(&args.incremental_link_options())
}

fn input_order_hash(file_loader: &FileLoader<'_>) -> String {
    let mut hasher = blake3::Hasher::new();
    for file in &file_loader.loaded_files {
        let path = encode_path(&file.filename);
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().to_string()
}

fn sld_version(args: &impl platform::Args) -> String {
    args.common().version.to_string()
}

fn sld_version_relink_reason<'a>(previous: Option<&'a str>, current: &str) -> Option<&'a str> {
    match previous {
        Some(previous) if previous == current => None,
        Some(_) => Some("linker version changed"),
        None => Some("linker version missing from previous state"),
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn incremental_elf_ignores_only_empty_rustc_raw_dylib_search_paths() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("rustcEmpty").join("raw-dylibs");
        let non_empty = dir.path().join("rustcUsed").join("raw-dylibs");
        let regular = dir.path().join("regular");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&non_empty).unwrap();
        std::fs::create_dir_all(&regular).unwrap();
        std::fs::write(non_empty.join("library.so"), b"used").unwrap();

        let mut elf_args = crate::args::elf::ElfArgs::default();
        elf_args.common.incremental = true;
        elf_args.lib_search_path = vec![
            empty.into_boxed_path(),
            non_empty.clone().into_boxed_path(),
            regular.clone().into_boxed_path(),
        ];
        let mut args = crate::args::Args::Elf(elf_args);

        stabilize_rustc_transient_inputs(&mut args).unwrap();

        if let crate::args::Args::Elf(elf_args) = args {
            assert_eq!(
                elf_args.lib_search_path,
                vec![non_empty.into_boxed_path(), regular.into_boxed_path()]
            );
        }
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn incremental_elf_stabilizes_rustc_transient_final_inputs_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir
            .path()
            .join("target")
            .join("debug")
            .join("deps")
            .join("uv-abc123");
        let output_dir = output.parent().unwrap();
        let symbols = output_dir.join("rustcXYZ").join("symbols.o");
        let codegen_unit = output_dir.join("uv-abc123.cgu7.session.rcgu.o");
        let archive = output_dir.join("libuv-python.rlib");
        std::fs::create_dir_all(symbols.parent().unwrap()).unwrap();
        std::fs::write(&symbols, b"symbols").unwrap();
        std::fs::write(&codegen_unit, b"codegen").unwrap();
        std::fs::write(&archive, b"archive").unwrap();

        let mut elf_args = crate::args::elf::ElfArgs::default();
        elf_args.common.incremental = true;
        elf_args.output = Arc::from(output.as_path());
        elf_args.common.inputs = [&symbols, &codegen_unit, &archive]
            .into_iter()
            .map(|path| crate::args::Input {
                spec: InputSpec::File(path.clone().into_boxed_path()),
                search_first: None,
                modifiers: crate::args::Modifiers::default(),
            })
            .collect();
        let mut args = crate::args::Args::Elf(elf_args);

        stabilize_rustc_transient_inputs(&mut args).unwrap();

        let stable_dir = state_dir_for_output(&output).join(STABLE_RUSTC_INPUT_DIR);
        let stable_symbols = stable_dir.join("rustc-symbols.o");
        let stable_codegen_unit = stable_dir.join("uv-abc123.cgu7.rcgu.o");
        if let crate::args::Args::Elf(elf_args) = args {
            assert_eq!(
                elf_args.common.inputs[0].spec,
                InputSpec::File(stable_symbols.clone().into_boxed_path())
            );
            assert_eq!(
                elf_args.common.inputs[1].spec,
                InputSpec::File(stable_codegen_unit.clone().into_boxed_path())
            );
            assert_eq!(
                elf_args.common.inputs[2].spec,
                InputSpec::File(archive.clone().into_boxed_path())
            );
        }
        assert_eq!(std::fs::read(&stable_symbols).unwrap(), b"symbols");
        assert_eq!(std::fs::read(&stable_codegen_unit).unwrap(), b"codegen");

        let stable_codegen_identity = FileIdentity::from_path(&stable_codegen_unit).unwrap();
        let mut elf_args = crate::args::elf::ElfArgs::default();
        elf_args.common.incremental = true;
        elf_args.output = Arc::from(output.as_path());
        elf_args.common.inputs = [&symbols, &codegen_unit, &archive]
            .into_iter()
            .map(|path| crate::args::Input {
                spec: InputSpec::File(path.clone().into_boxed_path()),
                search_first: None,
                modifiers: crate::args::Modifiers::default(),
            })
            .collect();
        let mut args = crate::args::Args::Elf(elf_args);

        stabilize_rustc_transient_inputs(&mut args).unwrap();

        assert_eq!(
            FileIdentity::from_path(&stable_codegen_unit).unwrap(),
            stable_codegen_identity
        );

        let next_symbols = output_dir.join("rustcNext").join("symbols.o");
        let next_codegen_unit = output_dir.join("uv-abc123.cgu7.next.rcgu.o");
        std::fs::create_dir_all(next_symbols.parent().unwrap()).unwrap();
        std::fs::write(&next_symbols, b"next-symbols").unwrap();
        std::fs::write(&next_codegen_unit, b"next-codegen").unwrap();
        let mut elf_args = crate::args::elf::ElfArgs::default();
        elf_args.common.incremental = true;
        elf_args.output = Arc::from(output.as_path());
        elf_args.common.inputs = [&next_symbols, &next_codegen_unit, &archive]
            .into_iter()
            .map(|path| crate::args::Input {
                spec: InputSpec::File(path.clone().into_boxed_path()),
                search_first: None,
                modifiers: crate::args::Modifiers::default(),
            })
            .collect();
        let mut args = crate::args::Args::Elf(elf_args);

        stabilize_rustc_transient_inputs(&mut args).unwrap();

        if let crate::args::Args::Elf(elf_args) = args {
            assert_eq!(
                elf_args.common.inputs[0].spec,
                InputSpec::File(stable_symbols.clone().into_boxed_path())
            );
            assert_eq!(
                elf_args.common.inputs[1].spec,
                InputSpec::File(stable_codegen_unit.clone().into_boxed_path())
            );
        }
        assert_eq!(std::fs::read(stable_symbols).unwrap(), b"next-symbols");
        assert_eq!(std::fs::read(stable_codegen_unit).unwrap(), b"next-codegen");
    }

    #[test]
    fn stable_rustc_input_name_is_scoped_to_final_link_inputs() {
        let output = Path::new("/target/debug/deps/uv-abc123");
        assert_eq!(
            stable_rustc_input_name(
                Path::new("/target/debug/deps/uv-abc123.cgu7.session.rcgu.o"),
                output,
            ),
            Some(PathBuf::from("uv-abc123.cgu7.rcgu.o"))
        );
        assert_eq!(
            stable_rustc_input_name(Path::new("/target/debug/deps/rustcXYZ/symbols.o"), output),
            Some(PathBuf::from("rustc-symbols.o"))
        );
        assert_eq!(
            stable_rustc_input_name(
                Path::new("/target/debug/deps/dependency-abc.cgu7.session.rcgu.o"),
                output,
            ),
            None
        );
        assert_eq!(
            stable_rustc_input_name(
                Path::new("/target/debug/deps/uv-abc123.uv.metadata-cgu.0.rcgu.o"),
                output,
            ),
            None
        );
        assert_eq!(
            stable_rustc_input_name(
                Path::new("/target/debug/other/uv-abc123.cgu7.session.rcgu.o"),
                output,
            ),
            None
        );
    }

    #[test]
    fn rustc_work_product_provenance_parser_requires_versioned_unique_digests() {
        let input = Path::new("/target/debug/deps/uv-abc123.cgu7.session.rcgu.o");
        let digest = "a".repeat(64);
        let record = format!(
            "{RUSTC_WORK_PRODUCT_PROVENANCE_VERSION}\n{digest}\t{}\n",
            encode_path(input)
        );
        assert_eq!(
            parse_rustc_work_product_provenance(&record),
            Some(HashMap::from([(input.to_path_buf(), digest.clone())]))
        );
        assert!(
            parse_rustc_work_product_provenance(&format!(
                "wrong-version\n{digest}\t{}\n",
                encode_path(input)
            ))
            .is_none()
        );
        assert!(
            parse_rustc_work_product_provenance(&format!(
                "{RUSTC_WORK_PRODUCT_PROVENANCE_VERSION}\ninvalid\t{}\n",
                encode_path(input)
            ))
            .is_none()
        );
        assert!(
            parse_rustc_work_product_provenance(&format!(
                "{record}{digest}\t{}\n",
                encode_path(input)
            ))
            .is_none()
        );
        assert_eq!(rustc_work_product_provenance(None, false), None);
        assert_eq!(
            rustc_work_product_provenance(None, true),
            Some(HashMap::new())
        );
        assert_eq!(
            rustc_work_product_provenance(Some("malformed"), true),
            Some(HashMap::new())
        );
    }

    #[cfg(unix)]
    #[test]
    fn isolated_stable_rustc_input_reuses_only_matching_prior_producer_digest() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("uv-abc123.cgu7.session.rcgu.o");
        let target = dir.path().join("stable").join("uv-abc123.cgu7.rcgu.o");
        std::fs::write(&source, b"codegen").unwrap();
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        copy_isolated_snapshot_bytes(&source, &target).unwrap();

        let source_identity = FileIdentity::from_path(&source).unwrap().unwrap();
        let target_identity = FileIdentity::from_path(&target).unwrap().unwrap();
        assert_ne!(source_identity.ino, target_identity.ino);

        let mut previous = state("args", b"output", &[("input", b"codegen")]);
        previous.input_files[0].path = encode_path(&target);
        previous.input_files[0].content = FileContentState::from_path(&target).unwrap();
        let digest = hash_bytes(b"codegen");
        let previous_inputs_by_path = previous_input_files_by_path(Some(&previous));
        assert!(stable_rustc_input_matches_previous_producer_digest(
            &previous_inputs_by_path,
            &target,
            &digest
        ));
        assert!(!stable_rustc_input_matches_previous_producer_digest(
            &previous_inputs_by_path,
            &target,
            &hash_bytes(b"different")
        ));

        let mut duplicate_previous = previous.clone();
        let mut duplicate = duplicate_previous.input_files[0].clone();
        duplicate.content.hash = hash_bytes(b"different");
        duplicate_previous.input_files.insert(0, duplicate);
        assert!(!stable_rustc_input_matches_previous_producer_digest(
            &previous_input_files_by_path(Some(&duplicate_previous)),
            &target,
            &digest
        ));

        assert!(std::fs::write(&target, b"changed").is_err());
        let replacement = target.with_extension("replacement");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(replacement, &target).unwrap();
        assert!(!stable_rustc_input_matches_previous_producer_digest(
            &previous_inputs_by_path,
            &target,
            &digest
        ));
    }

    fn state(args_hash: &str, output: &[u8], inputs: &[(&str, &[u8])]) -> PersistedState {
        PersistedState {
            args_hash: args_hash.to_owned(),
            link_options_hash: Some(args_hash.to_owned()),
            input_order_hash: Some(input_order_hash_for_paths(
                inputs.iter().map(|(path, _)| *path),
            )),
            sld_version: Some("sld-test".to_owned()),
            link_start: None,
            output: FileContentState::from_bytes(output),
            build_id_hashes: None,
            input_files: inputs
                .iter()
                .map(|(path, bytes)| FileState {
                    path: hex::encode(path),
                    content: FileContentState::from_bytes(bytes),
                    snapshot_identity: None,
                    patch: None,
                })
                .collect(),
            sections: Vec::new(),
            relocations: Vec::new(),
            macho_symbol_resolutions: Vec::new(),
            fdes: Vec::new(),
            dynamic_relocations: Vec::new(),
            sections_file: None,
            macho_symbol_resolutions_file: None,
            patch_records_file: None,
            patch_record_locations: Vec::new(),
            raw_patch_record_locations: None,
        }
    }

    fn input_order_hash_for_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> String {
        let mut hasher = blake3::Hasher::new();
        for path in paths {
            hasher.update(hex::encode(path).as_bytes());
            hasher.update(&[0]);
        }
        hasher.finalize().to_hex().to_string()
    }

    #[test]
    fn output_symbol_value_patches_match_duplicate_names_by_previous_value() {
        let (output, first_value_range, second_value_range) = duplicate_symbol_name_elf();

        let patches = output_symbol_value_patches(
            &output,
            &[RelocationTargetSymbolPatch {
                target_name: hex::encode(b"duplicate"),
                previous_target_value: 0x200,
                target_value: 0x208,
                allow_missing: false,
            }],
        )
        .unwrap()
        .unwrap();

        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].output_offset, second_value_range.start as u64);
        assert_eq!(patches[0].data, 0x208_u64.to_le_bytes());
        assert_eq!(&output[first_value_range], &0x100_u64.to_le_bytes());
        assert_eq!(&output[second_value_range], &0x200_u64.to_le_bytes());
    }

    #[test]
    fn output_symbol_value_patches_reject_missing_output_symbols() {
        let (output, _, _) = duplicate_symbol_name_elf();

        let patches = output_symbol_value_patches(
            &output,
            &[RelocationTargetSymbolPatch {
                target_name: hex::encode(b"missing"),
                previous_target_value: 0x200,
                target_value: 0x208,
                allow_missing: false,
            }],
        )
        .unwrap();

        assert!(matches!(
            patches,
            Err(reason) if reason == "missing output symbol for incremental value patch"
        ));
    }

    #[test]
    fn output_symbol_value_patches_reject_unexpected_existing_value_when_missing_is_allowed() {
        let output = test_macho_object(b"\x01\x02\x03\x04", b"\x05\x06\x07\x08", 0);

        let patches = output_symbol_value_patches(
            &output,
            &[RelocationTargetSymbolPatch {
                target_name: hex::encode(b"_data"),
                previous_target_value: 8,
                target_value: 12,
                allow_missing: true,
            }],
        )
        .unwrap();

        assert!(matches!(
            patches,
            Err(reason) if reason == "output symbol value changed before incremental patch"
        ));
    }

    #[test]
    fn output_symbol_value_patches_skip_absent_symbol_when_missing_is_allowed() {
        let output = test_macho_object(b"\x01\x02\x03\x04", b"\x05\x06\x07\x08", 0);

        let patches = output_symbol_value_patches(
            &output,
            &[RelocationTargetSymbolPatch {
                target_name: hex::encode(b"_stripped"),
                previous_target_value: 8,
                target_value: 12,
                allow_missing: true,
            }],
        )
        .unwrap()
        .unwrap();

        assert!(patches.is_empty());
    }

    #[test]
    fn relocation_target_patch_supports_32_bit_output_payloads() {
        let (previous, first_value_range, _) = duplicate_symbol_name_elf();
        let mut current = previous.clone();
        current[first_value_range.clone()].copy_from_slice(&0x108_u64.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &previous)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some(0x1000),
            0x2000,
            Some("duplicate"),
            Some(("input.o", 1, 0x100)),
            0,
            300,
            4,
            object::elf::R_X86_64_32,
            0,
        );
        let mut relocations = vec![relocation];

        let patches = relocation_target_patches_for_input(&mut relocations, &input, &current)
            .unwrap()
            .unwrap();

        assert_eq!(patches.input_ranges, vec![first_value_range]);
        assert_eq!(patches.output_patches.len(), 1);
        assert_eq!(patches.output_patches[0].output_offset, 300);
        assert_eq!(patches.output_patches[0].size, 4);
        assert_eq!(
            patches.output_patches[0].data,
            0x1008_u32.to_le_bytes().to_vec()
        );
        assert_eq!(relocations[0].written_value, Some(0x1008));
        assert_eq!(relocations[0].target_value, 0x2008);
        assert_eq!(
            relocations[0]
                .target
                .as_ref()
                .map(|target| target.section_offset),
            Some(0x108)
        );
    }

    #[test]
    fn relocation_target_patch_encodes_negative_x86_64_pc32_payloads() {
        let (previous, first_value_range, _) = duplicate_symbol_name_elf();
        let mut current = previous.clone();
        current[first_value_range.clone()].copy_from_slice(&0xf8_u64.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &previous)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some((-8_i64) as u64),
            0x2000,
            Some("duplicate"),
            Some(("input.o", 1, 0x100)),
            0,
            300,
            4,
            object::elf::R_X86_64_PC32,
            0,
        );
        let mut relocations = vec![relocation];

        let patches = relocation_target_patches_for_input(&mut relocations, &input, &current)
            .unwrap()
            .unwrap();

        assert_eq!(
            patches.output_patches[0].data,
            (-16_i32).to_le_bytes().to_vec()
        );
        assert_eq!(relocations[0].written_value, Some((-16_i64) as u64));
        assert_eq!(relocations[0].target_value, 0x1ff8);
    }

    #[test]
    fn relocation_target_patch_encodes_x86_64_pc32_crossing_zero() {
        let (previous, first_value_range, _) = duplicate_symbol_name_elf();
        let mut current = previous.clone();
        current[first_value_range.clone()].copy_from_slice(&0x110_u64.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &previous)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some((-8_i64) as u64),
            0x2000,
            Some("duplicate"),
            Some(("input.o", 1, 0x100)),
            0,
            300,
            4,
            object::elf::R_X86_64_PC32,
            0,
        );
        let mut relocations = vec![relocation];

        let patches = relocation_target_patches_for_input(&mut relocations, &input, &current)
            .unwrap()
            .unwrap();

        assert_eq!(patches.output_patches[0].data, 8_i32.to_le_bytes().to_vec());
        assert_eq!(relocations[0].written_value, Some(8));
        assert_eq!(relocations[0].target_value, 0x2010);
    }

    #[test]
    fn relocation_target_patch_defers_riscv_instruction_payloads() {
        let (mut previous, first_value_range, _) = duplicate_symbol_name_elf();
        previous[18..20].copy_from_slice(&object::elf::EM_RISCV.to_le_bytes());
        let mut current = previous.clone();
        current[first_value_range.clone()].copy_from_slice(&0x108_u64.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &previous)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some(0x1000),
            0x2000,
            Some("duplicate"),
            Some(("input.o", 1, 0x100)),
            0,
            300,
            4,
            object::elf::R_RISCV_JAL,
            0,
        );
        let mut relocations = vec![relocation];

        let patches = relocation_target_patches_for_input(&mut relocations, &input, &current)
            .unwrap()
            .unwrap();

        assert_eq!(patches.output_patches.len(), 1);
        assert!(patches.output_patches[0].deferred_relocation.is_some());
        assert_eq!(patches.output_patches[0].data, vec![0; 4]);
        assert_eq!(relocations[0].written_value, Some(0x1008));
    }

    #[test]
    fn relocation_target_patch_defers_aarch64_instruction_payloads() {
        let (mut previous, first_value_range, _) = duplicate_symbol_name_elf();
        previous[18..20].copy_from_slice(&object::elf::EM_AARCH64.to_le_bytes());
        let mut current = previous.clone();
        current[first_value_range.clone()].copy_from_slice(&0x108_u64.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &previous)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some(0x1000),
            0x2000,
            Some("duplicate"),
            Some(("input.o", 1, 0x100)),
            0,
            300,
            4,
            object::elf::R_AARCH64_JUMP26,
            0,
        );
        let mut relocations = vec![relocation];

        let patches = relocation_target_patches_for_input(&mut relocations, &input, &current)
            .unwrap()
            .unwrap();

        assert_eq!(patches.output_patches.len(), 1);
        assert!(patches.output_patches[0].deferred_relocation.is_some());
        assert_eq!(patches.output_patches[0].data, vec![0; 4]);
        assert_eq!(relocations[0].written_value, Some(0x1008));
    }

    #[test]
    fn deferred_riscv_instruction_patches_preserve_non_relocation_bits() {
        let rel_info = riscv64::relocation_type_from_raw(object::elf::R_RISCV_JAL).unwrap();
        let previous_output = 0x0000_006f_u32.to_le_bytes();
        let mut data = vec![0; previous_output.len()];
        let mut expected = previous_output.to_vec();
        rel_info.write_to_buffer(8, &mut expected).unwrap();

        materialize_deferred_relocation_patch(
            &mut data,
            &previous_output,
            DeferredRelocationPatch {
                rel_info,
                previous_written_value: 0,
                written_value: 8,
            },
        )
        .unwrap();

        assert_eq!(data, expected);
        assert_eq!(data[0] & 0x7f, 0x6f);
    }

    #[test]
    fn deferred_riscv_call_patches_cover_both_instruction_words() {
        let rel_info = riscv64::relocation_type_from_raw(object::elf::R_RISCV_CALL_PLT).unwrap();
        let previous_output = [0x97, 0x00, 0x00, 0x00, 0xe7, 0x80, 0x00, 0x00];
        let mut data = vec![0; previous_output.len()];
        let mut expected = previous_output.to_vec();
        rel_info.write_to_buffer(8, &mut expected).unwrap();

        materialize_deferred_relocation_patch(
            &mut data,
            &previous_output,
            DeferredRelocationPatch {
                rel_info,
                previous_written_value: 0,
                written_value: 8,
            },
        )
        .unwrap();

        assert_eq!(data, expected);
    }

    #[test]
    fn deferred_riscv_call_patches_reject_relaxed_output_windows() {
        let rel_info = riscv64::relocation_type_from_raw(object::elf::R_RISCV_CALL_PLT).unwrap();
        let previous_output = 0x0000_006f_u32.to_le_bytes();
        let mut data = vec![0; previous_output.len()];

        let result = materialize_deferred_relocation_patch(
            &mut data,
            &previous_output,
            DeferredRelocationPatch {
                rel_info,
                previous_written_value: 0,
                written_value: 8,
            },
        );

        assert!(matches!(
            result,
            Err(reason) if reason == "deferred relocation patch output size changed"
        ));
    }

    #[test]
    fn relocation_target_patch_rejects_same_offset_section_moves() {
        let (previous, _, _) = duplicate_symbol_name_elf();
        let mut current = previous.clone();
        current[0x7e..0x80].copy_from_slice(&2_u16.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &previous)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some(0x1000),
            0x2000,
            Some("duplicate"),
            Some(("input.o", 1, 0x100)),
            0,
            300,
            8,
            1,
            0,
        );
        let mut relocations = vec![relocation];

        let patches =
            relocation_target_patches_for_input(&mut relocations, &input, &current).unwrap();

        assert!(matches!(
            patches,
            Err(reason) if reason
                == "relocation target `duplicate` moved from section 1 offset 0x100 to section 2 offset 0x100 in input.o"
        ));
    }

    fn duplicate_symbol_name_elf() -> (Vec<u8>, std::ops::Range<usize>, std::ops::Range<usize>) {
        let mut bytes = vec![0; 0x220];
        let shstrtab = b"\0.text\0.symtab\0.strtab\0.shstrtab\0";
        let strtab = b"\0duplicate\0";
        let text_offset = 0x40;
        let symtab_offset = 0x60;
        let strtab_offset = 0xa8;
        let shstrtab_offset = 0xb8;
        let section_headers_offset = 0xe0;
        bytes[strtab_offset..strtab_offset + strtab.len()].copy_from_slice(strtab);
        bytes[shstrtab_offset..shstrtab_offset + shstrtab.len()].copy_from_slice(shstrtab);

        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&object::elf::ET_REL.to_le_bytes());
        bytes[18..20].copy_from_slice(&object::elf::EM_X86_64.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&(section_headers_offset as u64).to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&5_u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&4_u16.to_le_bytes());

        let write_symbol = |bytes: &mut [u8], index: usize, value: u64| {
            let symbol_offset = symtab_offset + index * 24;
            bytes[symbol_offset..symbol_offset + 4].copy_from_slice(&1_u32.to_le_bytes());
            bytes[symbol_offset + 4] = object::elf::STT_OBJECT;
            bytes[symbol_offset + 6..symbol_offset + 8].copy_from_slice(&1_u16.to_le_bytes());
            bytes[symbol_offset + 8..symbol_offset + 16].copy_from_slice(&value.to_le_bytes());
        };
        write_symbol(&mut bytes, 1, 0x100);
        write_symbol(&mut bytes, 2, 0x200);

        let name_offset = |name: &[u8]| -> u32 {
            shstrtab
                .windows(name.len())
                .position(|window| window == name)
                .unwrap() as u32
        };
        let write_section = |bytes: &mut [u8],
                             index: usize,
                             name: &[u8],
                             ty: u32,
                             flags: u64,
                             offset: u64,
                             size: u64,
                             link: u32,
                             info: u32,
                             align: u64,
                             entsize: u64| {
            let header = section_headers_offset + index * 64;
            bytes[header..header + 4].copy_from_slice(&name_offset(name).to_le_bytes());
            bytes[header + 4..header + 8].copy_from_slice(&ty.to_le_bytes());
            bytes[header + 8..header + 16].copy_from_slice(&flags.to_le_bytes());
            bytes[header + 24..header + 32].copy_from_slice(&offset.to_le_bytes());
            bytes[header + 32..header + 40].copy_from_slice(&size.to_le_bytes());
            bytes[header + 40..header + 44].copy_from_slice(&link.to_le_bytes());
            bytes[header + 44..header + 48].copy_from_slice(&info.to_le_bytes());
            bytes[header + 48..header + 56].copy_from_slice(&align.to_le_bytes());
            bytes[header + 56..header + 64].copy_from_slice(&entsize.to_le_bytes());
        };
        write_section(
            &mut bytes,
            1,
            b".text",
            object::elf::SHT_PROGBITS,
            u64::from(object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR),
            text_offset as u64,
            0x20,
            0,
            0,
            16,
            0,
        );
        write_section(
            &mut bytes,
            2,
            b".symtab",
            object::elf::SHT_SYMTAB,
            0,
            symtab_offset as u64,
            72,
            3,
            3,
            8,
            24,
        );
        write_section(
            &mut bytes,
            3,
            b".strtab",
            object::elf::SHT_STRTAB,
            0,
            strtab_offset as u64,
            strtab.len() as u64,
            0,
            0,
            1,
            0,
        );
        write_section(
            &mut bytes,
            4,
            b".shstrtab",
            object::elf::SHT_STRTAB,
            0,
            shstrtab_offset as u64,
            shstrtab.len() as u64,
            0,
            0,
            1,
            0,
        );

        let first_value_range = symtab_offset + 24 + 8..symtab_offset + 24 + 16;
        let second_value_range = symtab_offset + 48 + 8..symtab_offset + 48 + 16;
        (bytes, first_value_range, second_value_range)
    }

    fn section_record(
        input: &str,
        section_index: u32,
        output_offset: u64,
        size: u64,
    ) -> SectionRecord {
        SectionRecord {
            input_file: hex::encode(input).into(),
            input: hex::encode(input).into(),
            section_index,
            output_offset,
            size,
        }
    }

    fn fde_record(
        input: &str,
        section_index: u32,
        eh_frame_section_index: u32,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> FdeRecord {
        FdeRecord {
            input_file: hex::encode(input).into(),
            input: hex::encode(input).into(),
            section_index,
            eh_frame_section_index,
            input_offset,
            output_offset,
            size,
        }
    }

    fn dynamic_relocation_record(
        input: &str,
        section_index: u32,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> DynamicRelocationRecord {
        DynamicRelocationRecord {
            input_file: hex::encode(input).into(),
            input: hex::encode(input).into(),
            section_index,
            relocation_offset,
            output_offset,
            size,
            output_r_offset: None,
            output_r_info: None,
        }
    }

    fn dynamic_relocation_record_with_output_info(
        input: &str,
        section_index: u32,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        output_r_offset: u64,
        output_r_info: u64,
    ) -> DynamicRelocationRecord {
        let mut record =
            dynamic_relocation_record(input, section_index, relocation_offset, output_offset, size);
        record.output_r_offset = Some(output_r_offset);
        record.output_r_info = Some(output_r_info);
        record
    }

    fn relocation_record(
        input: &str,
        section_index: u32,
        target_symbol_id: u32,
        written_value: Option<u64>,
        target_value: u64,
        target_name: Option<&str>,
        target: Option<(&str, u32, u64)>,
        relocation_offset: u64,
        output_offset: u64,
        size: u64,
        kind: u32,
        addend: i64,
    ) -> RelocationRecord {
        RelocationRecord {
            target_symbol_id,
            written_value,
            target_value,
            applied_target_value: None,
            target_name: target_name.map(|name| hex::encode(name).into()),
            target: target.map(
                |(input, section_index, section_offset)| RelocationTargetRecord {
                    input_file: hex::encode(input).into(),
                    input: hex::encode(input).into(),
                    section_index,
                    section_offset,
                },
            ),
            input_file: hex::encode(input).into(),
            input: hex::encode(input).into(),
            section_index,
            relocation_offset,
            output_offset,
            size,
            kind,
            addend,
        }
    }

    fn current_relocation_as_v25_line(line: &str) -> String {
        let Some(rest) = line.strip_prefix("reloc3\t") else {
            return line.to_owned();
        };
        let fields = rest.split('\t').collect::<Vec<_>>();
        format!(
            "reloc\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            fields[0],
            fields[1],
            fields[2],
            fields[3],
            fields[4],
            fields[5],
            fields[6],
            fields[7],
            fields[9],
            fields[11],
            hex::encode("a.o"),
            hex::encode("a.o"),
            fields[13],
            fields[14],
        )
    }

    fn current_relocation_as_v37_line(line: &str) -> String {
        let Some(rest) = line.strip_prefix("reloc3\t") else {
            return line.to_owned();
        };
        let fields = rest.split('\t').collect::<Vec<_>>();
        let mut fields_without_applied_target = Vec::with_capacity(fields.len() - 1);
        fields_without_applied_target.extend_from_slice(&fields[..10]);
        fields_without_applied_target.extend_from_slice(&fields[11..]);
        format!("reloc2\t{}", fields_without_applied_target.join("\t"))
    }

    fn current_relocation_as_v24_line(line: &str) -> String {
        let Some(rest) = line.strip_prefix("reloc3\t") else {
            return line.to_owned();
        };
        let fields = rest.split('\t').collect::<Vec<_>>();
        format!(
            "reloc\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            fields[0],
            fields[1],
            fields[2],
            fields[3],
            fields[4],
            fields[5],
            fields[6],
            fields[7],
            fields[9],
            fields[11],
        )
    }

    fn current_relocation_as_v23_line(line: &str) -> String {
        let Some(rest) = line.strip_prefix("reloc3\t") else {
            return line.to_owned();
        };
        let fields = rest.split('\t').take(8).collect::<Vec<_>>().join("\t");
        format!("reloc\t{fields}")
    }

    fn section_reference(source_section_name: &str, relocation_offset: u64) -> SectionReference {
        SectionReference {
            source_section_name: source_section_name.to_owned(),
            relocation_offset,
            relocation_kind: "Absolute".to_owned(),
            relocation_encoding: "Generic".to_owned(),
            relocation_size: 64,
            relocation_addend: 0,
        }
    }

    fn render_legacy_state(state: &PersistedState, version: &str) -> String {
        let mut out = String::new();
        writeln!(&mut out, "{version}").unwrap();
        writeln!(&mut out, "args\t{}", state.args_hash).unwrap();
        writeln!(
            &mut out,
            "output\t{}\t{}\t{}",
            state.output.len,
            state.output.hash,
            state.output.render_identity()
        )
        .unwrap();
        writeln!(&mut out, "inputs\t{}", state.input_files.len()).unwrap();
        for input in &state.input_files {
            writeln!(
                &mut out,
                "input\t{}\t{}\t{}\t{}",
                input.path,
                input.content.len,
                input.content.hash,
                input.content.render_identity()
            )
            .unwrap();
        }
        writeln!(&mut out, "sections\t{}", state.sections.len()).unwrap();
        for section in &state.sections {
            writeln!(
                &mut out,
                "section\t{}\t{}\t{}\t{}\t{}",
                section.input_file,
                section.input,
                section.section_index,
                section.output_offset,
                section.size
            )
            .unwrap();
        }
        out
    }

    fn render_v8_state(state: &PersistedState) -> String {
        state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V8, 1)
            .lines()
            .filter(|line| {
                !line.starts_with("build-id-hash\t") && !line.starts_with("sld-version\t")
            })
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            })
    }

    fn identity(len: u64, dev: u64, ino: u64, modified_sec: i64, changed_sec: i64) -> FileIdentity {
        FileIdentity {
            len,
            dev,
            ino,
            modified_sec,
            modified_nsec: 0,
            changed_sec,
            changed_nsec: 0,
        }
    }

    fn content_hash_with_path_identity(path: &Path, bytes: &[u8]) -> FileContentState {
        let mut content = FileContentState::from_bytes(bytes);
        content.identity = FileIdentity::from_path(path).unwrap();
        content
    }

    #[test]
    fn state_dir_appends_suffix() {
        assert_eq!(
            state_dir_for_output(Path::new("target/debug/app")),
            Path::new("target/debug/app.incr")
        );
        assert_eq!(
            state_dir_for_output(Path::new("target/debug/app.so")),
            Path::new("target/debug/app.so.incr")
        );
    }

    #[test]
    fn user_state_dir_uses_override() {
        let dir = user_state_dir_from_env(|name| {
            (name == USER_STATE_DIR_ENV).then(|| OsString::from("/tmp/sld-state"))
        });

        assert_eq!(dir, Some(PathBuf::from("/tmp/sld-state")));
    }

    #[test]
    fn user_state_dir_uses_platform_default() {
        let dir = user_state_dir_from_env(|name| match name {
            "HOME" => Some(OsString::from("/home/sld")),
            _ => None,
        })
        .unwrap();

        #[cfg(target_os = "macos")]
        assert_eq!(
            dir,
            PathBuf::from("/home/sld")
                .join("Library")
                .join("Application Support")
                .join("sld")
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            dir,
            PathBuf::from("/home/sld")
                .join(".local")
                .join("state")
                .join("sld")
        );
    }

    #[test]
    fn user_state_dir_prefers_xdg_state_home_on_non_macos() {
        let dir = user_state_dir_from_env(|name| match name {
            "HOME" => Some(OsString::from("/home/sld")),
            "XDG_STATE_HOME" => Some(OsString::from("/state")),
            _ => None,
        })
        .unwrap();

        #[cfg(target_os = "macos")]
        assert_eq!(
            dir,
            PathBuf::from("/home/sld")
                .join("Library")
                .join("Application Support")
                .join("sld")
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(dir, PathBuf::from("/state").join("sld"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn global_log_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        append_global_log_to(
            dir.path(),
            Path::new("target/debug/app.incr"),
            "full relink: no previous incremental state",
        )
        .unwrap();

        let mut out = Vec::new();
        print_global_log_from(dir.path(), &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();

        assert!(
            out.contains("\ttarget/debug/app.incr\tfull relink: no previous incremental state\n")
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn incremental_link_options_can_be_logged_for_diagnosis() {
        let dir = tempfile::tempdir().unwrap();
        let args = crate::args::elf::ElfArgs::default();
        log_incremental_link_options_if_requested(&args, dir.path(), true, true).unwrap();

        let log = std::fs::read_to_string(dir.path().join(LOG_FILE)).unwrap();
        assert_eq!(
            log,
            format!(
                "incremental link options: {}\nincremental exact args: {args:?}\n",
                platform::Args::incremental_link_options(&args),
            )
        );
    }

    #[test]
    fn input_snapshot_path_is_stable_for_input_path() {
        let state_dir = Path::new("target/debug/app.incr");
        assert_eq!(
            input_snapshot_path(state_dir, Path::new("obj/main.o")),
            input_snapshot_path(state_dir, Path::new("obj/main.o"))
        );
        assert_ne!(
            input_snapshot_path(state_dir, Path::new("obj/main.o")),
            input_snapshot_path(state_dir, Path::new("obj/other.o"))
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn input_snapshots_are_isolated_copies() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, b"object").unwrap();
        let mut input_files = vec![FileState {
            path: encode_path(&input),
            content: FileContentState::from_path_identity_only(&input).unwrap(),
            snapshot_identity: None,
            patch: None,
        }];

        assert_eq!(
            snapshot_input_paths(&state_dir, [input.as_path()]).unwrap(),
            1
        );
        refresh_input_file_identities(&mut input_files);

        let snapshot = input_snapshot_path(&state_dir, &input);
        assert_eq!(std::fs::read(&snapshot).unwrap(), b"object");
        assert!(
            input_files[0]
                .content
                .identity_matches_path(&input)
                .unwrap()
        );

        std::fs::write(&input, b"changed").unwrap();
        assert_eq!(std::fs::read(&snapshot).unwrap(), b"object");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn directly_patched_output_generation_installs_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("app");
        let state_dir = dir.path().join("app.incr");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::write(&output, b"original").unwrap();
        let original_identity = FileIdentity::from_path(&output).unwrap().unwrap();

        let in_place = DirectlyPatchedOutput::new(&output, &state_dir, false).unwrap();
        assert_eq!(in_place.path(), output);
        assert!(!in_place.is_generation());
        assert!(in_place.should_invalidate_code_signature_cache());
        drop(in_place);

        let aborted = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
        let aborted_path = aborted.path().to_path_buf();
        assert_ne!(aborted.path(), output);
        assert!(aborted.is_generation());
        assert!(!aborted.should_invalidate_code_signature_cache());
        std::fs::write(aborted.path(), b"aborted").unwrap();
        drop(aborted);
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        assert!(!aborted_path.exists());

        let mut installed = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
        let installed_path = installed.path().to_path_buf();
        std::fs::write(installed.path(), b"changed!").unwrap();
        let installed_identity = FileIdentity::from_path(&installed_path).unwrap().unwrap();
        installed.install(&[0..8]).unwrap();
        assert!(!installed.is_generation());
        drop(installed);

        assert_eq!(std::fs::read(&output).unwrap(), b"changed!");
        let output_identity = FileIdentity::from_path(&output).unwrap().unwrap();
        assert_eq!(output_identity.ino, installed_identity.ino);
        assert_ne!(output_identity.ino, original_identity.ino);
        #[cfg(target_vendor = "apple")]
        {
            assert_eq!(std::fs::read(&installed_path).unwrap(), b"original");
            assert_eq!(
                read_directly_patched_output_standby_ranges(
                    &output,
                    &installed_path,
                    &directly_patched_output_standby_state_path(&state_dir),
                )
                .unwrap(),
                vec![0..8],
            );

            let mut reused = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
            assert_eq!(reused.path(), installed_path);
            assert_eq!(std::fs::read(reused.path()).unwrap(), b"changed!");
            std::fs::write(reused.path(), b"second!!").unwrap();
            reused.install(&[0..8]).unwrap();
            drop(reused);

            assert_eq!(std::fs::read(&output).unwrap(), b"second!!");
            assert_eq!(std::fs::read(&installed_path).unwrap(), b"changed!");
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            assert!(!installed_path.exists());
            assert!(!directly_patched_output_standby_state_path(&state_dir).exists());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn directly_patched_output_generation_discards_replaced_predecessor() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("app");
        let state_dir = dir.path().join("app.incr");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::write(&output, b"abcdefgh").unwrap();

        let mut first = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
        std::fs::write(first.path(), b"Abcdefgh").unwrap();
        first.install(&[0..1]).unwrap();
        drop(first);

        let mut raced = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
        assert_eq!(std::fs::read(raced.path()).unwrap(), b"Abcdefgh");
        std::fs::write(raced.path(), b"ABcdefgh").unwrap();
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"XXXXXXXX").unwrap();
        std::fs::rename(&replacement, &output).unwrap();
        raced.install(&[1..2]).unwrap();
        drop(raced);

        assert_eq!(std::fs::read(&output).unwrap(), b"ABcdefgh");
        assert!(!directly_patched_output_standby_state_path(&state_dir).exists());
        let recloned = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
        assert_eq!(std::fs::read(recloned.path()).unwrap(), b"ABcdefgh");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn directly_patched_output_generation_rejects_truncated_standby_state() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("app");
        let state_dir = dir.path().join("app.incr");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::write(&output, b"original").unwrap();

        let mut installed = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
        std::fs::write(installed.path(), b"changed!").unwrap();
        installed.install(&[0..8]).unwrap();
        drop(installed);

        let state_path = directly_patched_output_standby_state_path(&state_dir);
        let truncated = std::fs::read_to_string(&state_path)
            .unwrap()
            .lines()
            .take(4)
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&state_path, format!("{truncated}\n")).unwrap();

        let fallback = DirectlyPatchedOutput::new(&output, &state_dir, true).unwrap();
        assert_eq!(std::fs::read(fallback.path()).unwrap(), b"changed!");
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn directly_patched_output_generation_discards_standby_above_range_cap() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join(DIRECT_PATCH_STANDBY_STATE_FILE);
        let standby = dir.path().join(DIRECT_PATCH_STANDBY_FILE);
        std::fs::write(&state_path, b"state").unwrap();
        std::fs::write(&standby, b"standby").unwrap();
        let ranges = (0..=DIRECT_PATCH_STANDBY_MAX_RANGES)
            .map(|index| index * 2..index * 2 + 1)
            .collect::<Vec<_>>();

        write_directly_patched_output_standby_state(
            &state_path,
            Path::new("unused-output"),
            &standby,
            &ranges,
        )
        .unwrap();

        assert!(!state_path.exists());
        assert!(!standby.exists());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn begin_update_invalidates_directly_patched_output_standby_only_for_relinks() {
        fn prepared_state(mode: IncrementalMode, state_dir: PathBuf) -> PreparedState<'static> {
            PreparedState {
                mode,
                current: CurrentState {
                    state_dir,
                    args_hash: String::new(),
                    link_options_hash: String::new(),
                    input_order_hash: String::new(),
                    sld_version: String::new(),
                    link_start: None,
                    input_files: Vec::new(),
                },
                retained_output_to_restore: None,
                reusable_inputs: HashSet::new(),
                previous_sections: HashSet::new(),
                previous_relocations: Vec::new(),
                previous_macho_symbol_resolutions_file: None,
                previous_fdes: Vec::new(),
                previous_dynamic_relocations: Vec::new(),
                current_sections: RecordBuffers::default(),
                current_relocations: RecordBuffers::default(),
                current_macho_symbol_resolutions: RecordBuffers::default(),
                current_fdes: RecordBuffers::default(),
                current_dynamic_relocations: RecordBuffers::default(),
                record_texts: RecordTextInterner::default(),
                reused_sections: AtomicUsize::new(0),
                prepared_fast_build_id_state: Mutex::new(None),
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let reused_state_dir = dir.path().join("reused.incr");
        std::fs::create_dir(&reused_state_dir).unwrap();
        std::fs::write(
            directly_patched_output_standby_path(&reused_state_dir),
            b"output",
        )
        .unwrap();
        std::fs::write(
            directly_patched_output_standby_state_path(&reused_state_dir),
            b"state",
        )
        .unwrap();
        let reused = prepared_state(IncrementalMode::Reuse, reused_state_dir.clone());
        drop(reused.begin_update().unwrap());
        assert!(directly_patched_output_standby_path(&reused_state_dir).exists());
        assert!(directly_patched_output_standby_state_path(&reused_state_dir).exists());

        let relinked_state_dir = dir.path().join("relinked.incr");
        std::fs::create_dir(&relinked_state_dir).unwrap();
        std::fs::write(
            directly_patched_output_standby_path(&relinked_state_dir),
            b"output",
        )
        .unwrap();
        std::fs::write(
            directly_patched_output_standby_state_path(&relinked_state_dir),
            b"state",
        )
        .unwrap();
        let relinked = prepared_state(
            IncrementalMode::Relink {
                reason: "input file changed".to_owned(),
                can_reuse_unchanged_sections: true,
            },
            relinked_state_dir.clone(),
        );
        drop(relinked.begin_update().unwrap());
        assert!(!directly_patched_output_standby_path(&relinked_state_dir).exists());
        assert!(!directly_patched_output_standby_state_path(&relinked_state_dir).exists());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn input_snapshots_deduplicate_paths() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, b"object").unwrap();

        assert_eq!(
            snapshot_input_paths(&state_dir, [input.as_path(), input.as_path()]).unwrap(),
            1
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_input_snapshots_skip_unchanged_rustc_rlibs() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let unchanged = dir.path().join("libunchanged.rlib");
        let changed = dir.path().join("libchanged.rlib");
        std::fs::write(&unchanged, b"old unchanged").unwrap();
        std::fs::write(&changed, b"old changed").unwrap();
        snapshot_input_paths(&state_dir, [unchanged.as_path(), changed.as_path()]).unwrap();

        let replacement = dir.path().join("replacement.rlib");
        std::fs::write(&replacement, b"new unchanged").unwrap();
        std::fs::rename(&replacement, &unchanged).unwrap();
        let replacement = dir.path().join("replacement.rlib");
        std::fs::write(&replacement, b"new changed").unwrap();
        std::fs::rename(&replacement, &changed).unwrap();

        let changed_inputs = vec![(0, unchanged.clone()), (1, changed.clone())];
        let unchanged_rustc_rlibs = HashSet::from([0]);
        assert_eq!(
            snapshot_input_paths(
                &state_dir,
                changed_inputs_requiring_snapshot(&changed_inputs, &unchanged_rustc_rlibs)
                    .map(|(_, path)| path.as_path()),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            std::fs::read(input_snapshot_path(&state_dir, &unchanged)).unwrap(),
            b"old unchanged"
        );
        assert_eq!(
            std::fs::read(input_snapshot_path(&state_dir, &changed)).unwrap(),
            b"new changed"
        );

        let mut input_files = vec![
            FileState {
                path: encode_path(&unchanged),
                content: FileContentState::from_path_identity_only(&unchanged).unwrap(),
                snapshot_identity: None,
                patch: None,
            },
            FileState {
                path: encode_path(&changed),
                content: FileContentState::from_path_identity_only(&changed).unwrap(),
                snapshot_identity: None,
                patch: None,
            },
        ];
        refresh_input_snapshot_identities_at_indices(
            &state_dir,
            &mut input_files,
            changed_inputs_requiring_snapshot(&changed_inputs, &unchanged_rustc_rlibs)
                .map(|(input_index, _)| *input_index),
        );
        assert!(input_files[0].snapshot_identity.is_none());
        assert_eq!(
            input_files[1].snapshot_identity,
            FileIdentity::from_path(&input_snapshot_path(&state_dir, &changed)).unwrap()
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_input_snapshots_accept_readonly_atomic_rust_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let stable_dir = state_dir.join(STABLE_RUSTC_INPUT_DIR);
        std::fs::create_dir_all(&stable_dir).unwrap();
        let input = stable_dir.join("crate.0123456789abcdef.rcgu.o");
        std::fs::write(&input, b"changed").unwrap();
        let mut permissions = std::fs::metadata(&input).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&input, permissions).unwrap();

        assert_eq!(
            snapshot_input_paths(&state_dir, [input.as_path()]).unwrap(),
            1
        );
        assert_eq!(
            std::fs::read(input_snapshot_path(&state_dir, &input)).unwrap(),
            b"changed"
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_input_snapshots_skip_readonly_atomic_rust_inputs_outside_stable_dir() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("crate.0123456789abcdef.rcgu.o");
        std::fs::write(&input, b"changed").unwrap();
        let mut permissions = std::fs::metadata(&input).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&input, permissions).unwrap();

        assert_eq!(
            snapshot_input_paths(&state_dir, [input.as_path()]).unwrap(),
            0
        );
        assert!(!input_snapshot_path(&state_dir, &input).exists());
    }

    #[test]
    fn rust_snapshot_hardlinks_only_atomic_replacement_artifacts() {
        assert!(is_atomic_replacement_rust_input(Path::new("libcrate.rlib")));
        assert!(is_atomic_replacement_rust_input(Path::new(
            "crate.0123456789abcdef.rcgu.o"
        )));
        assert!(!is_atomic_replacement_rust_input(Path::new("member.o")));
    }

    #[test]
    fn hashless_atomic_replacement_snapshots_require_matched_trusted_inputs() {
        assert!(can_retain_hashless_atomic_replacement_snapshot(
            Path::new("libcrate.rlib"),
            true,
            true,
        ));
        assert!(!can_retain_hashless_atomic_replacement_snapshot(
            Path::new("libcrate.rlib"),
            false,
            true,
        ));
        assert!(!can_retain_hashless_atomic_replacement_snapshot(
            Path::new("libcrate.rlib"),
            true,
            false,
        ));
        assert!(!can_retain_hashless_atomic_replacement_snapshot(
            Path::new("libcrate.a"),
            true,
            true,
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn rust_hardlink_snapshot_survives_atomic_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("crate.0123456789abcdef.rcgu.o");
        let snapshot = dir.path().join("snapshot");
        std::fs::write(&input, b"object").unwrap();

        assert!(hardlink_rust_snapshot_bytes(&input, &snapshot));
        let replacement = dir.path().join("replacement.o");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(&replacement, &input).unwrap();

        assert_eq!(std::fs::read(&snapshot).unwrap(), b"object");
        assert_eq!(std::fs::read(&input).unwrap(), b"changed");
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn deferred_hashing_uses_bytes_from_installed_rust_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let stable_dir = state_dir.join(STABLE_RUSTC_INPUT_DIR);
        std::fs::create_dir_all(&stable_dir).unwrap();
        let input = stable_dir.join("crate.0123456789abcdef.rcgu.o");
        std::fs::write(&input, b"object").unwrap();
        let mut permissions = std::fs::metadata(&input).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&input, permissions).unwrap();
        let input_file = crate::input_data::InputFile::from_path_for_testing(&input);
        let mut input_files = vec![FileState {
            path: encode_path(&input),
            content: FileContentState::from_path_identity_only(&input).unwrap(),
            snapshot_identity: None,
            patch: None,
        }];
        let sections = vec![SectionRecord {
            input_file: encode_path(&input).into(),
            input: encode_path(&input).into(),
            section_index: 1,
            output_offset: 0,
            size: 6,
        }];

        assert_eq!(
            snapshot_loaded_input_files(
                &state_dir,
                &[&input_file],
                &mut input_files,
                &sections,
                false,
            )
            .unwrap(),
            1
        );
        assert!(input_files[0].content.hash.is_empty());

        let replacement = dir.path().join("replacement.o");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(&replacement, &input).unwrap();
        hash_loaded_input_files(&[&input_file], &mut input_files);

        assert_eq!(input_files[0].content.hash, hash_bytes(b"object"));
        assert_eq!(
            read_verified_input_snapshot(&state_dir, &input_files[0]).unwrap(),
            Some(b"object".to_vec())
        );
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have threads")]
    fn deferred_loaded_input_hashes_fill_identity_only_content() {
        let bytes = vec![b'x'; 256 * 1024 + 1];
        let expected_hash = hash_bytes(&bytes);
        let mut input_files = vec![FileState {
            path: encode_path(Path::new("libcrate.rlib")),
            content: FileContentState {
                len: bytes.len() as u64,
                hash: String::new(),
                identity: None,
            },
            snapshot_identity: None,
            patch: None,
        }];

        let mut expected_inputs = vec![ExpectedInputContent::from_content(
            Path::new("libcrate.rlib"),
            &input_files[0].content,
        )];
        let hashes = std::thread::spawn(move || {
            hash_deferred_loaded_input_contents(&[(0, 0, bytes)], false)
        })
        .join()
        .unwrap();
        install_deferred_loaded_input_content_hashes(
            &mut input_files,
            &mut expected_inputs,
            hashes,
        )
        .unwrap();

        assert_eq!(input_files[0].content.hash, expected_hash);
        assert_eq!(expected_inputs[0].hash, expected_hash);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn installed_rust_snapshot_refreshes_after_atomic_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("crate.0123456789abcdef.rcgu.o");
        std::fs::write(&input, b"object").unwrap();

        snapshot_loaded_input_file(&state_dir, &input, b"object", true).unwrap();
        let replacement = dir.path().join("replacement.o");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(&replacement, &input).unwrap();

        let (_, _, snapshot_identity) =
            snapshot_loaded_input_file(&state_dir, &input, b"changed", true).unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        assert_eq!(std::fs::read(&snapshot).unwrap(), b"changed");
        assert_eq!(snapshot_identity, FileIdentity::from_path(&input).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn pending_reuse_hashes_ambiguous_unsnapshotted_input() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.o");
        std::fs::write(&input, b"object").unwrap();
        let input_file = crate::input_data::InputFile::from_path_for_testing(&input);
        let mut input_files = vec![FileState {
            path: encode_path(&input),
            content: FileContentState::from_path_identity_only(&input).unwrap(),
            snapshot_identity: None,
            patch: None,
        }];
        let link_start = FileIdentity::from_path(&input).unwrap();

        hash_pending_reuse_input_files(&[&input_file], &mut input_files, link_start.as_ref());

        assert_eq!(input_files[0].content.hash, hash_bytes(b"object"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn replaced_rust_hardlink_snapshot_matches_previous_content() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("crate.0123456789abcdef.rcgu.o");
        std::fs::write(&input, b"object").unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        assert!(hardlink_rust_snapshot_bytes(&input, &snapshot));
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, b"object"),
            snapshot_identity: FileIdentity::from_path(&snapshot).unwrap(),
            patch: None,
        };

        let replacement = dir.path().join("replacement.o");
        std::fs::write(&replacement, b"object").unwrap();
        std::fs::rename(&replacement, &input).unwrap();

        assert!(input_content_matches_previous(&state_dir, &previous, &input).unwrap());
        assert_eq!(
            read_verified_input_snapshot(&state_dir, &previous).unwrap(),
            Some(b"object".to_vec())
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn mutated_rust_hardlink_snapshot_cannot_match_previous_content() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("libcrate.rlib");
        std::fs::write(&input, b"object").unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        assert!(hardlink_rust_snapshot_bytes(&input, &snapshot));
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, b"object"),
            snapshot_identity: FileIdentity::from_path(&snapshot).unwrap(),
            patch: None,
        };

        std::fs::write(&input, b"damage").unwrap();

        assert!(!input_content_matches_previous(&state_dir, &previous, &input).unwrap());
        assert!(
            read_verified_input_snapshot(&state_dir, &previous)
                .unwrap()
                .is_none()
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn compressed_loaded_snapshot_hashes_and_round_trips_loaded_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("snapshot.o");

        let hash = write_compressed_loaded_snapshot(b"loaded-object", &target, true)
            .unwrap()
            .unwrap();

        assert_eq!(hash, hash_bytes(b"loaded-object"));
        assert_eq!(
            zstd::stream::decode_all(std::fs::read(target).unwrap().as_slice()).unwrap(),
            b"loaded-object"
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn verified_snapshot_reads_compressed_fallback_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        let compressed_snapshot = compressed_input_snapshot_path(&state_dir, &input);
        std::fs::create_dir_all(compressed_snapshot.parent().unwrap()).unwrap();
        write_compressed_loaded_snapshot(b"loaded-object", &compressed_snapshot, false).unwrap();
        let previous = FileState {
            path: encode_path(&input),
            content: FileContentState::from_bytes(b"loaded-object"),
            snapshot_identity: None,
            patch: None,
        };

        assert_eq!(
            read_verified_input_snapshot(&state_dir, &previous)
                .unwrap()
                .unwrap(),
            b"loaded-object"
        );
    }

    #[test]
    fn loaded_input_hash_matches_regular_hash_for_parallel_input() {
        let bytes = vec![7; 512 * 1024];

        assert_eq!(hash_loaded_input_bytes(&bytes), hash_bytes(&bytes));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn stable_identity_read_records_matching_parallel_hashed_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.o");
        let expected = vec![7; 512 * 1024];
        std::fs::write(&path, &expected).unwrap();

        let (bytes, content) = read_file_with_stable_identity(&path).unwrap().unwrap();

        assert_eq!(bytes, expected);
        assert_eq!(content, FileContentState::from_path(&path).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn input_identity_refresh_can_target_changed_indices() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.o");
        let second = dir.path().join("second.o");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let mut input_files = vec![
            FileState {
                path: encode_path(&first),
                content: FileContentState::from_bytes(b""),
                snapshot_identity: None,
                patch: None,
            },
            FileState {
                path: encode_path(&second),
                content: FileContentState::from_bytes(b""),
                snapshot_identity: None,
                patch: None,
            },
        ];

        refresh_input_file_identities_at_indices(&mut input_files, [1, 1, 99]);

        assert!(input_files[0].content.identity.is_none());
        assert_eq!(input_files[0].content.len, 0);
        assert!(
            input_files[1]
                .content
                .identity_matches_path(&second)
                .unwrap()
        );
        assert_eq!(input_files[1].content.len, 6);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn input_snapshot_matches_rewritten_file_with_same_content() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, b"object").unwrap();

        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let mut previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, b"object"),
            snapshot_identity: None,
            patch: None,
        };
        refresh_input_file_identities(std::slice::from_mut(&mut previous));

        let replacement = dir.path().join("replacement.o");
        std::fs::write(&replacement, b"object").unwrap();
        std::fs::rename(&replacement, &input).unwrap();

        assert!(!previous.content.identity_matches_path(&input).unwrap());
        assert!(input_content_matches_previous(&state_dir, &previous, &input).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn input_snapshot_rejects_rewritten_file_with_changed_content() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, b"object").unwrap();

        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let mut previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, b"object"),
            snapshot_identity: None,
            patch: None,
        };
        refresh_input_file_identities(std::slice::from_mut(&mut previous));

        let replacement = dir.path().join("replacement.o");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(&replacement, &input).unwrap();

        assert!(!input_content_matches_previous(&state_dir, &previous, &input).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn hashless_snapshot_is_not_trusted_for_reuse_or_patching() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, b"object").unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        let previous = FileState {
            path: encode_path(&input),
            content: FileContentState {
                len: 6,
                hash: String::new(),
                identity: None,
            },
            snapshot_identity: FileIdentity::from_path(&snapshot).unwrap(),
            patch: None,
        };

        assert!(!input_content_matches_previous(&state_dir, &previous, &input).unwrap());
        assert_eq!(
            read_verified_input_snapshot(&state_dir, &previous).unwrap(),
            None
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn trusted_hashless_atomic_replacement_snapshot_requires_unchanged_identity() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("libcrate.rlib");
        std::fs::write(&input, b"object").unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        let previous = FileState {
            path: encode_path(&input),
            content: FileContentState {
                len: 6,
                hash: String::new(),
                identity: FileIdentity::from_path(&input).unwrap(),
            },
            snapshot_identity: FileIdentity::from_path(&snapshot).unwrap(),
            patch: None,
        };

        assert!(
            !snapshot_identity_matches_previous_atomic_replacement_input(
                &previous, &snapshot, false,
            )
        );
        assert!(snapshot_identity_matches_previous_atomic_replacement_input(
            &previous, &snapshot, true,
        ));

        let copied_snapshot = state_dir.join("copied");
        std::fs::write(&copied_snapshot, b"object").unwrap();
        let mut copied_previous = previous.clone();
        copied_previous.snapshot_identity = FileIdentity::from_path(&copied_snapshot).unwrap();
        assert!(
            !snapshot_identity_matches_previous_atomic_replacement_input(
                &copied_previous,
                &copied_snapshot,
                true,
            )
        );

        let damaged_snapshot = state_dir.join("damaged");
        std::fs::write(&damaged_snapshot, b"damage").unwrap();
        std::fs::rename(&damaged_snapshot, &snapshot).unwrap();

        assert!(
            !snapshot_identity_matches_previous_atomic_replacement_input(
                &previous, &snapshot, true,
            )
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn content_hash_verifies_snapshot_until_snapshot_changes() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("libcrate.rlib");
        std::fs::write(&input, b"object").unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, b"object"),
            snapshot_identity: FileIdentity::from_path(&snapshot).unwrap(),
            patch: None,
        };

        assert_eq!(
            read_verified_input_snapshot(&state_dir, &previous)
                .unwrap()
                .unwrap(),
            b"object"
        );
        assert!(
            !snapshot_identity_matches_previous_atomic_replacement_input(
                &previous, &snapshot, true,
            )
        );
        std::fs::write(&snapshot, b"damage").unwrap();
        assert!(
            read_verified_input_snapshot(&state_dir, &previous)
                .unwrap()
                .is_none()
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_patch_sections_identifies_changed_section() {
        let Ok(current_exe) = std::env::current_exe() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&current_exe) else {
            return;
        };
        let Ok(object) = object::File::parse(&*bytes) else {
            return;
        };
        let Some(section) = object.section_by_name(".data") else {
            return;
        };
        let Some((offset, size)) = section.file_range() else {
            return;
        };
        if size == 0 {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, &bytes).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, &bytes),
            snapshot_identity: None,
            patch: None,
        };
        let input_ref = encode_path(&input);
        let mut current = bytes.clone();
        current[offset as usize] ^= 1;
        let patch_section = PatchSection {
            input: input_ref,
            section_index: section.index().0 as u32,
            section_name: patch_section_name_for_matching(&section),
            input_size: size,
            output_offset: 64,
            output_size: size,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert_eq!(
            changed_patch_sections(
                &state_dir,
                &previous,
                &current,
                &[MatchedPatchSection::same(patch_section)]
            )
            .unwrap()
            .unwrap()
            .len(),
            1
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn match_patch_sections_identifies_changed_section() {
        let Ok(current_exe) = std::env::current_exe() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&current_exe) else {
            return;
        };
        let Ok(object) = object::File::parse(&*bytes) else {
            return;
        };
        let Some(section) = object.section_by_name(".data") else {
            return;
        };
        let Some((offset, size)) = section.file_range() else {
            return;
        };
        if size == 0 {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, &bytes).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, &bytes),
            snapshot_identity: None,
            patch: None,
        };
        let input_ref = encode_path(&input);
        let mut current = bytes.clone();
        current[offset as usize] ^= 1;
        let patch_section = PatchSection {
            input: input_ref,
            section_index: section.index().0 as u32,
            section_name: patch_section_name_for_matching(&section),
            input_size: size,
            output_offset: 64,
            output_size: size,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        let matched = match_patch_sections(&state_dir, &previous, &current, &[patch_section])
            .unwrap()
            .unwrap();

        assert_eq!(matched.sections.len(), 1);
        assert_eq!(matched.changed_sections.len(), 1);
        assert_eq!(
            matched.changed_sections[0].section_index,
            section.index().0 as u32
        );
        assert_eq!(matched.changed_sections[0].input_size, size);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn match_patch_sections_records_current_section_size_after_growth() {
        let bytes = growable_data_elf();
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, &bytes).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, &bytes),
            snapshot_identity: None,
            patch: None,
        };
        let input_ref = encode_path(&input);
        let patch_section = PatchSection {
            input: input_ref,
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let mut current = bytes.clone();
        current[0x44] = 5;
        current[0xe0..0xe8].copy_from_slice(&5_u64.to_le_bytes());

        let matched = match_patch_sections(&state_dir, &previous, &current, &[patch_section])
            .unwrap()
            .unwrap();

        assert_eq!(matched.sections.len(), 1);
        assert_eq!(matched.changed_sections.len(), 1);
        assert_eq!(matched.sections[0].current.input_size, 5);
        assert_eq!(matched.changed_sections[0].input_size, 5);
    }

    #[test]
    fn match_patch_sections_uses_current_hashes_for_stable_names() {
        let bytes = growable_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: Some(hash_bytes(&[1, 2, 3, 4])),
            cstring_nul_boundaries_hash: None,
        };
        let mut current = bytes.clone();
        current[0x40] = 9;

        let matched =
            match_patch_sections_from_current_hashes(&current, &input_ref, &[patch_section])
                .unwrap()
                .unwrap();

        assert_eq!(matched.sections.len(), 1);
        assert_eq!(matched.sections[0].current.section_index, 1);
        assert_eq!(
            matched.sections[0].current.data_hash.as_deref(),
            Some(hash_bytes(&[9, 2, 3, 4]).as_str())
        );
        assert_eq!(matched.changed_sections.len(), 1);
        assert_eq!(
            matched.changed_sections[0].data_hash.as_deref(),
            Some(hash_bytes(&[9, 2, 3, 4]).as_str())
        );
    }

    #[test]
    fn current_hash_matching_requires_hashes_and_rejects_changed_anonymous_sections() {
        let bytes = growable_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let mut patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(
            match_patch_sections_from_current_hashes(
                &bytes,
                &input_ref,
                std::slice::from_ref(&patch_section),
            )
            .unwrap()
            .is_none()
        );

        patch_section.data_hash = Some(hash_bytes(&[1, 2, 3, 4]));
        patch_section.section_name = None;
        let matched = match_patch_sections_from_current_hashes(
            &bytes,
            &input_ref,
            std::slice::from_ref(&patch_section),
        )
        .unwrap()
        .unwrap();
        assert!(matched.changed_sections.is_empty());

        let mut current = bytes.clone();
        current[0x40] = 9;
        assert!(
            match_patch_sections_from_current_hashes(&current, &input_ref, &[patch_section])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn reference_matching_resolves_unique_anonymous_section() {
        let signature = vec![section_reference(".text.foo", 12)];
        let current_references = HashMap::from([
            (
                object::SectionIndex(3),
                vec![section_reference(".text.bar", 4)],
            ),
            (object::SectionIndex(7), signature.clone()),
        ]);

        assert_eq!(
            match_section_by_references(&signature, &current_references),
            Some(object::SectionIndex(7))
        );
    }

    #[test]
    fn reference_matching_rejects_ambiguous_anonymous_section() {
        let signature = vec![section_reference(".text.foo", 12)];
        let current_references = HashMap::from([
            (object::SectionIndex(3), signature.clone()),
            (object::SectionIndex(7), signature.clone()),
        ]);

        assert_eq!(
            match_section_by_references(&signature, &current_references),
            None
        );
    }

    #[test]
    fn anonymous_patch_reference_counts_classify_ambiguity() {
        let signature = vec![section_reference(".text.foo", 12)];
        let previous_references = HashMap::from([(object::SectionIndex(2), signature.clone())]);
        let current_references = HashMap::from([
            (object::SectionIndex(3), signature.clone()),
            (object::SectionIndex(7), signature),
        ]);

        assert_eq!(
            anonymous_patch_reference_counts(
                object::SectionIndex(2),
                &previous_references,
                &current_references,
            ),
            (1, 2)
        );
        assert_eq!(
            anonymous_patch_reference_counts(
                object::SectionIndex(4),
                &previous_references,
                &current_references,
            ),
            (0, 0)
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn rejects_changed_unreferenced_anonymous_elf_section_with_same_index_name() {
        let mut bytes = growable_data_elf();
        bytes[0x49..0x4e].copy_from_slice(b".L__0");
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, &bytes).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, &bytes),
            snapshot_identity: None,
            patch: None,
        };
        let input_ref = encode_path(&input);
        let mut current = bytes.clone();
        current[0x40] = 9;
        let patch_section = PatchSection {
            input: input_ref,
            section_index: 1,
            section_name: None,
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(
            match_patch_sections(&state_dir, &previous, &current, &[patch_section])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn patched_section_records_follow_current_section_identity() {
        let input_file = hex::encode("input.o");
        let input_ref = input_file.clone();
        let unrelated_input = hex::encode("other.o");
        let mut records = vec![
            SectionRecord {
                input_file: input_file.clone().into(),
                input: input_ref.clone().into(),
                section_index: 3,
                output_offset: 64,
                size: 16,
            },
            SectionRecord {
                input_file: unrelated_input.into(),
                input: input_ref.clone().into(),
                section_index: 3,
                output_offset: 64,
                size: 16,
            },
        ];
        let previous = PatchSection {
            input: input_ref.clone(),
            section_index: 3,
            section_name: None,
            input_size: 8,
            output_offset: 64,
            output_size: 16,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let current = PatchSection {
            input: input_ref.clone(),
            section_index: 7,
            section_name: None,
            input_size: 9,
            output_offset: 64,
            output_size: 16,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(update_section_records_for_matched_patches(
            &input_file,
            &[MatchedPatchSection { previous, current }],
            &mut records,
        ));

        assert_eq!(records[0].section_index, 7);
        assert_eq!(records[0].size, 16);
        assert_eq!(records[1].section_index, 3);
    }

    #[test]
    fn matched_patch_sections_follow_resolved_current_sections() {
        let input_ref = hex::encode("input.o");
        let previous = PatchSection {
            input: input_ref.clone(),
            section_index: 3,
            section_name: Some(".data.old".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 16,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let current = PatchSection {
            input: input_ref,
            section_index: 7,
            section_name: Some(".data.old".to_owned()),
            input_size: 9,
            output_offset: 64,
            output_size: 16,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let mut matched_sections = vec![MatchedPatchSection::same(previous.clone())];

        update_matched_patch_current_sections(&mut matched_sections, &[current.clone()]);

        assert_eq!(
            matched_sections[0].previous.section_index,
            previous.section_index
        );
        assert_eq!(
            matched_sections[0].current.section_index,
            current.section_index
        );
        assert_eq!(matched_sections[0].current.input_size, current.input_size);
    }

    #[test]
    fn patch_sections_for_input_rejects_section_growth_beyond_capacity() {
        let Ok(current_exe) = std::env::current_exe() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&current_exe) else {
            return;
        };
        let Ok(object) = object::File::parse(&*bytes) else {
            return;
        };
        let Some(section) = object
            .sections()
            .find(|section| section.file_range().is_some_and(|(_, size)| size > 0))
        else {
            return;
        };
        let Some((_, size)) = section.file_range() else {
            return;
        };
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: section.index().0 as u32,
            section_name: section.name().ok().map(str::to_owned),
            input_size: size,
            output_offset: 64,
            output_size: size - 1,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(
            patch_sections_for_input(&bytes, &input_ref, [patch_section])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn patch_fingerprint_allows_section_size_growth_within_capacity() {
        let bytes = growable_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let previous_fingerprint = patch_fingerprint(&bytes, &input_ref, [patch_section.clone()])
            .unwrap()
            .unwrap();

        let mut current = bytes.clone();
        current[0x44] = 5;
        current[0xe0..0xe8].copy_from_slice(&5_u64.to_le_bytes());

        assert_eq!(
            patch_fingerprint(&current, &input_ref, [patch_section.clone()])
                .unwrap()
                .unwrap(),
            previous_fingerprint
        );
        let patches = patch_sections_for_input(&current, &input_ref, [patch_section])
            .unwrap()
            .unwrap();

        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].size, 8);
        assert_eq!(patches[0].data, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn patch_fingerprint_rejects_relocation_metadata_changes() {
        let bytes = relocated_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let previous_fingerprint = patch_fingerprint(&bytes, &input_ref, [patch_section.clone()])
            .unwrap()
            .unwrap();

        let mut data_changed = bytes.clone();
        data_changed[0x40] ^= 1;
        assert_eq!(
            patch_fingerprint(&data_changed, &input_ref, [patch_section.clone()])
                .unwrap()
                .unwrap(),
            previous_fingerprint
        );

        let mut relocation_changed = bytes.clone();
        relocation_changed[0x80] ^= 1;
        assert_ne!(
            patch_fingerprint(&relocation_changed, &input_ref, [patch_section])
                .unwrap()
                .unwrap(),
            previous_fingerprint
        );
    }

    #[test]
    fn patch_fingerprint_allows_duplicate_extra_ranges() {
        let bytes = growable_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        let fingerprint = patch_fingerprint_with_extra_ranges(
            &bytes,
            &input_ref,
            [patch_section],
            [0x40..0x44, 0x40..0x44],
        )
        .unwrap();

        assert!(fingerprint.is_some());
    }

    #[test]
    fn metadata_only_fingerprint_matches_previous_without_extra_ranges() {
        let bytes = relocated_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let previous_with_extra = patch_fingerprint_with_extra_ranges(
            &bytes,
            &input_ref,
            [patch_section.clone()],
            [0x90..0x98],
        )
        .unwrap()
        .unwrap();

        let mut current = bytes.clone();
        current[0x40] ^= 1;
        let current_without_extra =
            patch_fingerprint(&current, &input_ref, [patch_section.clone()])
                .unwrap()
                .unwrap();
        assert_ne!(previous_with_extra, current_without_extra);

        assert!(
            patch_fingerprint_matches_previous_without_extra_ranges(
                &bytes,
                current_without_extra.as_str(),
                &input_ref,
                &[MatchedPatchSection::same(patch_section)],
                true,
            )
            .unwrap()
        );
    }

    #[test]
    fn patch_fingerprint_allows_dynamic_relocation_addend_changes() {
        let bytes = relocated_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let relocation = dynamic_relocation_record("input.o", 1, 4, 300, 24);
        let previous_patches =
            dynamic_relocation_patches_for_input(&bytes, &input_ref, [&relocation]).unwrap();
        let previous_fingerprint = patch_fingerprint_with_extra_ranges(
            &bytes,
            &input_ref,
            [patch_section.clone()],
            previous_patches
                .iter()
                .filter_map(|patch| patch.input_range.clone()),
        )
        .unwrap()
        .unwrap();

        let mut addend_changed = bytes.clone();
        addend_changed[0x90] ^= 1;
        let current_patches =
            dynamic_relocation_patches_for_input(&addend_changed, &input_ref, [&relocation])
                .unwrap();

        assert_eq!(
            patch_fingerprint_with_extra_ranges(
                &addend_changed,
                &input_ref,
                [patch_section],
                current_patches
                    .iter()
                    .filter_map(|patch| patch.input_range.clone()),
            )
            .unwrap()
            .unwrap(),
            previous_fingerprint
        );
        assert_eq!(current_patches.len(), 1);
        assert_eq!(current_patches[0].input_range, Some(0x90..0x98));
        assert_eq!(current_patches[0].patch.output_offset, 300);
        assert_eq!(current_patches[0].patch.size, 24);
        assert_eq!(current_patches[0].patch.preserve_ranges, vec![0..16]);
        assert_eq!(
            &current_patches[0].patch.data[16..24],
            &addend_changed[0x90..0x98]
        );
    }

    #[test]
    fn patch_fingerprint_rejects_dynamic_relocation_offset_changes() {
        let bytes = relocated_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let relocation = dynamic_relocation_record("input.o", 1, 4, 300, 24);
        let previous_patches =
            dynamic_relocation_patches_for_input(&bytes, &input_ref, [&relocation]).unwrap();
        let previous_fingerprint = patch_fingerprint_with_extra_ranges(
            &bytes,
            &input_ref,
            [patch_section.clone()],
            previous_patches
                .iter()
                .filter_map(|patch| patch.input_range.clone()),
        )
        .unwrap()
        .unwrap();

        let mut offset_changed = bytes.clone();
        offset_changed[0x80] ^= 1;
        let current_patches =
            dynamic_relocation_patches_for_input(&offset_changed, &input_ref, [&relocation])
                .unwrap();

        assert_ne!(
            patch_fingerprint_with_extra_ranges(
                &offset_changed,
                &input_ref,
                [patch_section],
                current_patches
                    .iter()
                    .filter_map(|patch| patch.input_range.clone()),
            )
            .unwrap()
            .unwrap(),
            previous_fingerprint
        );
    }

    #[test]
    fn dynamic_relocation_patch_tombstones_missing_relocation_entry() {
        let bytes = growable_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let relocation = dynamic_relocation_record("input.o", 1, 4, 300, 24);

        let patches =
            dynamic_relocation_patches_for_input(&bytes, &input_ref, [&relocation]).unwrap();

        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].record, relocation);
        assert_eq!(patches[0].input_range, None);
        assert_eq!(patches[0].patch.output_offset, 300);
        assert_eq!(patches[0].patch.size, 24);
        assert_eq!(patches[0].patch.data, vec![0; 24]);
        assert!(patches[0].patch.preserve_ranges.is_empty());
    }

    #[test]
    fn dynamic_relocation_patch_restores_recorded_output_info() {
        let bytes = relocated_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let relocation = dynamic_relocation_record_with_output_info(
            "input.o",
            1,
            4,
            300,
            24,
            0x400040,
            0x100000006,
        );

        let patches =
            dynamic_relocation_patches_for_input(&bytes, &input_ref, [&relocation]).unwrap();

        assert_eq!(patches.len(), 1);
        assert_eq!(
            patches[0].patch.preserve_ranges,
            Vec::<std::ops::Range<usize>>::new()
        );
        assert_eq!(&patches[0].patch.data[0..8], &0x400040_u64.to_le_bytes());
        assert_eq!(
            &patches[0].patch.data[8..16],
            &0x100000006_u64.to_le_bytes()
        );
        assert_eq!(&patches[0].patch.data[16..24], &bytes[0x90..0x98]);
    }

    #[test]
    fn dynamic_relocation_patch_uses_free_slot_for_added_relocation() {
        let previous = relocated_data_elf();
        let current = relocated_data_elf_with_added_relocation();
        let input_ref = encode_path(Path::new("input.o"));
        let relocation = dynamic_relocation_record_with_output_info(
            "input.o",
            1,
            4,
            300,
            24,
            0x400040,
            0x100000006,
        );
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let sections = vec![generated_section_record(
            "generated:.rela.dyn.general",
            300,
            48,
        )];

        let patches = added_dynamic_relocation_patches_for_input(
            &current,
            &previous,
            &input_ref,
            &[MatchedPatchSection::same(patch_section.clone())],
            std::slice::from_ref(&relocation),
            &sections,
        );

        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].record.relocation_offset, 6);
        assert_eq!(patches[0].record.output_offset, 324);
        assert_eq!(patches[0].record.output_r_offset, Some(0x400042));
        assert_eq!(patches[0].record.output_r_info, Some(0x100000006));
        assert_eq!(patches[0].input_range, Some(0xa8..0xb0));
        assert_eq!(patches[0].patch.output_offset, 324);
        assert_eq!(&patches[0].patch.data[0..8], &0x400042_u64.to_le_bytes());
        assert_eq!(
            &patches[0].patch.data[8..16],
            &0x100000006_u64.to_le_bytes()
        );
        assert_eq!(&patches[0].patch.data[16..24], &9_i64.to_le_bytes());

        let mut all_patches =
            dynamic_relocation_patches_for_input(&current, &input_ref, [&relocation]).unwrap();
        all_patches.extend(patches);
        assert!(
            object_diff_allows_dynamic_relocation_addition(
                &previous,
                &current,
                &input_ref,
                &[MatchedPatchSection::same(patch_section)],
                &all_patches,
            )
            .unwrap()
        );
    }

    #[test]
    fn object_diff_allows_dynamic_relocation_addition_from_metadata_only_change() {
        let current = relocated_data_elf();
        let mut previous = current.clone();
        let rela_header = 0x100 + 128;
        previous[rela_header + 32..rela_header + 40].copy_from_slice(&0_u64.to_le_bytes());
        let input_ref = encode_path(Path::new("input.o"));
        let relocation = dynamic_relocation_record_with_output_info(
            "input.o",
            1,
            4,
            300,
            24,
            0x400040,
            0x100000006,
        );
        let patches =
            dynamic_relocation_patches_for_input(&current, &input_ref, [&relocation]).unwrap();
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(
            object_diff_allows_dynamic_relocation_addition(
                &previous,
                &current,
                &input_ref,
                &[MatchedPatchSection::same(patch_section)],
                &patches,
            )
            .unwrap()
        );
    }

    #[test]
    fn object_diff_rejects_dynamic_relocation_addition_without_free_slot_info() {
        let current = relocated_data_elf();
        let mut previous = current.clone();
        let rela_header = 0x100 + 128;
        previous[rela_header + 32..rela_header + 40].copy_from_slice(&0_u64.to_le_bytes());
        let input_ref = encode_path(Path::new("input.o"));
        let relocation = dynamic_relocation_record("input.o", 1, 4, 300, 24);
        let patches =
            dynamic_relocation_patches_for_input(&current, &input_ref, [&relocation]).unwrap();
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(
            !object_diff_allows_dynamic_relocation_addition(
                &previous,
                &current,
                &input_ref,
                &[MatchedPatchSection::same(patch_section)],
                &patches,
            )
            .unwrap()
        );
    }

    #[test]
    fn fde_removal_diff_ignores_relocation_metadata_sections() {
        assert!(section_name_is_metadata_for_fde_removal(
            ".rela.text.removed_fde_target"
        ));
        assert!(section_name_is_metadata_for_fde_removal(
            ".rel.text.removed_fde_target"
        ));
        assert!(section_name_is_metadata_for_fde_removal(".rela.eh_frame"));
    }

    #[test]
    fn object_diff_allows_fde_removal_with_surviving_fde_updates() {
        let bytes = eh_frame_relocation_elf(8, -4);
        let input_ref = encode_path(Path::new("input.o"));
        let removed_fde = fde_record("input.o", 2, 2, 0, 300, 16);
        let patches = vec![
            FdeRelocationPatch {
                input_ranges: Vec::new(),
                patch: None,
                eh_frame_hdr_change: Some(EhFrameHdrChange::Remove(removed_fde)),
                record_update: None,
            },
            FdeRelocationPatch {
                input_ranges: Vec::new(),
                patch: Some(SectionPatch {
                    output_offset: 320,
                    size: 16,
                    data: vec![0; 16],
                    deferred_relocation: None,
                    preserve_ranges: Vec::new(),
                    adjustments: Vec::new(),
                }),
                eh_frame_hdr_change: Some(EhFrameHdrChange::Adjust(EhFrameHdrDelta {
                    fde_output_offset: 320,
                    frame_ptr_delta: 4,
                })),
                record_update: None,
            },
        ];

        assert!(object_diff_allows_fde_removal(&bytes, &bytes, &input_ref, &[], &patches).unwrap());
    }

    #[test]
    fn patch_fingerprint_allows_relocation_addend_changes() {
        let bytes = relocated_data_elf();
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some(0x1000),
            0x1000,
            None,
            None,
            4,
            300,
            8,
            1,
            2,
        );
        let previous_ranges =
            relocation_addend_ranges_for_input(&bytes, &input_ref, [&relocation]).unwrap();
        let previous_fingerprint = patch_fingerprint_with_extra_ranges(
            &bytes,
            &input_ref,
            [patch_section.clone()],
            previous_ranges,
        )
        .unwrap()
        .unwrap();

        let mut addend_changed = bytes.clone();
        addend_changed[0x90..0x98].copy_from_slice(&5_i64.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &bytes)]);
        let input = state.input_files.remove(0);
        let mut relocations = vec![relocation];
        let patches = relocation_addend_patches_for_input(
            &mut relocations,
            &input,
            &addend_changed,
            None,
            &[],
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            patch_fingerprint_with_extra_ranges(
                &addend_changed,
                &input_ref,
                [patch_section],
                patches.input_ranges.clone(),
            )
            .unwrap()
            .unwrap(),
            previous_fingerprint
        );
        assert_eq!(patches.input_ranges, vec![0x90..0x98]);
        assert_eq!(patches.output_patches.len(), 1);
        assert_eq!(patches.output_patches[0].output_offset, 300);
        assert_eq!(patches.output_patches[0].size, 8);
        assert_eq!(
            patches.output_patches[0].data,
            0x1003_u64.to_le_bytes().to_vec()
        );
        assert_eq!(relocations[0].addend, 5);
        assert_eq!(relocations[0].written_value, Some(0x1003));
    }

    #[test]
    fn dynamic_relocation_addend_does_not_patch_output_data_word() {
        let bytes = relocated_data_elf();
        let mut addend_changed = bytes.clone();
        addend_changed[0x90..0x98].copy_from_slice(&5_i64.to_le_bytes());
        let mut state = state("args", b"output", &[("input.o", &bytes)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some(0x1000),
            0x1000,
            None,
            None,
            4,
            300,
            8,
            1,
            2,
        );
        let dynamic_relocation = dynamic_relocation_record("input.o", 1, 4, 400, 24);
        let mut relocations = vec![relocation];
        let patches = relocation_addend_patches_for_input(
            &mut relocations,
            &input,
            &addend_changed,
            None,
            &[dynamic_relocation],
        )
        .unwrap()
        .unwrap();

        assert_eq!(patches.input_ranges, vec![0x90..0x98]);
        assert!(patches.output_patches.is_empty());
        assert_eq!(relocations[0].addend, 5);
        assert_eq!(relocations[0].written_value, Some(0x1000));
    }

    #[test]
    fn relocation_addend_patch_ignores_unchanged_raw_addends_for_unsupported_sizes() {
        let mut previous = relocated_data_elf();
        previous[0x88..0x90]
            .copy_from_slice(&u64::from(object::elf::R_X86_64_GOTPCREL).to_le_bytes());
        previous[0x90..0x98].copy_from_slice(&(-4_i64).to_le_bytes());
        let current = previous.clone();
        let mut state = state("args", b"output", &[("input.o", &previous)]);
        let input = state.input_files.remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            42,
            Some(0x1000),
            0x1000,
            None,
            None,
            4,
            300,
            4,
            42,
            0,
        );

        let mut relocations = vec![relocation.clone()];
        let patches = relocation_addend_patches_for_input(
            &mut relocations,
            &input,
            &current,
            Some(&previous),
            &[],
        )
        .unwrap()
        .unwrap();

        assert_eq!(patches.input_ranges, vec![0x90..0x98]);
        assert!(patches.output_patches.is_empty());
        assert_eq!(relocations[0].addend, 0);

        let mut relocations = vec![relocation];
        let err = match relocation_addend_patches_for_input(
            &mut relocations,
            &input,
            &current,
            None,
            &[],
        )
        .unwrap()
        {
            Ok(_) => panic!("unsupported relocation addend unexpectedly patched"),
            Err(err) => err,
        };
        assert!(err.contains("unsupported relocation addend patch size"));
    }

    #[test]
    fn patch_fingerprint_allows_fde_relocation_addend_changes() {
        let previous = eh_frame_relocation_elf(8, -4);
        let current = eh_frame_relocation_elf(8, 2);
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".text".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 4,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let fde = fde_record("input.o", 1, 2, 0, 300, 16);
        let previous_ranges =
            fde_patch_input_ranges_for_input(&previous, &input_ref, [&fde]).unwrap();
        let previous_fingerprint = patch_fingerprint_with_extra_ranges(
            &previous,
            &input_ref,
            [patch_section.clone()],
            previous_ranges.into_iter(),
        )
        .unwrap()
        .unwrap();
        let current_patches =
            fde_relocation_patches_for_input(&current, &previous, &input_ref, [&fde]).unwrap();

        assert_eq!(
            patch_fingerprint_with_extra_ranges(
                &current,
                &input_ref,
                [patch_section],
                current_patches
                    .iter()
                    .flat_map(|patch| patch.input_ranges.iter().cloned()),
            )
            .unwrap()
            .unwrap(),
            previous_fingerprint
        );
        assert_eq!(current_patches.len(), 1);
        assert_eq!(
            current_patches[0].input_ranges,
            vec![0x48..0x58, 0x58 + 16..0x58 + 24]
        );
        let patch = current_patches[0].patch.as_ref().unwrap();
        assert_eq!(patch.output_offset, 300);
        assert_eq!(patch.size, 16);
        assert_eq!(patch.preserve_ranges, vec![4..8, 8..12]);
        assert_eq!(patch.adjustments.len(), 1);
        assert_eq!(patch.adjustments[0].range, 8..12);
        assert_eq!(patch.adjustments[0].addend_delta, 6);
        assert!(current_patches[0].record_update.is_none());
        assert!(matches!(
            current_patches[0].eh_frame_hdr_change,
            Some(EhFrameHdrChange::Adjust(_))
        ));
    }

    #[test]
    fn fde_relocation_patches_follow_current_eh_frame_section_name() {
        let previous = eh_frame_relocation_elf(8, -4);
        let current = eh_frame_relocation_elf_with_shifted_eh_frame_index(8, -4);
        let input_ref = encode_path(Path::new("input.o"));
        let fde = fde_record("input.o", 1, 2, 0, 300, 16);

        let current_patches =
            fde_relocation_patches_for_input(&current, &previous, &input_ref, [&fde]).unwrap();

        assert_eq!(current_patches.len(), 1);
        assert_eq!(
            current_patches[0].input_ranges,
            vec![0x48..0x58, 0x58 + 16..0x58 + 24]
        );
        assert!(current_patches[0].patch.is_none());
        assert!(current_patches[0].eh_frame_hdr_change.is_none());
    }

    #[test]
    fn fde_relocation_patches_follow_current_fde_offset() {
        let previous = eh_frame_two_fdes_same_section_elf();
        let current = eh_frame_relocation_elf(8, 2);
        let input_ref = encode_path(Path::new("input.o"));
        let fde = fde_record("input.o", 1, 2, 16, 300, 16);

        let current_patches =
            fde_relocation_patches_for_input(&current, &previous, &input_ref, [&fde]).unwrap();

        assert_eq!(current_patches.len(), 1);
        assert_eq!(
            current_patches[0].input_ranges,
            vec![0x48..0x58, 0x58 + 16..0x58 + 24]
        );
        let patch = current_patches[0].patch.as_ref().unwrap();
        assert_eq!(patch.output_offset, 300);
        assert_eq!(patch.size, 16);
        assert_eq!(patch.preserve_ranges, vec![4..8, 8..12]);
        assert_eq!(patch.adjustments.len(), 1);
        assert_eq!(patch.adjustments[0].range, 8..12);
        assert_eq!(patch.adjustments[0].addend_delta, 6);
        let update = current_patches[0].record_update.as_ref().unwrap();
        assert_eq!(update.previous.input_offset, 16);
        assert_eq!(update.current.input_offset, 0);
        assert!(matches!(
            current_patches[0].eh_frame_hdr_change,
            Some(EhFrameHdrChange::Adjust(_))
        ));
    }

    #[test]
    fn updated_fde_records_replace_previous_offsets() {
        let previous = fde_record("input.o", 1, 2, 16, 300, 16);
        let mut current = previous.clone();
        current.input_offset = 0;
        let mut fdes = vec![previous.clone()];

        update_fde_records(
            &mut fdes,
            vec![FdeRecordUpdate {
                previous,
                current: current.clone(),
            }],
        );

        assert_eq!(fdes, vec![current]);
    }

    #[test]
    fn fde_match_can_use_input_offset_with_multiple_fdes_per_section() {
        let bytes = eh_frame_two_fdes_same_section_elf();
        let section_headers = elf_section_headers(&bytes).unwrap();

        assert_eq!(
            fde_input_range_for_target_section(&bytes, &section_headers, 2, 1)
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            fde_input_range_for_target_section_at_offset(&bytes, &section_headers, 2, 1, 16)
                .unwrap()
                .1,
            16
        );
    }

    #[test]
    fn patch_fingerprint_allows_fde_content_changes() {
        let previous = eh_frame_relocation_elf(8, -4);
        let mut current = previous.clone();
        current[0x54] = 8;
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".text".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 4,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let fde = fde_record("input.o", 1, 2, 0, 300, 16);
        let previous_ranges =
            fde_patch_input_ranges_for_input(&previous, &input_ref, [&fde]).unwrap();
        let previous_fingerprint = patch_fingerprint_with_extra_ranges(
            &previous,
            &input_ref,
            [patch_section.clone()],
            previous_ranges.into_iter(),
        )
        .unwrap()
        .unwrap();
        let current_patches =
            fde_relocation_patches_for_input(&current, &previous, &input_ref, [&fde]).unwrap();

        assert_eq!(
            patch_fingerprint_with_extra_ranges(
                &current,
                &input_ref,
                [patch_section],
                current_patches
                    .iter()
                    .flat_map(|patch| patch.input_ranges.iter().cloned()),
            )
            .unwrap()
            .unwrap(),
            previous_fingerprint
        );
        assert_eq!(current_patches.len(), 1);
        let patch = current_patches[0].patch.as_ref().unwrap();
        assert_eq!(&patch.data[12..16], &[8, 0, 0, 0]);
        assert_eq!(patch.preserve_ranges, vec![4..8, 8..12]);
        assert!(patch.adjustments.is_empty());
        assert!(current_patches[0].eh_frame_hdr_change.is_none());
    }

    #[test]
    fn patch_fingerprint_rejects_fde_relocation_offset_changes() {
        let previous = eh_frame_relocation_elf(8, -4);
        let current = eh_frame_relocation_elf(12, -4);
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".text".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 4,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let fde = fde_record("input.o", 1, 2, 0, 300, 16);
        let previous_ranges =
            fde_patch_input_ranges_for_input(&previous, &input_ref, [&fde]).unwrap();
        let previous_fingerprint = patch_fingerprint_with_extra_ranges(
            &previous,
            &input_ref,
            [patch_section.clone()],
            previous_ranges.into_iter(),
        )
        .unwrap()
        .unwrap();
        let current_patches =
            fde_relocation_patches_for_input(&current, &previous, &input_ref, [&fde]).unwrap();

        assert_eq!(current_patches.len(), 1);
        assert!(current_patches[0].input_ranges.is_empty());
        assert!(current_patches[0].patch.is_none());
        assert_ne!(
            patch_fingerprint_with_extra_ranges(
                &current,
                &input_ref,
                [patch_section],
                current_patches
                    .iter()
                    .flat_map(|patch| patch.input_ranges.iter().cloned()),
            )
            .unwrap()
            .unwrap(),
            previous_fingerprint
        );
    }

    #[test]
    fn patch_fingerprint_rejects_fde_relocation_field_overflow() {
        let previous = eh_frame_relocation_elf(14, -4);
        let current = eh_frame_relocation_elf(14, 2);
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".text".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 4,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };
        let fde = fde_record("input.o", 1, 2, 0, 300, 16);
        let previous_ranges =
            fde_patch_input_ranges_for_input(&previous, &input_ref, [&fde]).unwrap();
        let previous_fingerprint = patch_fingerprint_with_extra_ranges(
            &previous,
            &input_ref,
            [patch_section.clone()],
            previous_ranges.into_iter(),
        )
        .unwrap()
        .unwrap();
        let current_patches =
            fde_relocation_patches_for_input(&current, &previous, &input_ref, [&fde]).unwrap();

        assert_eq!(current_patches.len(), 1);
        assert!(current_patches[0].input_ranges.is_empty());
        assert!(current_patches[0].patch.is_none());
        assert_ne!(
            patch_fingerprint_with_extra_ranges(
                &current,
                &input_ref,
                [patch_section],
                current_patches
                    .iter()
                    .flat_map(|patch| patch.input_ranges.iter().cloned()),
            )
            .unwrap()
            .unwrap(),
            previous_fingerprint
        );
    }

    #[test]
    fn eh_frame_hdr_patch_updates_matching_fde_entry() {
        let output = eh_frame_hdr_output_elf(&[(-48, 0x50), (-16, 0x60)]);
        let patches = eh_frame_hdr_patches_for_fde_changes(
            &output,
            &[EhFrameHdrChange::Adjust(EhFrameHdrDelta {
                fde_output_offset: 0x50,
                frame_ptr_delta: 6,
            })],
        )
        .unwrap()
        .unwrap();

        assert_eq!(patches.len(), 1);
        assert_eq!(
            patches[0].output_offset,
            (0x80 + std::mem::size_of::<crate::elf::EhFrameHdr>()) as u64
        );
        assert_eq!(patches[0].size, 4);
        assert_eq!(patches[0].data, (-42_i32).to_le_bytes());
        assert!(patches[0].preserve_ranges.is_empty());
        assert!(patches[0].adjustments.is_empty());
    }

    #[test]
    fn eh_frame_hdr_patch_rejects_unsorted_result() {
        let output = eh_frame_hdr_output_elf(&[(-48, 0x50), (-16, 0x60)]);
        let result = eh_frame_hdr_patches_for_fde_changes(
            &output,
            &[EhFrameHdrChange::Adjust(EhFrameHdrDelta {
                fde_output_offset: 0x50,
                frame_ptr_delta: 40,
            })],
        )
        .unwrap();
        let Err(error) = result else {
            panic!("expected .eh_frame_hdr patch to be rejected");
        };

        assert!(error.contains("would no longer be sorted"));
    }

    #[test]
    fn eh_frame_hdr_patch_rejects_missing_fde_entry() {
        let output = eh_frame_hdr_output_elf(&[(-48, 0x50), (-16, 0x60)]);
        let result = eh_frame_hdr_patches_for_fde_changes(
            &output,
            &[EhFrameHdrChange::Adjust(EhFrameHdrDelta {
                fde_output_offset: 0x70,
                frame_ptr_delta: 6,
            })],
        )
        .unwrap();
        let Err(error) = result else {
            panic!("expected .eh_frame_hdr patch to be rejected");
        };

        assert!(error.contains("could not find .eh_frame_hdr entry"));
    }

    #[test]
    fn eh_frame_hdr_patch_removes_fde_entry() {
        let output = eh_frame_hdr_output_elf(&[(-48, 0x50), (-16, 0x60), (8, 0x70)]);
        let patches = eh_frame_hdr_patches_for_fde_changes(
            &output,
            &[EhFrameHdrChange::Remove(fde_record(
                "input.o", 1, 2, 0, 0x60, 16,
            ))],
        )
        .unwrap()
        .unwrap();

        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].output_offset, 0x88);
        assert_eq!(patches[0].size, 4);
        assert_eq!(patches[0].data, 2_u32.to_le_bytes());
        assert_eq!(
            patches[1].output_offset,
            (0x80 + std::mem::size_of::<crate::elf::EhFrameHdr>() + 8) as u64
        );
        assert_eq!(patches[1].size, 16);
        assert_eq!(
            patches[1].data,
            [
                8_i32.to_le_bytes().as_slice(),
                (-16_i32).to_le_bytes().as_slice(),
                &[0, 0, 0, 0, 0, 0, 0, 0],
            ]
            .concat()
        );
    }

    #[test]
    fn eh_frame_hdr_patch_adds_fde_entry() {
        let output = eh_frame_hdr_output_elf_with_capacity(&[(-48, 0x50), (8, 0x70)], 3);
        let patches = eh_frame_hdr_patches_for_fde_changes(
            &output,
            &[EhFrameHdrChange::Add(EhFrameHdrEntryPatch {
                frame_ptr: -16,
                frame_info_ptr: -32,
            })],
        )
        .unwrap()
        .unwrap();

        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].output_offset, 0x88);
        assert_eq!(patches[0].size, 4);
        assert_eq!(patches[0].data, 3_u32.to_le_bytes());
        assert_eq!(
            patches[1].output_offset,
            (0x80 + std::mem::size_of::<crate::elf::EhFrameHdr>()) as u64
        );
        assert_eq!(patches[1].size, 24);
        assert_eq!(
            patches[1].data,
            [
                (-48_i32).to_le_bytes().as_slice(),
                (-48_i32).to_le_bytes().as_slice(),
                (-16_i32).to_le_bytes().as_slice(),
                (-32_i32).to_le_bytes().as_slice(),
                8_i32.to_le_bytes().as_slice(),
                (-16_i32).to_le_bytes().as_slice(),
            ]
            .concat()
        );
    }

    #[test]
    fn eh_frame_hdr_patch_rejects_added_fde_without_capacity() {
        let output = eh_frame_hdr_output_elf(&[(-48, 0x50), (8, 0x70)]);
        let result = eh_frame_hdr_patches_for_fde_changes(
            &output,
            &[EhFrameHdrChange::Add(EhFrameHdrEntryPatch {
                frame_ptr: -16,
                frame_info_ptr: -32,
            })],
        )
        .unwrap();
        let Err(error) = result else {
            panic!("expected .eh_frame_hdr addition to be rejected");
        };

        assert!(error.contains("no free .eh_frame_hdr entries"));
    }

    #[test]
    fn fde_add_patch_uses_free_eh_frame_tail() {
        let mut output = eh_frame_hdr_output_elf_with_capacity(&[(-48, 0x48)], 2);
        output[0x40..0x48].copy_from_slice(&[
            4, 0, 0, 0, // CIE length
            0, 0, 0, 0, // CIE id
        ]);
        output[0x48..0x58].copy_from_slice(&[
            12, 0, 0, 0, // FDE length
            12, 0, 0, 0, // output CIE pointer
            0, 0, 0, 0, // relocated pc begin
            4, 0, 0, 0, // pc range
        ]);

        let previous = vec![fde_record("input.o", 1, 2, 0, 0x48, 16)];
        let candidate = FdeAddCandidate {
            input_ranges: vec![0..16],
            input_file: "input.o".to_owned(),
            input: "input.o".to_owned(),
            target_section_index: 1,
            eh_frame_section_index: 2,
            input_offset: 16,
            target_section_offset: 0,
            target_output_offset: 0x40,
            fde_data: vec![12, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0],
            pc_begin_range: 8..12,
            cie_input_offset: 0,
            cie_reference_fde_output_offset: 0x48,
        };

        let resolved = fde_add_patches_for_output(&output, &[candidate], &previous)
            .unwrap()
            .unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].record.output_offset, 0x58);
        assert_eq!(resolved[0].record.size, 16);
        assert_eq!(resolved[0].patch.output_offset, 0x58);
        assert_eq!(resolved[0].patch.size, 20);
        assert_eq!(&resolved[0].patch.data[4..8], &28_u32.to_le_bytes());
        assert_eq!(&resolved[0].patch.data[8..12], &(-32_i32).to_le_bytes());
        assert_eq!(&resolved[0].patch.data[16..20], &0_u32.to_le_bytes());
        assert!(matches!(
            resolved[0].eh_frame_hdr_change,
            Some(EhFrameHdrChange::Add(EhFrameHdrEntryPatch {
                frame_ptr: -64,
                frame_info_ptr: -40,
            }))
        ));
    }

    #[test]
    fn fde_add_patch_rejects_without_free_eh_frame_tail() {
        let mut output = eh_frame_hdr_output_elf_with_capacity(&[(-48, 0x48)], 2);
        output[0x40..0x48].copy_from_slice(&[
            4, 0, 0, 0, // CIE length
            0, 0, 0, 0, // CIE id
        ]);
        output[0x48..0x58].copy_from_slice(&[
            12, 0, 0, 0, // FDE length
            12, 0, 0, 0, // output CIE pointer
            0, 0, 0, 0, // relocated pc begin
            4, 0, 0, 0, // pc range
        ]);
        output[0x160..0x168].copy_from_slice(&0x1c_u64.to_le_bytes());

        let previous = vec![fde_record("input.o", 1, 2, 0, 0x48, 16)];
        let candidate = FdeAddCandidate {
            input_ranges: vec![0..16],
            input_file: "input.o".to_owned(),
            input: "input.o".to_owned(),
            target_section_index: 1,
            eh_frame_section_index: 2,
            input_offset: 16,
            target_section_offset: 0,
            target_output_offset: 0x40,
            fde_data: vec![12, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0],
            pc_begin_range: 8..12,
            cie_input_offset: 0,
            cie_reference_fde_output_offset: 0x48,
        };

        let result = fde_add_patches_for_output(&output, &[candidate], &previous).unwrap();
        let Err(error) = result else {
            panic!("expected .eh_frame addition to be rejected");
        };

        assert!(error.contains("no free .eh_frame space"));
    }

    #[test]
    fn resolve_current_patch_sections_updates_section_size_after_growth() {
        let mut bytes = growable_data_elf();
        bytes[0x44] = 5;
        bytes[0xe0..0xe8].copy_from_slice(&5_u64.to_le_bytes());
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        let resolved = resolve_current_patch_sections(
            &bytes,
            &input_ref,
            [patch_section],
            std::iter::empty(),
            std::iter::empty(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].section_index, 1);
        assert_eq!(resolved[0].input_size, 5);
        assert_eq!(resolved[0].output_size, 8);
    }

    fn growable_data_elf() -> Vec<u8> {
        let mut bytes = vec![0; 0x140];

        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&1_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&0x80_u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&3_u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&2_u16.to_le_bytes());

        bytes[0x40..0x44].copy_from_slice(&[1, 2, 3, 4]);
        bytes[0x48..0x59].copy_from_slice(b"\0.data\0.shstrtab\0");

        let data_header = 0x80 + 64;
        bytes[data_header..data_header + 4].copy_from_slice(&1_u32.to_le_bytes());
        bytes[data_header + 4..data_header + 8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[data_header + 8..data_header + 16].copy_from_slice(&3_u64.to_le_bytes());
        bytes[data_header + 24..data_header + 32].copy_from_slice(&0x40_u64.to_le_bytes());
        bytes[data_header + 32..data_header + 40].copy_from_slice(&4_u64.to_le_bytes());
        bytes[data_header + 48..data_header + 56].copy_from_slice(&8_u64.to_le_bytes());

        let shstrtab_header = 0x80 + 128;
        bytes[shstrtab_header..shstrtab_header + 4].copy_from_slice(&7_u32.to_le_bytes());
        bytes[shstrtab_header + 4..shstrtab_header + 8].copy_from_slice(&3_u32.to_le_bytes());
        bytes[shstrtab_header + 24..shstrtab_header + 32].copy_from_slice(&0x48_u64.to_le_bytes());
        bytes[shstrtab_header + 32..shstrtab_header + 40].copy_from_slice(&17_u64.to_le_bytes());
        bytes[shstrtab_header + 48..shstrtab_header + 56].copy_from_slice(&1_u64.to_le_bytes());

        bytes
    }

    fn rmeta_link_wrapper_elf(metadata: &[u8]) -> Vec<u8> {
        const ELF_HEADER_SIZE: usize = 64;
        const SECTION_HEADER_SIZE: usize = 64;
        const SECTION_COUNT: usize = 3;
        const SECTION_NAMES: &[u8] = b"\0.rmeta-link\0.shstrtab\0";

        let metadata_offset = ELF_HEADER_SIZE;
        let section_names_offset = metadata_offset + metadata.len();
        let section_headers_offset =
            (section_names_offset + SECTION_NAMES.len()).next_multiple_of(8);
        let mut bytes = vec![0; section_headers_offset + SECTION_HEADER_SIZE * SECTION_COUNT];

        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&1_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&(section_headers_offset as u64).to_le_bytes());
        bytes[52..54].copy_from_slice(&(ELF_HEADER_SIZE as u16).to_le_bytes());
        bytes[58..60].copy_from_slice(&(SECTION_HEADER_SIZE as u16).to_le_bytes());
        bytes[60..62].copy_from_slice(&(SECTION_COUNT as u16).to_le_bytes());
        bytes[62..64].copy_from_slice(&2_u16.to_le_bytes());

        bytes[metadata_offset..section_names_offset].copy_from_slice(metadata);
        bytes[section_names_offset..section_names_offset + SECTION_NAMES.len()]
            .copy_from_slice(SECTION_NAMES);

        let metadata_header = section_headers_offset + SECTION_HEADER_SIZE;
        bytes[metadata_header..metadata_header + 4].copy_from_slice(&1_u32.to_le_bytes());
        bytes[metadata_header + 4..metadata_header + 8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[metadata_header + 8..metadata_header + 16]
            .copy_from_slice(&u64::from(object::elf::SHF_EXCLUDE).to_le_bytes());
        bytes[metadata_header + 24..metadata_header + 32]
            .copy_from_slice(&(metadata_offset as u64).to_le_bytes());
        bytes[metadata_header + 32..metadata_header + 40]
            .copy_from_slice(&(metadata.len() as u64).to_le_bytes());
        bytes[metadata_header + 48..metadata_header + 56].copy_from_slice(&1_u64.to_le_bytes());

        let section_names_header = section_headers_offset + SECTION_HEADER_SIZE * 2;
        bytes[section_names_header..section_names_header + 4]
            .copy_from_slice(&13_u32.to_le_bytes());
        bytes[section_names_header + 4..section_names_header + 8]
            .copy_from_slice(&3_u32.to_le_bytes());
        bytes[section_names_header + 24..section_names_header + 32]
            .copy_from_slice(&(section_names_offset as u64).to_le_bytes());
        bytes[section_names_header + 32..section_names_header + 40]
            .copy_from_slice(&(SECTION_NAMES.len() as u64).to_le_bytes());
        bytes[section_names_header + 48..section_names_header + 56]
            .copy_from_slice(&1_u64.to_le_bytes());

        bytes
    }

    fn relocated_data_elf() -> Vec<u8> {
        let mut bytes = vec![0; 0x220];

        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&1_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&0x100_u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&4_u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&3_u16.to_le_bytes());

        bytes[0x40..0x48].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        bytes[0x80..0x88].copy_from_slice(&4_u64.to_le_bytes());
        bytes[0x88..0x90].copy_from_slice(&1_u64.to_le_bytes());
        bytes[0x90..0x98].copy_from_slice(&2_i64.to_le_bytes());
        bytes[0xa0..0xbc].copy_from_slice(b"\0.data\0.rela.data\0.shstrtab\0");

        let data_header = 0x100 + 64;
        bytes[data_header..data_header + 4].copy_from_slice(&1_u32.to_le_bytes());
        bytes[data_header + 4..data_header + 8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[data_header + 8..data_header + 16].copy_from_slice(&3_u64.to_le_bytes());
        bytes[data_header + 24..data_header + 32].copy_from_slice(&0x40_u64.to_le_bytes());
        bytes[data_header + 32..data_header + 40].copy_from_slice(&8_u64.to_le_bytes());
        bytes[data_header + 48..data_header + 56].copy_from_slice(&8_u64.to_le_bytes());

        let rela_header = 0x100 + 128;
        bytes[rela_header..rela_header + 4].copy_from_slice(&7_u32.to_le_bytes());
        bytes[rela_header + 4..rela_header + 8].copy_from_slice(&4_u32.to_le_bytes());
        bytes[rela_header + 24..rela_header + 32].copy_from_slice(&0x80_u64.to_le_bytes());
        bytes[rela_header + 32..rela_header + 40].copy_from_slice(&24_u64.to_le_bytes());
        bytes[rela_header + 40..rela_header + 44].copy_from_slice(&0_u32.to_le_bytes());
        bytes[rela_header + 44..rela_header + 48].copy_from_slice(&1_u32.to_le_bytes());
        bytes[rela_header + 48..rela_header + 56].copy_from_slice(&8_u64.to_le_bytes());
        bytes[rela_header + 56..rela_header + 64].copy_from_slice(&24_u64.to_le_bytes());

        let shstrtab_header = 0x100 + 192;
        bytes[shstrtab_header..shstrtab_header + 4].copy_from_slice(&18_u32.to_le_bytes());
        bytes[shstrtab_header + 4..shstrtab_header + 8].copy_from_slice(&3_u32.to_le_bytes());
        bytes[shstrtab_header + 24..shstrtab_header + 32].copy_from_slice(&0xa0_u64.to_le_bytes());
        bytes[shstrtab_header + 32..shstrtab_header + 40].copy_from_slice(&28_u64.to_le_bytes());
        bytes[shstrtab_header + 48..shstrtab_header + 56].copy_from_slice(&1_u64.to_le_bytes());

        bytes
    }

    fn relocated_text_elf(relocation_kind: u32) -> Vec<u8> {
        let mut bytes = relocated_data_elf();
        bytes[0x88..0x90].copy_from_slice(&u64::from(relocation_kind).to_le_bytes());
        bytes[0xa1..0xa6].copy_from_slice(b".text");
        bytes[0xa7..0xb1].copy_from_slice(b".rela.text");
        bytes
    }

    fn relocated_data_elf_with_added_relocation() -> Vec<u8> {
        let mut bytes = relocated_data_elf();
        let shstrtab = bytes[0xa0..0xbc].to_vec();
        bytes[0xa0..0xc0].fill(0);
        bytes[0xc0..0xdc].copy_from_slice(&shstrtab);

        bytes[0x98..0xa0].copy_from_slice(&6_u64.to_le_bytes());
        bytes[0xa0..0xa8].copy_from_slice(&1_u64.to_le_bytes());
        bytes[0xa8..0xb0].copy_from_slice(&9_i64.to_le_bytes());

        let rela_header = 0x100 + 128;
        bytes[rela_header + 32..rela_header + 40].copy_from_slice(&48_u64.to_le_bytes());

        let shstrtab_header = 0x100 + 192;
        bytes[shstrtab_header + 24..shstrtab_header + 32].copy_from_slice(&0xc0_u64.to_le_bytes());

        bytes
    }

    fn eh_frame_hdr_output_elf(entries: &[(i32, u64)]) -> Vec<u8> {
        eh_frame_hdr_output_elf_with_capacity(entries, entries.len())
    }

    fn eh_frame_hdr_output_elf_with_capacity(
        entries: &[(i32, u64)],
        entry_capacity: usize,
    ) -> Vec<u8> {
        assert!(entry_capacity >= entries.len());
        let mut bytes = vec![0; 0x220];
        let shstrtab = b"\0.eh_frame\0.eh_frame_hdr\0.shstrtab\0";
        let shstrtab_offset = 0xd0;
        let eh_frame_offset = 0x40_u64;
        let eh_frame_size = 0x40_u64;
        let eh_frame_address = 0x400040_u64;
        let eh_frame_hdr_offset = 0x80_u64;
        let eh_frame_hdr_address = 0x400080_u64;
        let eh_frame_hdr_size = std::mem::size_of::<crate::elf::EhFrameHdr>() + entry_capacity * 8;
        bytes[shstrtab_offset..shstrtab_offset + shstrtab.len()].copy_from_slice(shstrtab);

        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&object::elf::ET_EXEC.to_le_bytes());
        bytes[18..20].copy_from_slice(&object::elf::EM_X86_64.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&0x100_u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&4_u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&3_u16.to_le_bytes());

        let entry_start =
            eh_frame_hdr_offset as usize + std::mem::size_of::<crate::elf::EhFrameHdr>();
        bytes[eh_frame_hdr_offset as usize + 8..eh_frame_hdr_offset as usize + 12]
            .copy_from_slice(&(entries.len() as u32).to_le_bytes());
        for (index, (frame_ptr, fde_output_offset)) in entries.iter().copied().enumerate() {
            let offset = entry_start + index * 8;
            let fde_offset_in_section = fde_output_offset - eh_frame_offset;
            let frame_info_ptr = i32::try_from(
                i128::from(eh_frame_address + fde_offset_in_section)
                    - i128::from(eh_frame_hdr_address),
            )
            .unwrap();
            bytes[offset..offset + 4].copy_from_slice(&frame_ptr.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&frame_info_ptr.to_le_bytes());
        }

        let name_offset = |name: &[u8]| -> u32 {
            shstrtab
                .windows(name.len())
                .position(|window| window == name)
                .unwrap() as u32
        };
        let write_section = |bytes: &mut [u8],
                             index: usize,
                             name: &[u8],
                             ty: u32,
                             flags: u64,
                             address: u64,
                             offset: u64,
                             size: u64,
                             align: u64| {
            let header = 0x100 + index * 64;
            bytes[header..header + 4].copy_from_slice(&name_offset(name).to_le_bytes());
            bytes[header + 4..header + 8].copy_from_slice(&ty.to_le_bytes());
            bytes[header + 8..header + 16].copy_from_slice(&flags.to_le_bytes());
            bytes[header + 16..header + 24].copy_from_slice(&address.to_le_bytes());
            bytes[header + 24..header + 32].copy_from_slice(&offset.to_le_bytes());
            bytes[header + 32..header + 40].copy_from_slice(&size.to_le_bytes());
            bytes[header + 48..header + 56].copy_from_slice(&align.to_le_bytes());
        };
        write_section(
            &mut bytes,
            1,
            b".eh_frame",
            object::elf::SHT_PROGBITS,
            u64::from(object::elf::SHF_ALLOC),
            eh_frame_address,
            eh_frame_offset,
            eh_frame_size,
            8,
        );
        write_section(
            &mut bytes,
            2,
            b".eh_frame_hdr",
            object::elf::SHT_PROGBITS,
            u64::from(object::elf::SHF_ALLOC),
            eh_frame_hdr_address,
            eh_frame_hdr_offset,
            eh_frame_hdr_size as u64,
            4,
        );
        write_section(
            &mut bytes,
            3,
            b".shstrtab",
            object::elf::SHT_STRTAB,
            0,
            0,
            shstrtab_offset as u64,
            shstrtab.len() as u64,
            1,
        );

        bytes
    }

    fn eh_frame_relocation_elf(relocation_offset: u64, addend: i64) -> Vec<u8> {
        let mut bytes = vec![0; 0x288];
        let shstrtab = b"\0.text\0.eh_frame\0.rela.eh_frame\0.symtab\0.strtab\0.shstrtab\0";
        let shstrtab_offset = 0xa8;
        bytes[shstrtab_offset..shstrtab_offset + shstrtab.len()].copy_from_slice(shstrtab);

        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&object::elf::ET_REL.to_le_bytes());
        bytes[18..20].copy_from_slice(&object::elf::EM_X86_64.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&0xc8_u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&7_u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&6_u16.to_le_bytes());

        bytes[0x40..0x44].copy_from_slice(&[0xc3, 0, 0, 0]);
        bytes[0x48..0x58].copy_from_slice(&[
            12, 0, 0, 0, // length
            4, 0, 0, 0, // CIE pointer
            0, 0, 0, 0, // relocated pc begin
            4, 0, 0, 0, // pc range
        ]);
        bytes[0x58..0x60].copy_from_slice(&relocation_offset.to_le_bytes());
        let relocation_info = (1_u64 << 32) | u64::from(object::elf::R_X86_64_PC32);
        bytes[0x60..0x68].copy_from_slice(&relocation_info.to_le_bytes());
        bytes[0x68..0x70].copy_from_slice(&addend.to_le_bytes());

        let symbol_offset = 0x70 + 24;
        bytes[symbol_offset..symbol_offset + 4].copy_from_slice(&1_u32.to_le_bytes());
        bytes[symbol_offset + 4] = object::elf::STT_FUNC;
        bytes[symbol_offset + 6..symbol_offset + 8].copy_from_slice(&1_u16.to_le_bytes());
        bytes[symbol_offset + 16..symbol_offset + 24].copy_from_slice(&4_u64.to_le_bytes());
        bytes[0xa0..0xa6].copy_from_slice(b"\0func\0");

        let name_offset = |name: &[u8]| -> u32 {
            shstrtab
                .windows(name.len())
                .position(|window| window == name)
                .unwrap() as u32
        };
        let write_section = |bytes: &mut [u8],
                             index: usize,
                             name: &[u8],
                             ty: u32,
                             flags: u64,
                             offset: u64,
                             size: u64,
                             link: u32,
                             info: u32,
                             align: u64,
                             entsize: u64| {
            let header = 0xc8 + index * 64;
            bytes[header..header + 4].copy_from_slice(&name_offset(name).to_le_bytes());
            bytes[header + 4..header + 8].copy_from_slice(&ty.to_le_bytes());
            bytes[header + 8..header + 16].copy_from_slice(&flags.to_le_bytes());
            bytes[header + 24..header + 32].copy_from_slice(&offset.to_le_bytes());
            bytes[header + 32..header + 40].copy_from_slice(&size.to_le_bytes());
            bytes[header + 40..header + 44].copy_from_slice(&link.to_le_bytes());
            bytes[header + 44..header + 48].copy_from_slice(&info.to_le_bytes());
            bytes[header + 48..header + 56].copy_from_slice(&align.to_le_bytes());
            bytes[header + 56..header + 64].copy_from_slice(&entsize.to_le_bytes());
        };
        write_section(
            &mut bytes,
            1,
            b".text",
            object::elf::SHT_PROGBITS,
            u64::from(object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR),
            0x40,
            4,
            0,
            0,
            4,
            0,
        );
        write_section(
            &mut bytes,
            2,
            b".eh_frame",
            object::elf::SHT_PROGBITS,
            u64::from(object::elf::SHF_ALLOC),
            0x48,
            16,
            0,
            0,
            8,
            0,
        );
        write_section(
            &mut bytes,
            3,
            b".rela.eh_frame",
            object::elf::SHT_RELA,
            0,
            0x58,
            24,
            4,
            2,
            8,
            crate::elf::RELA_ENTRY_SIZE,
        );
        write_section(
            &mut bytes,
            4,
            b".symtab",
            object::elf::SHT_SYMTAB,
            0,
            0x70,
            48,
            5,
            1,
            8,
            24,
        );
        write_section(
            &mut bytes,
            5,
            b".strtab",
            object::elf::SHT_STRTAB,
            0,
            0xa0,
            6,
            0,
            0,
            1,
            0,
        );
        write_section(
            &mut bytes,
            6,
            b".shstrtab",
            object::elf::SHT_STRTAB,
            0,
            shstrtab_offset as u64,
            shstrtab.len() as u64,
            0,
            0,
            1,
            0,
        );

        bytes
    }

    fn eh_frame_two_fdes_same_section_elf() -> Vec<u8> {
        let mut bytes = vec![0; 0x300];
        let shstrtab = b"\0.text\0.eh_frame\0.rela.eh_frame\0.symtab\0.strtab\0.shstrtab\0";
        let shstrtab_offset = 0xd0;
        bytes[shstrtab_offset..shstrtab_offset + shstrtab.len()].copy_from_slice(shstrtab);

        bytes[0..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&object::elf::ET_REL.to_le_bytes());
        bytes[18..20].copy_from_slice(&object::elf::EM_X86_64.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[40..48].copy_from_slice(&0x130_u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64_u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&7_u16.to_le_bytes());
        bytes[62..64].copy_from_slice(&6_u16.to_le_bytes());

        bytes[0x40..0x48].copy_from_slice(&[0xc3, 0, 0, 0, 0xc3, 0, 0, 0]);
        bytes[0x48..0x58].copy_from_slice(&[
            12, 0, 0, 0, // length
            4, 0, 0, 0, // CIE pointer
            0, 0, 0, 0, // relocated pc begin
            4, 0, 0, 0, // pc range
        ]);
        bytes[0x58..0x68].copy_from_slice(&[
            12, 0, 0, 0, // length
            4, 0, 0, 0, // CIE pointer
            0, 0, 0, 0, // relocated pc begin
            4, 0, 0, 0, // pc range
        ]);
        let relocation_info = (1_u64 << 32) | u64::from(object::elf::R_X86_64_PC32);
        for (rela_offset, fde_pc_begin) in [(0x68, 8_u64), (0x80, 24_u64)] {
            bytes[rela_offset..rela_offset + 8].copy_from_slice(&fde_pc_begin.to_le_bytes());
            bytes[rela_offset + 8..rela_offset + 16]
                .copy_from_slice(&relocation_info.to_le_bytes());
            bytes[rela_offset + 16..rela_offset + 24].copy_from_slice(&(-4_i64).to_le_bytes());
        }

        let symbol_offset = 0x98 + 24;
        bytes[symbol_offset..symbol_offset + 4].copy_from_slice(&1_u32.to_le_bytes());
        bytes[symbol_offset + 4] = object::elf::STT_FUNC;
        bytes[symbol_offset + 6..symbol_offset + 8].copy_from_slice(&1_u16.to_le_bytes());
        bytes[symbol_offset + 16..symbol_offset + 24].copy_from_slice(&8_u64.to_le_bytes());
        bytes[0xc8..0xce].copy_from_slice(b"\0func\0");

        let name_offset = |name: &[u8]| -> u32 {
            shstrtab
                .windows(name.len())
                .position(|window| window == name)
                .unwrap() as u32
        };
        let write_section = |bytes: &mut [u8],
                             index: usize,
                             name: &[u8],
                             ty: u32,
                             flags: u64,
                             offset: u64,
                             size: u64,
                             link: u32,
                             info: u32,
                             align: u64,
                             entsize: u64| {
            let header = 0x130 + index * 64;
            bytes[header..header + 4].copy_from_slice(&name_offset(name).to_le_bytes());
            bytes[header + 4..header + 8].copy_from_slice(&ty.to_le_bytes());
            bytes[header + 8..header + 16].copy_from_slice(&flags.to_le_bytes());
            bytes[header + 24..header + 32].copy_from_slice(&offset.to_le_bytes());
            bytes[header + 32..header + 40].copy_from_slice(&size.to_le_bytes());
            bytes[header + 40..header + 44].copy_from_slice(&link.to_le_bytes());
            bytes[header + 44..header + 48].copy_from_slice(&info.to_le_bytes());
            bytes[header + 48..header + 56].copy_from_slice(&align.to_le_bytes());
            bytes[header + 56..header + 64].copy_from_slice(&entsize.to_le_bytes());
        };
        write_section(
            &mut bytes,
            1,
            b".text",
            object::elf::SHT_PROGBITS,
            u64::from(object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR),
            0x40,
            8,
            0,
            0,
            4,
            0,
        );
        write_section(
            &mut bytes,
            2,
            b".eh_frame",
            object::elf::SHT_PROGBITS,
            u64::from(object::elf::SHF_ALLOC),
            0x48,
            32,
            0,
            0,
            8,
            0,
        );
        write_section(
            &mut bytes,
            3,
            b".rela.eh_frame",
            object::elf::SHT_RELA,
            0,
            0x68,
            48,
            4,
            2,
            8,
            crate::elf::RELA_ENTRY_SIZE,
        );
        write_section(
            &mut bytes,
            4,
            b".symtab",
            object::elf::SHT_SYMTAB,
            0,
            0x98,
            48,
            5,
            1,
            8,
            24,
        );
        write_section(
            &mut bytes,
            5,
            b".strtab",
            object::elf::SHT_STRTAB,
            0,
            0xc8,
            6,
            0,
            0,
            1,
            0,
        );
        write_section(
            &mut bytes,
            6,
            b".shstrtab",
            object::elf::SHT_STRTAB,
            0,
            shstrtab_offset as u64,
            shstrtab.len() as u64,
            0,
            0,
            1,
            0,
        );

        bytes
    }

    fn eh_frame_relocation_elf_with_shifted_eh_frame_index(
        relocation_offset: u64,
        addend: i64,
    ) -> Vec<u8> {
        let mut bytes = eh_frame_relocation_elf(relocation_offset, addend);
        let eh_frame_header = 0xc8 + 2 * 64;
        let rela_header = 0xc8 + 3 * 64;
        let mut eh_frame = [0; 64];
        let mut rela = [0; 64];
        eh_frame.copy_from_slice(&bytes[eh_frame_header..eh_frame_header + 64]);
        rela.copy_from_slice(&bytes[rela_header..rela_header + 64]);
        bytes[eh_frame_header..eh_frame_header + 64].copy_from_slice(&rela);
        bytes[rela_header..rela_header + 64].copy_from_slice(&eh_frame);
        bytes[eh_frame_header + 44..eh_frame_header + 48].copy_from_slice(&3_u32.to_le_bytes());
        bytes
    }

    #[test]
    fn patch_sections_for_input_resolves_unique_section_names() {
        let Ok(current_exe) = std::env::current_exe() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&current_exe) else {
            return;
        };
        let Ok(object) = object::File::parse(&*bytes) else {
            return;
        };
        let mut selected = None;
        for section in object.sections() {
            let Some(section_name) = patch_section_name_for_matching(&section) else {
                continue;
            };
            let Some((_, size)) = section.file_range() else {
                continue;
            };
            let Ok(data) = section.data() else {
                continue;
            };
            if size == 0
                || section_direct_patch_preserve_ranges(&object, &section, data, None, None)
                    .is_none()
                || object
                    .sections()
                    .filter(|candidate| {
                        patch_section_name_for_matching(candidate).as_deref()
                            == Some(section_name.as_str())
                    })
                    .count()
                    != 1
            {
                continue;
            }
            selected = Some((section_name, size));
            break;
        }
        let Some((section_name, size)) = selected else {
            return;
        };
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: u32::MAX,
            section_name: Some(section_name),
            input_size: size,
            output_offset: 64,
            output_size: size,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(
            patch_ranges(&bytes, &input_ref, [patch_section.clone()])
                .unwrap()
                .is_some()
        );
        assert!(
            patch_sections_for_input(&bytes, &input_ref, [patch_section])
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn generated_local_section_names_are_not_stable_for_patch_matching() {
        assert!(section_name_is_stable_for_patch_matching(".text.symbol"));
        assert!(section_name_is_stable_for_patch_matching(".data.my_static"));
        assert!(!section_name_is_stable_for_patch_matching(""));
        assert!(!section_name_is_stable_for_patch_matching(
            ".rodata..L__unnamed_75"
        ));
        assert!(!section_name_is_stable_for_patch_matching(
            ".data.rel.ro..L__unnamed_12"
        ));
    }

    #[test]
    fn patch_input_range_decodes_archive_member_offsets() {
        let input_file = hex::encode("libarchive.a");
        let input_ref = hex::encode("libarchive.a\0member.o\012:34");

        assert_eq!(
            patch_input_range(&input_file, &input_ref).unwrap(),
            Some(12..34)
        );
    }

    #[test]
    fn patch_input_range_uses_whole_file_for_direct_inputs() {
        let input_file = hex::encode("main.o");

        assert_eq!(patch_input_range(&input_file, &input_file).unwrap(), None);
    }

    #[test]
    fn patch_input_bytes_finds_archive_member_by_identifier() {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"padding.o".to_vec(), 4),
                b"xxxx".as_slice(),
            )
            .unwrap();
        builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 11),
                b"member-data".as_slice(),
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let input_file = hex::encode("libarchive.a");
        let stale_ref = hex::encode("libarchive.a\0member.o\01:5");

        let member = patch_input_bytes(&archive, &input_file, &stale_ref)
            .unwrap()
            .unwrap();

        assert_eq!(member.bytes, b"member-data");
        assert_ne!(member.file_offset, 1);
    }

    #[test]
    fn patch_input_bytes_rejects_ambiguous_archive_member_names() {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 5),
                b"first".as_slice(),
            )
            .unwrap();
        builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 6),
                b"second".as_slice(),
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let input_file = hex::encode("libarchive.a");
        let input_ref = hex::encode("libarchive.a\0member.o\01:5");

        assert!(
            patch_input_bytes(&archive, &input_file, &input_ref)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn current_recorded_range_selects_an_ambiguous_archive_member() {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 5),
                b"first".as_slice(),
            )
            .unwrap();
        builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 6),
                b"second".as_slice(),
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let start = archive
            .windows(b"second".len())
            .rposition(|window| window == b"second")
            .unwrap();
        let input_file = hex::encode("libarchive.a");
        let input_ref = hex::encode(format!(
            "libarchive.a\0member.o\0{start}:{}",
            start + b"second".len()
        ));

        let member = patch_input_bytes_with_lookup(
            &archive,
            &input_file,
            &input_ref,
            PatchInputLookup::CurrentRecordedRange,
        )
        .unwrap()
        .unwrap();

        assert_eq!(member.bytes, b"second");
        assert_eq!(member.file_offset, start);
    }

    #[test]
    fn patch_input_bytes_matches_rustc_member_across_invocation_names() {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"crate-hash.cgu.new.rcgu.o".to_vec(), 11),
                b"member-data".as_slice(),
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let input_file = hex::encode("libarchive.rlib");
        let stale_ref = hex::encode("libarchive.rlib\0crate-hash.cgu.old.rcgu.o\01:5");

        let member = patch_input_bytes(&archive, &input_file, &stale_ref)
            .unwrap()
            .unwrap();

        assert_eq!(member.bytes, b"member-data");
    }

    #[test]
    fn patch_input_resolver_matches_rustc_member_across_invocation_names() {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"crate-hash.cgu.new.rcgu.o".to_vec(), 11),
                b"member-data".as_slice(),
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let input_file = hex::encode("libarchive.rlib");
        let stale_ref = hex::encode("libarchive.rlib\0crate-hash.cgu.old.rcgu.o\01:5");
        let resolver = PatchInputResolver::new(&archive, true).unwrap();

        let member = resolver
            .resolve(
                &input_file,
                &stale_ref,
                PatchInputLookup::MatchArchiveMember,
            )
            .unwrap()
            .unwrap();

        assert_eq!(member.bytes, b"member-data");

        let raw_resolver = PatchInputResolver::new(&archive, false).unwrap();
        assert_ne!(
            raw_resolver
                .resolve(
                    &input_file,
                    &stale_ref,
                    PatchInputLookup::MatchArchiveMember,
                )
                .unwrap()
                .unwrap()
                .bytes,
            b"member-data",
        );
    }

    #[test]
    fn patch_input_resolver_rejects_ambiguous_archive_member_names() {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 5),
                b"first".as_slice(),
            )
            .unwrap();
        builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 6),
                b"second".as_slice(),
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let input_file = hex::encode("libarchive.a");
        let input_ref = hex::encode("libarchive.a\0member.o\01:5");
        let resolver = PatchInputResolver::new(&archive, true).unwrap();

        assert!(
            resolver
                .resolve(
                    &input_file,
                    &input_ref,
                    PatchInputLookup::MatchArchiveMember
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn archive_patch_fingerprint_ignores_rust_metadata_and_invocation_names() {
        let archive = |invocation: &[u8], metadata: &[u8], link_metadata: &[u8], object: &[u8]| {
            let mut builder = ar::Builder::new(Vec::new());
            builder
                .append(
                    &ar::Header::new(b"lib.rmeta".to_vec(), metadata.len() as u64),
                    metadata,
                )
                .unwrap();
            builder
                .append(
                    &ar::Header::new(b"lib.rmeta-link".to_vec(), link_metadata.len() as u64),
                    link_metadata,
                )
                .unwrap();
            builder
                .append(
                    &ar::Header::new(
                        [b"crate-hash.cgu.".as_slice(), invocation, b".rcgu.o"].concat(),
                        object.len() as u64,
                    ),
                    object,
                )
                .unwrap();
            builder.into_inner().unwrap()
        };
        let previous = archive(
            b"old",
            b"previous metadata",
            b"crate-hash.cgu.old.rcgu.o",
            b"object data",
        );
        let renamed = archive(
            b"new",
            b"new metadata",
            b"crate-hash.cgu.new.rcgu.o",
            b"object data",
        );
        let changed = archive(
            b"new",
            b"new metadata",
            b"crate-hash.cgu.new.rcgu.o",
            b"changed object data",
        );

        assert_eq!(
            archive_patch_fingerprint(&previous, &[]).unwrap(),
            archive_patch_fingerprint(&renamed, &[]).unwrap()
        );
        assert_ne!(
            archive_patch_fingerprint(&previous, &[]).unwrap(),
            archive_patch_fingerprint(&changed, &[]).unwrap()
        );
        let fingerprint = archive_patch_fingerprint(&previous, &[]).unwrap().unwrap();
        assert!(fingerprint.starts_with(PARALLEL_ARCHIVE_PATCH_FINGERPRINT_PREFIX));
        assert_ne!(
            patch_fingerprint_from_ranges(&previous, vec![0..1], std::iter::empty(), false)
                .unwrap(),
            patch_fingerprint_from_ranges(&renamed, vec![0..1], std::iter::empty(), false).unwrap()
        );
    }

    #[test]
    fn archive_patch_fingerprint_preserves_native_member_order() {
        let archive = |members: &[(&[u8], &[u8])]| {
            let mut builder = ar::Builder::new(Vec::new());
            for (identifier, bytes) in members {
                builder
                    .append(
                        &ar::Header::new(identifier.to_vec(), bytes.len() as u64),
                        *bytes,
                    )
                    .unwrap();
            }
            builder.into_inner().unwrap()
        };
        let previous = archive(&[(b"first.o", b"first"), (b"second.o", b"second")]);
        let reordered = archive(&[(b"second.o", b"second"), (b"first.o", b"first")]);

        assert_ne!(
            archive_patch_fingerprint(&previous, &[]).unwrap(),
            archive_patch_fingerprint(&reordered, &[]).unwrap()
        );
    }

    #[test]
    fn macho_object_patch_fingerprint_ignores_serialization_layout_churn() {
        let previous = test_macho_object(b"\x01\x02\x03\x04", b"\x05\x06\x07\x08", 0);
        let grown = test_macho_object(b"\x01\x02\x03\x04\x09\x0a\x0b\x0c", b"\x05\x06\x07\x08", 0);
        let changed_data =
            test_macho_object(b"\x01\x02\x03\x04\x09\x0a\x0b\x0c", b"\x15\x06\x07\x08", 0);
        let moved_data_symbol =
            test_macho_object(b"\x01\x02\x03\x04\x09\x0a\x0b\x0c", b"\x05\x06\x07\x08", 1);
        let mut different_cpu_subtype = previous.clone();
        different_cpu_subtype[8] ^= 1;

        let previous_text = test_macho_section_range(&previous, "__text");
        let previous_data = test_macho_section_range(&previous, "__data");
        let grown_text = test_macho_section_range(&grown, "__text");
        let changed_data_text = test_macho_section_range(&changed_data, "__text");
        let moved_data_symbol_text = test_macho_section_range(&moved_data_symbol, "__text");
        let previous_data_symbol =
            macho_symbol_value_field_range(&previous, object::SymbolIndex(1)).unwrap();
        let moved_data_symbol_value =
            macho_symbol_value_field_range(&moved_data_symbol, object::SymbolIndex(1)).unwrap();

        assert_eq!(
            macho_object_patch_fingerprint(&previous, 0, &[previous_text.clone()]).unwrap(),
            macho_object_patch_fingerprint(&grown, 0, &[grown_text]).unwrap()
        );
        assert_ne!(
            macho_object_patch_fingerprint(&previous, 0, &[previous_text.clone()]).unwrap(),
            macho_object_patch_fingerprint(&changed_data, 0, &[changed_data_text]).unwrap()
        );
        assert_ne!(
            macho_object_patch_fingerprint(&previous, 0, &[previous_text.clone()]).unwrap(),
            macho_object_patch_fingerprint(
                &moved_data_symbol,
                0,
                &[moved_data_symbol_text.clone()],
            )
            .unwrap()
        );
        assert_eq!(
            macho_object_patch_fingerprint(
                &previous,
                0,
                &[previous_text.clone(), previous_data_symbol]
            )
            .unwrap(),
            macho_object_patch_fingerprint(
                &moved_data_symbol,
                0,
                &[moved_data_symbol_text, moved_data_symbol_value]
            )
            .unwrap()
        );
        assert_ne!(
            macho_object_patch_fingerprint(&previous, 0, &[previous_text.clone()]).unwrap(),
            macho_object_patch_fingerprint(&different_cpu_subtype, 0, &[previous_text]).unwrap()
        );
        assert!(
            macho_object_patch_fingerprint(&previous, 0, &[previous_data]).is_none(),
            "semantic normalization is scoped to text patches"
        );
    }

    #[test]
    fn archive_patch_fingerprint_allows_a_masked_macho_member_to_grow() {
        let previous_object = test_macho_object(b"\x01\x02\x03\x04", b"\x05\x06\x07\x08", 0);
        let grown_object =
            test_macho_object(b"\x01\x02\x03\x04\x09\x0a\x0b\x0c", b"\x05\x06\x07\x08", 0);
        let changed_object =
            test_macho_object(b"\x01\x02\x03\x04\x09\x0a\x0b\x0c", b"\x15\x06\x07\x08", 0);
        let (previous, previous_text) = test_macho_archive(&previous_object);
        let (grown, grown_text) = test_macho_archive(&grown_object);
        let (changed, changed_text) = test_macho_archive(&changed_object);

        assert_eq!(
            archive_patch_fingerprint(&previous, &[previous_text]).unwrap(),
            archive_patch_fingerprint(&grown, &[grown_text]).unwrap()
        );
        assert_ne!(
            archive_patch_fingerprint(&previous, &[test_macho_archive(&previous_object).1])
                .unwrap(),
            archive_patch_fingerprint(&changed, &[changed_text]).unwrap()
        );
    }

    fn test_macho_archive(object: &[u8]) -> (Vec<u8>, std::ops::Range<usize>) {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"crate-hash.cgu.old.rcgu.o".to_vec(), object.len() as u64),
                object,
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();
        let mut entries = ArchiveIterator::from_archive_bytes(&archive).unwrap();
        let ArchiveEntry::Regular(member) = entries.next().unwrap().unwrap() else {
            panic!("expected regular archive member");
        };
        let range = test_macho_section_range(member.entry_data, "__text");
        let data_offset = member.data_offset;
        (archive, data_offset + range.start..data_offset + range.end)
    }

    fn test_macho_section_range(bytes: &[u8], name: &str) -> std::ops::Range<usize> {
        let file = object::File::parse(bytes).unwrap();
        let section = file.section_by_name(name).unwrap();
        let (offset, size) = section.file_range().unwrap();
        offset as usize..(offset + size) as usize
    }

    #[test]
    fn macho_symbol_value_ranges_follow_raw_nlist_indices() {
        let mut bytes = test_macho_object(b"\x01\x02\x03\x04", b"\x05\x06\x07\x08", 1);
        let first = macho_symbol_value_field_range(&bytes, object::SymbolIndex(0)).unwrap();
        let second = macho_symbol_value_field_range(&bytes, object::SymbolIndex(1)).unwrap();

        assert_eq!(&bytes[first.clone()], &0_u64.to_le_bytes());
        assert_eq!(&bytes[second.clone()], &5_u64.to_le_bytes());

        let first_entry = first.start - 8;
        bytes[first_entry + 4] = object::macho::N_STAB;
        let file = object::File::parse(bytes.as_slice()).unwrap();
        let symbols = file.symbols().collect::<Vec<_>>();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].index(), object::SymbolIndex(1));
        assert_eq!(
            macho_symbol_value_field_range(&bytes, symbols[0].index()),
            Some(second)
        );
        assert!(macho_symbol_value_field_range(&bytes, object::SymbolIndex(2)).is_none());
    }

    #[test]
    fn output_symbol_value_patches_macho_nlist64() {
        let output = test_macho_object(b"\x01\x02\x03\x04", b"\x05\x06\x07\x08", 0);
        let value_range = macho_symbol_value_field_range(&output, object::SymbolIndex(1)).unwrap();

        let patches = output_symbol_value_patches(
            &output,
            &[RelocationTargetSymbolPatch {
                target_name: hex::encode(b"_data"),
                previous_target_value: 4,
                target_value: 8,
                allow_missing: true,
            }],
        )
        .unwrap()
        .unwrap();

        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].output_offset, value_range.start as u64);
        assert_eq!(patches[0].data, 8_u64.to_le_bytes());
    }

    fn test_macho_object(text: &[u8], data: &[u8], data_symbol_offset: u64) -> Vec<u8> {
        const HEADER_SIZE: usize = 32;
        const SEGMENT_COMMAND_SIZE: usize = 72;
        const SECTION_SIZE: usize = 80;
        const SYMTAB_COMMAND_SIZE: usize = 24;
        const COMMANDS_SIZE: usize = SEGMENT_COMMAND_SIZE + 2 * SECTION_SIZE + SYMTAB_COMMAND_SIZE;
        const DATA_OFFSET: usize = HEADER_SIZE + COMMANDS_SIZE;
        const NLIST_64_SIZE: usize = 16;

        let data_offset = DATA_OFFSET + text.len();
        let symoff = (data_offset + data.len()).next_multiple_of(8);
        let stroff = symoff + 2 * NLIST_64_SIZE;
        let strings = b"\0_text\0_data\0";
        let mut bytes = Vec::new();

        push_u32(&mut bytes, object::macho::MH_MAGIC_64);
        push_u32(&mut bytes, object::macho::CPU_TYPE_ARM64 as u32);
        push_u32(&mut bytes, object::macho::CPU_SUBTYPE_ARM64_ALL as u32);
        push_u32(&mut bytes, object::macho::MH_OBJECT);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, COMMANDS_SIZE as u32);
        push_u32(&mut bytes, object::macho::MH_SUBSECTIONS_VIA_SYMBOLS);
        push_u32(&mut bytes, 0);

        push_u32(&mut bytes, object::macho::LC_SEGMENT_64);
        push_u32(&mut bytes, (SEGMENT_COMMAND_SIZE + 2 * SECTION_SIZE) as u32);
        push_fixed_name(&mut bytes, b"");
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, (text.len() + data.len()) as u64);
        push_u64(&mut bytes, DATA_OFFSET as u64);
        push_u64(&mut bytes, (text.len() + data.len()) as u64);
        push_u32(&mut bytes, 7);
        push_u32(&mut bytes, 7);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, 0);

        push_macho_section(
            &mut bytes,
            b"__text",
            b"__TEXT",
            0,
            text.len(),
            DATA_OFFSET,
            object::macho::S_REGULAR
                | object::macho::S_ATTR_PURE_INSTRUCTIONS
                | object::macho::S_ATTR_SOME_INSTRUCTIONS,
        );
        push_macho_section(
            &mut bytes,
            b"__data",
            b"__DATA",
            text.len() as u64,
            data.len(),
            data_offset,
            object::macho::S_REGULAR,
        );

        push_u32(&mut bytes, object::macho::LC_SYMTAB);
        push_u32(&mut bytes, SYMTAB_COMMAND_SIZE as u32);
        push_u32(&mut bytes, symoff as u32);
        push_u32(&mut bytes, 2);
        push_u32(&mut bytes, stroff as u32);
        push_u32(&mut bytes, strings.len() as u32);

        assert_eq!(bytes.len(), DATA_OFFSET);
        bytes.extend_from_slice(text);
        bytes.extend_from_slice(data);
        bytes.resize(symoff, 0);
        push_macho_symbol(&mut bytes, 1, 1, 0);
        push_macho_symbol(&mut bytes, 7, 2, text.len() as u64 + data_symbol_offset);
        bytes.extend_from_slice(strings);
        bytes
    }

    fn push_macho_section(
        bytes: &mut Vec<u8>,
        name: &[u8],
        segment: &[u8],
        address: u64,
        size: usize,
        offset: usize,
        flags: u32,
    ) {
        push_fixed_name(bytes, name);
        push_fixed_name(bytes, segment);
        push_u64(bytes, address);
        push_u64(bytes, size as u64);
        push_u32(bytes, offset as u32);
        push_u32(bytes, 2);
        push_u32(bytes, 0);
        push_u32(bytes, 0);
        push_u32(bytes, flags);
        push_u32(bytes, 0);
        push_u32(bytes, 0);
        push_u32(bytes, 0);
    }

    fn push_macho_symbol(bytes: &mut Vec<u8>, name_offset: u32, section: u8, value: u64) {
        push_u32(bytes, name_offset);
        bytes.push(object::macho::N_SECT | object::macho::N_EXT);
        bytes.push(section);
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        push_u64(bytes, value);
    }

    fn push_fixed_name(bytes: &mut Vec<u8>, name: &[u8]) {
        assert!(name.len() <= 16);
        bytes.extend_from_slice(name);
        bytes.resize(bytes.len() + 16 - name.len(), 0);
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn unchanged_normalized_archive_patch_state_accepts_only_unlinked_churn() {
        let archive = |invocation: &[u8], metadata: &[u8], object: &[u8]| {
            let mut builder = ar::Builder::new(Vec::new());
            builder
                .append(
                    &ar::Header::new(b"lib.rmeta".to_vec(), metadata.len() as u64),
                    metadata,
                )
                .unwrap();
            builder
                .append(
                    &ar::Header::new(
                        [b"crate-hash.cgu.".as_slice(), invocation, b".rcgu.o"].concat(),
                        object.len() as u64,
                    ),
                    object,
                )
                .unwrap();
            builder.into_inner().unwrap()
        };
        let input_path = hex::encode("libarchive.rlib");
        let section = PatchSection {
            input: hex::encode("libarchive.rlib\0crate-hash.cgu.old.rcgu.o\00:1"),
            section_index: 1,
            section_name: Some(".data".to_owned()),
            input_size: 4,
            output_offset: 64,
            output_size: 8,
            data_hash: Some(hash_bytes(&[1, 2, 3, 4])),
            cstring_nul_boundaries_hash: None,
        };
        let previous = archive(b"old", b"previous metadata", &growable_data_elf());
        let current = archive(b"new", b"new metadata", &growable_data_elf());
        let previous_patch = PreviousPatchState {
            fingerprint: patch_fingerprint(&previous, input_path.as_str(), [section.clone()])
                .unwrap()
                .unwrap(),
            archive_member_patch_fingerprints: None,
            sections: vec![section.clone()],
        };
        let input = FileState {
            path: input_path,
            content: FileContentState::from_bytes(&previous),
            snapshot_identity: None,
            patch: None,
        };

        let state = classify_normalized_rust_archive_patch_state(&input, &current, &previous_patch)
            .unwrap();
        assert!(matches!(
            &state,
            NormalizedRustArchivePatchState::Unchanged(_)
        ));
        if let NormalizedRustArchivePatchState::Unchanged(patch) = state {
            assert_eq!(patch.fingerprint, previous_patch.fingerprint);
            assert_eq!(patch.sections.len(), 1);
        }

        let mismatched_previous_patch = PreviousPatchState {
            fingerprint: "mismatched fingerprint".to_owned(),
            archive_member_patch_fingerprints: None,
            sections: vec![section.clone()],
        };
        let mismatched_state = classify_normalized_rust_archive_patch_state(
            &input,
            &current,
            &mismatched_previous_patch,
        )
        .unwrap();
        assert!(matches!(
            &mismatched_state,
            NormalizedRustArchivePatchState::Unknown
        ));

        let mut changed_object = growable_data_elf();
        changed_object[0x40] = 9;
        let changed = archive(b"current", b"new metadata", &changed_object);
        let changed_state =
            classify_normalized_rust_archive_patch_state(&input, &changed, &previous_patch)
                .unwrap();
        assert!(matches!(
            &changed_state,
            NormalizedRustArchivePatchState::MatchedButNotUnchanged(_)
        ));
        if let NormalizedRustArchivePatchState::MatchedButNotUnchanged(matched) = changed_state {
            assert_eq!(matched.sections.len(), 1);
            assert_eq!(matched.changed_sections.len(), 1);
        }
    }

    #[test]
    fn archive_member_identifiers_track_member_set() {
        let mut builder = ar::Builder::new(Vec::new());
        builder
            .append(
                &ar::Header::new(b"first.o".to_vec(), 5),
                b"first".as_slice(),
            )
            .unwrap();
        builder
            .append(
                &ar::Header::new(b"second.o".to_vec(), 6),
                b"second".as_slice(),
            )
            .unwrap();
        let archive = builder.into_inner().unwrap();

        assert_eq!(
            archive_member_identifiers(&archive).unwrap().unwrap(),
            vec![b"first.o".to_vec(), b"second.o".to_vec()]
        );
        assert!(
            archive_member_identifiers(b"not an archive")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            archive_member_patch_identifier(b"crate-hash.cgu.session.rcgu.o"),
            b"crate-hash.cgu.rcgu.o".to_vec()
        );
        assert_eq!(
            archive_member_patch_identifier(b"foreign-member.o"),
            b"foreign-member.o".to_vec()
        );
    }

    #[test]
    fn rustc_rlib_link_content_digest_decoder_requires_versioned_blake3_trailer() {
        let digest = "a".repeat(blake3::OUT_LEN * 2);
        let mut metadata = b"encoded metadata".to_vec();
        metadata.extend_from_slice(RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX);
        metadata.extend_from_slice(digest.as_bytes());
        metadata.extend_from_slice(RUSTC_SERIALIZED_METADATA_END);

        assert_eq!(
            decode_rustc_rlib_link_content_digest(&metadata),
            Some(digest)
        );
        assert_eq!(
            decode_rustc_rlib_link_content_digest(b"encoded metadatarust-end-file"),
            None
        );

        let last = metadata.len() - RUSTC_SERIALIZED_METADATA_END.len() - 1;
        metadata[last] = b'g';
        assert_eq!(decode_rustc_rlib_link_content_digest(&metadata), None);
    }

    #[test]
    fn rustc_rlib_raw_object_digest_decoder_requires_versioned_ordered_trailer() {
        let aggregate_digest = "c".repeat(blake3::OUT_LEN * 2);
        let objects = [
            (
                b"crate.cgu.0.old.rcgu.o".as_slice(),
                "a".repeat(blake3::OUT_LEN * 2),
            ),
            (
                b"crate.cgu.1.old.rcgu.o".as_slice(),
                "b".repeat(blake3::OUT_LEN * 2),
            ),
        ];
        let mut payload = Vec::new();
        payload.extend_from_slice(&(objects.len() as u64).to_le_bytes());
        for (identifier, digest) in &objects {
            payload.extend_from_slice(&(identifier.len() as u64).to_le_bytes());
            payload.extend_from_slice(identifier);
            payload.extend_from_slice(digest.as_bytes());
        }
        let mut metadata = b"encoded metadata".to_vec();
        metadata.extend_from_slice(RUSTC_RLIB_RAW_OBJECT_DIGESTS_PREFIX);
        metadata.extend_from_slice(&payload);
        metadata.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        metadata.extend_from_slice(RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX);
        metadata.extend_from_slice(aggregate_digest.as_bytes());
        metadata.extend_from_slice(RUSTC_SERIALIZED_METADATA_END);

        assert_eq!(
            decode_rustc_rlib_raw_object_digests(&metadata),
            Some(
                objects
                    .iter()
                    .map(|(identifier, digest)| RustcRlibRawObjectDigest {
                        identifier: identifier.to_vec(),
                        digest: digest.clone(),
                    })
                    .collect()
            )
        );
        assert_eq!(
            decode_rustc_rlib_link_content_digest(&metadata),
            Some(aggregate_digest)
        );

        let payload_len_offset = metadata.len()
            - RUSTC_SERIALIZED_METADATA_END.len()
            - RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX.len()
            - blake3::OUT_LEN * 2
            - 8;
        metadata[payload_len_offset..payload_len_offset + 8]
            .copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode_rustc_rlib_raw_object_digests(&metadata), None);
        assert_eq!(
            decode_rustc_rlib_link_content_digest(&metadata),
            Some("c".repeat(blake3::OUT_LEN * 2))
        );
    }

    #[test]
    fn archive_patch_fingerprint_reuses_trusted_unchanged_rustc_object_digests() {
        let archive = |digest: &str, object: &[u8]| {
            let mut payload = Vec::new();
            payload.extend_from_slice(&1_u64.to_le_bytes());
            payload.extend_from_slice(&(b"crate.cgu.0.old.rcgu.o".len() as u64).to_le_bytes());
            payload.extend_from_slice(b"crate.cgu.0.old.rcgu.o");
            payload.extend_from_slice(digest.as_bytes());
            let mut metadata = b"encoded metadata".to_vec();
            metadata.extend_from_slice(RUSTC_RLIB_RAW_OBJECT_DIGESTS_PREFIX);
            metadata.extend_from_slice(&payload);
            metadata.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            metadata.extend_from_slice(RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX);
            metadata.extend_from_slice("c".repeat(blake3::OUT_LEN * 2).as_bytes());
            metadata.extend_from_slice(RUSTC_SERIALIZED_METADATA_END);
            let metadata = rmeta_link_wrapper_elf(&metadata);
            let mut builder = ar::Builder::new(Vec::new());
            builder
                .append(
                    &ar::Header::new(
                        RUSTC_RLIB_LINK_METADATA_MEMBER.to_vec(),
                        metadata.len() as u64,
                    ),
                    metadata.as_slice(),
                )
                .unwrap();
            builder
                .append(
                    &ar::Header::new(b"crate.cgu.0.old.rcgu.o".to_vec(), object.len() as u64),
                    object,
                )
                .unwrap();
            builder.into_inner().unwrap()
        };
        let digest = "a".repeat(blake3::OUT_LEN * 2);
        let previous = archive(&digest, b"object");
        let changed_with_same_digest = archive(&digest, b"mutant");
        let changed_with_changed_digest = archive(&"b".repeat(blake3::OUT_LEN * 2), b"mutant");
        let (previous_fingerprint, previous_members) =
            archive_patch_fingerprint_with_previous(&previous, &[], None, true)
                .unwrap()
                .unwrap();
        let (reused_fingerprint, _) = archive_patch_fingerprint_with_previous(
            &changed_with_same_digest,
            &[],
            previous_members.as_deref(),
            true,
        )
        .unwrap()
        .unwrap();
        let (changed_fingerprint, _) = archive_patch_fingerprint_with_previous(
            &changed_with_changed_digest,
            &[],
            previous_members.as_deref(),
            true,
        )
        .unwrap()
        .unwrap();

        assert_eq!(previous_fingerprint, reused_fingerprint);
        assert_ne!(previous_fingerprint, changed_fingerprint);
        assert_eq!(
            archive_patch_fingerprint(&previous, &[]).unwrap(),
            Some(previous_fingerprint)
        );
    }

    #[test]
    fn rustc_rlib_link_content_digest_reads_metadata_archive_member() {
        let archive = |digest: &str| {
            let mut metadata = b"encoded metadata".to_vec();
            metadata.extend_from_slice(RUSTC_RLIB_LINK_CONTENT_DIGEST_PREFIX);
            metadata.extend_from_slice(digest.as_bytes());
            metadata.extend_from_slice(RUSTC_SERIALIZED_METADATA_END);
            let metadata = rmeta_link_wrapper_elf(&metadata);
            let mut builder = ar::Builder::new(Vec::new());
            builder
                .append(
                    &ar::Header::new(
                        RUSTC_RLIB_LINK_METADATA_MEMBER.to_vec(),
                        metadata.len() as u64,
                    ),
                    metadata.as_slice(),
                )
                .unwrap();
            builder.into_inner().unwrap()
        };
        let previous_digest = "a".repeat(blake3::OUT_LEN * 2);
        let current_digest = "b".repeat(blake3::OUT_LEN * 2);
        let previous = archive(&previous_digest);
        let current = archive(&current_digest);
        let input = FileState {
            path: hex::encode("libarchive.rlib"),
            content: FileContentState::from_bytes(&previous),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: String::new(),
                archive_member_set_proof: archive_member_set_proof(&previous).unwrap(),
                archive_member_patch_fingerprints: None,
                sections: Vec::new(),
                raw_sections: None,
            }),
        };

        assert_eq!(
            rustc_rlib_link_content_digest(&previous),
            Some(previous_digest)
        );
        assert!(rustc_rlib_link_content_digest_matches_previous(
            &input, &previous
        ));
        assert!(!rustc_rlib_link_content_digest_matches_previous(
            &input, &current
        ));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("libarchive.rlib");
        std::fs::write(&path, &previous).unwrap();
        let content = rustc_rlib_link_content_digest_matches_previous_path(&input, &path).unwrap();
        assert_eq!(content.len, previous.len() as u64);
        assert!(content.hash.is_empty());
        assert!(content.identity.is_some());

        std::fs::write(&path, &current).unwrap();
        assert!(rustc_rlib_link_content_digest_matches_previous_path(&input, &path).is_none());
    }

    #[test]
    fn archive_member_set_proofs_preserve_order_and_normalize_rustc_invocations() {
        let archive = |members: &[&[u8]]| {
            let mut builder = ar::Builder::new(Vec::new());
            for member in members {
                builder
                    .append(
                        &ar::Header::new(member.to_vec(), member.len() as u64),
                        *member,
                    )
                    .unwrap();
            }
            builder.into_inner().unwrap()
        };
        let previous = archive(&[b"first.o", b"crate-hash.cgu.old.rcgu.o"]);
        let renamed = archive(&[b"first.o", b"crate-hash.cgu.new.rcgu.o"]);
        let reordered = archive(&[b"crate-hash.cgu.new.rcgu.o", b"first.o"]);
        let previous_proof = archive_member_set_proof(&previous).unwrap().unwrap();
        let renamed_proof = archive_member_set_proof(&renamed).unwrap().unwrap();
        let reordered_proof = archive_member_set_proof(&reordered).unwrap().unwrap();
        let direct_previous_patch = PreviousPatchState {
            fingerprint: String::new(),
            archive_member_patch_fingerprints: None,
            sections: Vec::new(),
        };

        assert_ne!(
            previous_proof.raw_ordered_hash,
            reordered_proof.raw_ordered_hash
        );
        assert_eq!(
            previous_proof.normalized_ordered_hash,
            renamed_proof.normalized_ordered_hash
        );
        assert_ne!(
            previous_proof.normalized_ordered_hash,
            reordered_proof.normalized_ordered_hash
        );
        assert_eq!(previous_proof.member_count, 2);

        let input = FileState {
            path: hex::encode("libarchive.rlib"),
            content: FileContentState::from_bytes(&previous),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: String::new(),
                archive_member_set_proof: Some(previous_proof),
                archive_member_patch_fingerprints: None,
                sections: Vec::new(),
                raw_sections: None,
            }),
        };
        assert_eq!(
            archive_member_set_proof_matches_current(
                &input,
                &direct_previous_patch,
                Some(&renamed_proof),
                true,
            ),
            Some(true)
        );
        assert_eq!(
            archive_member_set_proof_matches_current(
                &input,
                &direct_previous_patch,
                Some(&reordered_proof),
                true,
            ),
            Some(false)
        );
        assert_eq!(
            archive_member_set_proof_matches_current(
                &input,
                &direct_previous_patch,
                Some(&reordered_proof),
                false,
            ),
            Some(false)
        );
        assert_eq!(
            archive_member_set_proof_matches_current(&input, &direct_previous_patch, None, false),
            Some(false)
        );

        let direct_input = FileState {
            path: hex::encode("input.o"),
            content: FileContentState::from_bytes(b"object"),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: String::new(),
                archive_member_set_proof: None,
                archive_member_patch_fingerprints: None,
                sections: Vec::new(),
                raw_sections: None,
            }),
        };
        assert_eq!(
            archive_member_set_proof_matches_current(
                &direct_input,
                &direct_previous_patch,
                None,
                false,
            ),
            Some(true)
        );

        let legacy_archive = FileState {
            path: hex::encode("libarchive.rlib"),
            content: FileContentState::from_bytes(&previous),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: String::new(),
                archive_member_set_proof: None,
                archive_member_patch_fingerprints: None,
                sections: Vec::new(),
                raw_sections: None,
            }),
        };
        let legacy_previous_patch = PreviousPatchState {
            fingerprint: String::new(),
            archive_member_patch_fingerprints: None,
            sections: vec![PatchSection {
                input: hex::encode("libarchive.rlib\0member.o\00:1"),
                section_index: 0,
                section_name: None,
                input_size: 0,
                output_offset: 0,
                output_size: 0,
                data_hash: None,
                cstring_nul_boundaries_hash: None,
            }],
        };
        assert_eq!(
            archive_member_set_proof_matches_current(
                &legacy_archive,
                &legacy_previous_patch,
                None,
                false,
            ),
            Some(false)
        );
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    fn archive_member_reordering_requires_same_member_set() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("libarchive.a");
        let mut previous_builder = ar::Builder::new(Vec::new());
        previous_builder
            .append(
                &ar::Header::new(b"padding.o".to_vec(), 7),
                b"padding".as_slice(),
            )
            .unwrap();
        previous_builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 6),
                b"member".as_slice(),
            )
            .unwrap();
        let previous_archive = previous_builder.into_inner().unwrap();
        std::fs::write(&input, &previous_archive).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let mut member_ref = input.as_os_str().as_encoded_bytes().to_vec();
        member_ref.push(0);
        member_ref.extend_from_slice(b"member.o");
        member_ref.push(0);
        member_ref.extend_from_slice(b"8:14");
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, &previous_archive),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: String::new(),
                archive_member_set_proof: None,
                archive_member_patch_fingerprints: None,
                sections: vec![FilePatchSectionState {
                    input: hex::encode(member_ref),
                    section_index: 0,
                    section_name: None,
                    input_size: 0,
                    output_offset: 0,
                    output_size: 0,
                    data_hash: None,
                    cstring_nul_boundaries_hash: None,
                }],
                raw_sections: None,
            }),
        };

        let mut current_builder = ar::Builder::new(Vec::new());
        current_builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 6),
                b"member".as_slice(),
            )
            .unwrap();
        current_builder
            .append(
                &ar::Header::new(b"padding.o".to_vec(), 7),
                b"padding".as_slice(),
            )
            .unwrap();
        let current_archive = current_builder.into_inner().unwrap();

        assert!(
            !archive_members_match_snapshot(
                &state_dir,
                &previous,
                Some(previous_archive.as_slice()),
                &current_archive,
                true,
            )
            .unwrap()
        );
        assert!(
            !archive_members_match_snapshot(
                &state_dir,
                &previous,
                Some(previous_archive.as_slice()),
                &current_archive,
                false,
            )
            .unwrap()
        );

        let mut changed_builder = ar::Builder::new(Vec::new());
        changed_builder
            .append(
                &ar::Header::new(b"member.o".to_vec(), 6),
                b"member".as_slice(),
            )
            .unwrap();
        changed_builder
            .append(
                &ar::Header::new(b"extra.o".to_vec(), 5),
                b"extra".as_slice(),
            )
            .unwrap();
        let changed_archive = changed_builder.into_inner().unwrap();

        assert!(
            !archive_members_match_snapshot(
                &state_dir,
                &previous,
                Some(previous_archive.as_slice()),
                &changed_archive,
                true,
            )
            .unwrap()
        );
        assert!(
            !archive_members_match_snapshot(
                &state_dir,
                &previous,
                Some(previous_archive.as_slice()),
                b"not an archive",
                true,
            )
            .unwrap()
        );
        assert!(
            archive_members_match_snapshot(
                &state_dir,
                &previous,
                Some(previous_archive.as_slice()),
                &previous_archive,
                true,
            )
            .unwrap()
        );

        let mut previous_rust_builder = ar::Builder::new(Vec::new());
        previous_rust_builder
            .append(
                &ar::Header::new(b"crate-hash.cgu.old.rcgu.o".to_vec(), 6),
                b"member".as_slice(),
            )
            .unwrap();
        let previous_rust_archive = previous_rust_builder.into_inner().unwrap();
        std::fs::write(&input, &previous_rust_archive).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let rust_previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, &previous_rust_archive),
            snapshot_identity: None,
            patch: None,
        };
        let mut current_rust_builder = ar::Builder::new(Vec::new());
        current_rust_builder
            .append(
                &ar::Header::new(b"crate-hash.cgu.new.rcgu.o".to_vec(), 6),
                b"member".as_slice(),
            )
            .unwrap();
        let current_rust_archive = current_rust_builder.into_inner().unwrap();

        assert!(
            archive_members_match_snapshot(
                &state_dir,
                &rust_previous,
                Some(previous_rust_archive.as_slice()),
                &current_rust_archive,
                true,
            )
            .unwrap()
        );
        assert!(
            !archive_members_match_snapshot(
                &state_dir,
                &rust_previous,
                Some(previous_rust_archive.as_slice()),
                &current_rust_archive,
                false,
            )
            .unwrap()
        );
    }

    #[test]
    fn special_ordered_sections_are_not_directly_patchable() {
        assert!(section_name_allows_direct_patching(b".text.foo"));
        assert!(section_name_allows_direct_patching(b".data.foo"));
        assert!(section_name_allows_direct_patching(b"__data"));
        assert!(section_name_allows_direct_patching(b"__const"));
        assert!(section_name_allows_direct_patching(b"__cstring"));
        assert!(section_name_allows_direct_patching(b"__text"));
        assert!(!section_name_allows_direct_patching(b".eh_frame"));
        assert!(!section_name_allows_direct_patching(b".eh_frame_hdr"));
        assert!(!section_name_allows_direct_patching(b".init"));
        assert!(!section_name_allows_direct_patching(b".fini"));
        assert!(!section_name_allows_direct_patching(b".init_array"));
        assert!(!section_name_allows_direct_patching(b".init_array.100"));
        assert!(!section_name_allows_direct_patching(b".fini_array"));
        assert!(!section_name_allows_direct_patching(b".preinit_array"));
        assert!(!section_name_allows_direct_patching(b".ctors"));
        assert!(!section_name_allows_direct_patching(b".dtors"));
        assert!(!section_name_allows_direct_patching(b"__eh_frame"));
        assert!(!section_name_allows_direct_patching(b"__mod_init_func"));
    }

    #[test]
    fn start_stop_sections_are_not_padded() {
        assert!(section_name_allows_incremental_padding(b".text.foo"));
        assert!(section_name_allows_incremental_padding(b".data.foo"));
        assert!(section_name_allows_incremental_padding(b"__const"));
        assert!(!section_name_allows_incremental_padding(b"foo"));
        assert!(!section_name_allows_incremental_padding(b"bar"));
        assert!(!section_name_allows_incremental_padding(b"__cstring"));
        assert!(!section_name_allows_incremental_padding(b"__text"));
        assert!(!section_name_allows_incremental_padding(b".init_array"));
        assert!(!section_name_allows_incremental_padding(b".eh_frame"));
    }

    #[test]
    fn patchable_bytes_match_ignores_preserved_relocation_ranges() {
        let input = [1, 2, 3, 4, 5, 6];
        let linked = [1, 9, 9, 4, 8, 6];

        assert!(patchable_bytes_match(&linked, &input, &[1..3, 4..5]));
        assert!(!patchable_bytes_match(&linked, &input, &[1..3]));
        assert!(!patchable_bytes_match(
            &[0, 9, 9, 4, 8, 6],
            &input,
            &[1..3, 4..5]
        ));
    }

    #[test]
    fn x86_64_elf_pc_relative_code_relocations_are_directly_patchable() {
        for relocation_kind in [
            object::elf::R_X86_64_PC32,
            object::elf::R_X86_64_PLT32,
            object::elf::R_X86_64_GOTPCREL,
        ] {
            let bytes = relocated_text_elf(relocation_kind);
            let input_ref = encode_path(Path::new("input.o"));
            let patch_section = PatchSection {
                input: input_ref.clone(),
                section_index: 1,
                section_name: Some(".text".to_owned()),
                input_size: 8,
                output_offset: 64,
                output_size: 8,
                data_hash: None,
                cstring_nul_boundaries_hash: None,
            };

            let patches = patch_sections_for_input(&bytes, &input_ref, [patch_section])
                .unwrap()
                .unwrap();

            assert_eq!(patches[0].preserve_ranges, vec![4..8]);
        }
    }

    #[test]
    fn x86_64_elf_got_code_relocation_relaxation_class_must_be_stable() {
        let bytes = relocated_text_elf(object::elf::R_X86_64_GOTPCREL);
        let mut non_relaxable_change = bytes.clone();
        non_relaxable_change[0x42] = 0x90;
        non_relaxable_change[0x43] = 0x01;
        let mut newly_relaxable = bytes.clone();
        newly_relaxable[0x42] = 0x8b;
        let mut no_longer_relaxable = newly_relaxable.clone();
        no_longer_relaxable[0x42] = 0x90;
        let input_ref = encode_path(Path::new("input.o"));
        let patch_section = PatchSection {
            input: input_ref.clone(),
            section_index: 1,
            section_name: Some(".text".to_owned()),
            input_size: 8,
            output_offset: 64,
            output_size: 8,
            data_hash: None,
            cstring_nul_boundaries_hash: None,
        };

        assert!(
            matched_x86_64_elf_got_relaxation_contexts_are_stable(
                Some(&bytes),
                &bytes,
                &input_ref,
                &[MatchedPatchSection::same(patch_section.clone())]
            )
            .unwrap()
        );
        assert!(
            matched_x86_64_elf_got_relaxation_contexts_are_stable(
                Some(&bytes),
                &non_relaxable_change,
                &input_ref,
                &[MatchedPatchSection::same(patch_section.clone())]
            )
            .unwrap()
        );
        assert!(
            !matched_x86_64_elf_got_relaxation_contexts_are_stable(
                Some(&bytes),
                &newly_relaxable,
                &input_ref,
                &[MatchedPatchSection::same(patch_section.clone())]
            )
            .unwrap()
        );
        assert!(
            !matched_x86_64_elf_got_relaxation_contexts_are_stable(
                Some(&newly_relaxable),
                &no_longer_relaxable,
                &input_ref,
                &[MatchedPatchSection::same(patch_section.clone())]
            )
            .unwrap()
        );
        assert!(
            !matched_x86_64_elf_got_relaxation_contexts_are_stable(
                None,
                &no_longer_relaxable,
                &input_ref,
                &[MatchedPatchSection::same(patch_section)]
            )
            .unwrap()
        );
    }

    #[test]
    fn input_snapshot_bytes_are_loaded_lazily_once() {
        let loads = std::cell::Cell::new(0);
        let mut bytes = LazyInputSnapshotBytes::new(|| {
            loads.set(loads.get() + 1);
            Ok(Some(b"snapshot".to_vec()))
        });

        assert!(bytes.get_if_loaded().is_none());
        assert_eq!(loads.get(), 0);
        assert_eq!(bytes.get().unwrap(), Some(b"snapshot".as_slice()));
        assert_eq!(bytes.get().unwrap(), Some(b"snapshot".as_slice()));
        assert_eq!(loads.get(), 1);
    }

    #[test]
    fn persisted_state_round_trips() {
        let mut state = state("args", b"output", &[("a.o", b"a"), ("b.o", b"bbb")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state.sections.push(generated_section_record(
            "generated:.rela.dyn.general",
            256,
            24,
        ));
        assert_eq!(PersistedState::parse(&state.render()).unwrap(), state);
    }

    #[test]
    fn macho_symbol_resolutions_round_trip() {
        let resolutions = vec![
            MachOSymbolResolutionRecord {
                name: SharedText::from(hex::encode("_direct")),
                direct_value: Some(0x1000),
                got_address: None,
                stub_address: None,
                thunk_addresses: vec![0x1800],
                target: Some(RelocationTargetRecord {
                    input_file: SharedText::from(hex::encode("a.o")),
                    input: SharedText::from(hex::encode("a.o")),
                    section_index: 2,
                    section_offset: 16,
                }),
            },
            MachOSymbolResolutionRecord {
                name: SharedText::from(hex::encode("_import")),
                direct_value: None,
                got_address: Some(0x2000),
                stub_address: Some(0x3000),
                thunk_addresses: Vec::new(),
                target: None,
            },
        ];
        let mut rendered = String::new();
        write_rendered_macho_symbol_resolutions(&mut rendered, &resolutions).unwrap();

        assert_eq!(
            parse_macho_symbol_resolutions(rendered.lines()).unwrap(),
            resolutions
        );
    }

    fn rematerialized_macho_replay(
        input: &str,
        target_name: &str,
        target_value: u64,
        target: RelocationTargetRecord,
    ) -> MachOTextRelocationReplay {
        let raw_relocation = object::macho::RelocationInfo {
            r_address: 0,
            r_symbolnum: 0,
            r_pcrel: true,
            r_length: 2,
            r_extern: true,
            r_type: object::macho::ARM64_RELOC_PAGE21,
        };
        let rel_info =
            <crate::macho_aarch64::MachOAArch64 as crate::platform::Arch>::relocation_from_raw(
                raw_relocation,
            )
            .unwrap();
        MachOTextRelocationReplay {
            relocation_index: None,
            input_file: SharedText::from(hex::encode(input)),
            input: hex::encode(input),
            section_index: 1,
            previous_range: None,
            current_range: 0..4,
            r_type: raw_relocation.r_type,
            kind: encode_macho_aarch64_relocation_kind(raw_relocation),
            rel_info,
            previous_written_value: None,
            previous_applied_target_value: None,
            current_target_value: target_value,
            current_relocation_target_candidates: vec![target_value],
            current_target_section_offset: target.section_offset,
            target_symbol_id: 1,
            target_name: Some(SharedText::from(hex::encode(target_name))),
            target_is_global: true,
            target: Some(target),
            addend: 0,
        }
    }

    #[test]
    fn rematerialized_macho_symbol_resolution_activates_new_direct_target() {
        let input = state("args", b"output", &[("input.o", b"input")])
            .input_files
            .remove(0);
        let target = RelocationTargetRecord {
            input_file: SharedText::from(hex::encode("input.o")),
            input: SharedText::from(hex::encode("input.o")),
            section_index: 3,
            section_offset: 32,
        };
        let replay =
            rematerialized_macho_replay("input.o", "_new_target", 0x1000_4020, target.clone());
        let mut resolutions = Vec::new();

        let (output_symbols, changed) = reconcile_rematerialized_macho_symbol_resolutions(
            &mut resolutions,
            &[],
            &[replay],
            &[(
                SharedText::from(hex::encode("input.o")),
                hex::encode("input.o"),
                1,
                hex::encode("input.o"),
                1,
            )],
            &input,
        )
        .unwrap();

        assert!(changed);
        assert_eq!(output_symbols.len(), 1);
        assert_eq!(output_symbols[0].target_name, hex::encode("_new_target"));
        assert_eq!(output_symbols[0].previous_target_value, 0);
        assert_eq!(output_symbols[0].target_value, 0x1000_4020);
        assert_eq!(
            resolutions,
            vec![MachOSymbolResolutionRecord {
                name: SharedText::from(hex::encode("_new_target")),
                direct_value: Some(0x1000_4020),
                got_address: None,
                stub_address: None,
                thunk_addresses: Vec::new(),
                target: Some(target),
            }]
        );
    }

    #[test]
    fn rematerialized_macho_symbol_resolution_rejects_unproven_removal() {
        let input = state("args", b"output", &[("input.o", b"input")])
            .input_files
            .remove(0);
        let relocation = relocation_record(
            "input.o",
            1,
            1,
            Some(0),
            0x1000_4010,
            Some("_target"),
            Some(("input.o", 3, 16)),
            0,
            0x350,
            4,
            encode_macho_aarch64_relocation_kind(object::macho::RelocationInfo {
                r_address: 0,
                r_symbolnum: 0,
                r_pcrel: true,
                r_length: 2,
                r_extern: true,
                r_type: object::macho::ARM64_RELOC_PAGE21,
            }),
            0,
        );
        let mut resolutions = vec![MachOSymbolResolutionRecord {
            name: SharedText::from(hex::encode("_target")),
            direct_value: Some(0x1000_4010),
            got_address: None,
            stub_address: None,
            thunk_addresses: Vec::new(),
            target: relocation.target.clone(),
        }];

        let result = reconcile_rematerialized_macho_symbol_resolutions(
            &mut resolutions,
            &[relocation],
            &[],
            &[(
                SharedText::from(hex::encode("input.o")),
                hex::encode("input.o"),
                1,
                hex::encode("input.o"),
                1,
            )],
            &input,
        );

        assert!(matches!(
            result,
            Err(reason)
                if reason.contains(
                    "removed Mach-O text relocation target may change symbol liveness"
                )
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn macho_symbol_resolutions_are_loaded_lazily_from_hashed_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state
            .macho_symbol_resolutions
            .push(MachOSymbolResolutionRecord {
                name: SharedText::from(hex::encode("_target")),
                direct_value: Some(0x1000),
                got_address: Some(0x2000),
                stub_address: Some(0x3000),
                thunk_addresses: vec![0x4000, 0x5000],
                target: None,
            });

        state.write(dir.path()).unwrap();
        let mut parsed = PersistedState::read_metadata(dir.path()).unwrap().unwrap();

        assert!(parsed.macho_symbol_resolutions.is_empty());
        assert!(
            parsed
                .macho_symbol_resolutions_file
                .as_deref()
                .is_some_and(|file| file.starts_with(COMPRESSED_MACHO_RESOLUTIONS_FILE_PREFIX))
        );
        parsed.load_macho_symbol_resolutions(dir.path()).unwrap();
        assert_eq!(
            parsed.macho_symbol_resolutions,
            state.macho_symbol_resolutions
        );
    }

    #[test]
    fn v36_state_is_accepted_without_macho_resolution_sidecar() {
        let rendered = state("args", b"output", &[("a.o", b"a")])
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V36, 1)
            .lines()
            .filter(|line| !line.starts_with("macho-resolutions-file\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert!(parsed.macho_symbol_resolutions.is_empty());
        assert!(parsed.macho_symbol_resolutions_file.is_none());
    }

    #[test]
    fn persisted_state_round_trips_fde_records() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state.fdes.push(fde_record("a.o", 1, 4, 32, 200, 24));

        let rendered = state.render();

        assert!(rendered.contains("\nfdes\t1\n"));
        assert!(rendered.contains("\nfde\t0\t1\t4\t32\t200\t24\n"));
        assert_eq!(PersistedState::parse(&rendered).unwrap(), state);
    }

    #[test]
    fn persisted_state_round_trips_relocation_records() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state.relocations.push(relocation_record(
            "a.o",
            1,
            42,
            Some(0x5678),
            0x1234,
            Some("target"),
            Some(("a.o", 2, 16)),
            8,
            300,
            8,
            1,
            -4,
        ));
        state.relocations[0].applied_target_value = Some(0x9abc);

        let rendered = state.render();

        assert!(rendered.contains("\nrelocs\t1\n"));
        assert!(rendered.contains(
            "\nreloc3\t0\t1\t42\t8\t300\t8\t1\t-4\t22136\t4660\t39612\t746172676574\t0\t2\t16\n"
        ));
        assert_eq!(PersistedState::parse(&rendered).unwrap(), state);
    }

    #[test]
    fn v37_relocation_records_are_accepted_without_applied_target_value() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.relocations.push(relocation_record(
            "a.o",
            1,
            42,
            Some(0x5678),
            0x1234,
            Some("target"),
            Some(("a.o", 2, 16)),
            8,
            300,
            8,
            1,
            -4,
        ));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V37, 1)
            .lines()
            .map(current_relocation_as_v37_line)
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.relocations, state.relocations);
        assert_eq!(parsed.relocations[0].applied_target_value, None);
    }

    #[test]
    fn v25_state_version_is_accepted_without_written_relocation_value() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.relocations.push(relocation_record(
            "a.o",
            1,
            42,
            Some(0x5678),
            0x1234,
            Some("target"),
            Some(("a.o", 2, 16)),
            8,
            300,
            8,
            1,
            -4,
        ));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V25, 1)
            .lines()
            .map(current_relocation_as_v25_line)
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.relocations.len(), 1);
        assert_eq!(parsed.relocations[0].written_value, None);
        assert_eq!(parsed.relocations[0].target_value, 0x1234);
        assert_eq!(
            parsed.relocations[0].target_name,
            Some(SharedText::from(hex::encode("target")))
        );
        assert_eq!(
            parsed.relocations[0].target,
            Some(RelocationTargetRecord {
                input_file: hex::encode("a.o").into(),
                input: hex::encode("a.o").into(),
                section_index: 2,
                section_offset: 16,
            })
        );
    }

    #[test]
    fn v24_state_version_is_accepted_without_relocation_target_owner() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.relocations.push(relocation_record(
            "a.o",
            1,
            42,
            Some(0x5678),
            0x1234,
            Some("target"),
            Some(("a.o", 2, 16)),
            8,
            300,
            8,
            1,
            -4,
        ));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V24, 1)
            .lines()
            .map(current_relocation_as_v24_line)
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.relocations.len(), 1);
        assert_eq!(parsed.relocations[0].written_value, None);
        assert_eq!(parsed.relocations[0].target_value, 0x1234);
        assert_eq!(
            parsed.relocations[0].target_name,
            Some(SharedText::from(hex::encode("target")))
        );
        assert_eq!(parsed.relocations[0].target, None);
    }

    #[test]
    fn v23_state_version_is_accepted_without_relocation_target_metadata() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.relocations.push(relocation_record(
            "a.o",
            1,
            42,
            Some(0x5678),
            0x1234,
            Some("target"),
            Some(("a.o", 2, 16)),
            8,
            300,
            8,
            1,
            -4,
        ));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V23, 1)
            .lines()
            .map(current_relocation_as_v23_line)
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.relocations.len(), 1);
        assert_eq!(parsed.relocations[0].written_value, None);
        assert_eq!(parsed.relocations[0].target_value, 0);
        assert_eq!(parsed.relocations[0].target_name, None);
    }

    #[test]
    fn persisted_state_round_trips_dynamic_relocation_records() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state
            .dynamic_relocations
            .push(dynamic_relocation_record("a.o", 1, 8, 300, 24));
        state
            .dynamic_relocations
            .push(dynamic_relocation_record_with_output_info(
                "a.o",
                1,
                16,
                324,
                24,
                0x400040,
                0x100000006,
            ));

        let rendered = state.render();

        assert!(rendered.contains("\ndynrels\t2\n"));
        assert!(rendered.contains("\ndynrel\t0\t1\t8\t300\t24\n"));
        assert!(rendered.contains("\ndynrel\t0\t1\t16\t324\t24\t4194368\t4294967302\n"));
        assert_eq!(PersistedState::parse(&rendered).unwrap(), state);
    }

    #[test]
    fn v27_state_version_is_accepted_without_dynamic_relocation_output_info() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state
            .dynamic_relocations
            .push(dynamic_relocation_record("a.o", 1, 8, 300, 24));
        let rendered = state.render().replacen(STATE_VERSION, STATE_VERSION_V27, 1);

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.dynamic_relocations, state.dynamic_relocations);
        assert_eq!(parsed.dynamic_relocations[0].output_r_offset, None);
        assert_eq!(parsed.dynamic_relocations[0].output_r_info, None);
    }

    #[test]
    fn v22_state_version_is_accepted_without_relocation_records() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.fdes.push(fde_record("a.o", 1, 4, 32, 200, 24));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V22, 1)
            .lines()
            .filter(|line| {
                !line.starts_with("relocs\t")
                    && !line.starts_with("reloc\t")
                    && !line.starts_with("reloc2\t")
                    && !line.starts_with("reloc3\t")
            })
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.fdes.len(), 1);
        assert!(parsed.relocations.is_empty());
    }

    #[test]
    fn persisted_state_round_trips_patch_metadata() {
        let mut state = state("args", b"output", &[("a.o", b"a"), ("b.o", b"bbb")]);
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: "patch-hash".to_owned(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: vec![
                FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 1,
                    section_name: Some(".text.foo".to_owned()),
                    input_size: 4,
                    output_offset: 100,
                    output_size: 4,
                    data_hash: Some("text-hash".to_owned()),
                    cstring_nul_boundaries_hash: None,
                },
                FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 3,
                    section_name: Some(".data".to_owned()),
                    input_size: 8,
                    output_offset: 112,
                    output_size: 12,
                    data_hash: Some("data-hash".to_owned()),
                    cstring_nul_boundaries_hash: None,
                },
                FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 5,
                    section_name: None,
                    input_size: 16,
                    output_offset: 128,
                    output_size: 16,
                    data_hash: None,
                    cstring_nul_boundaries_hash: None,
                },
            ],
            raw_sections: None,
        });
        state.sections.push(section_record("a.o", 1, 100, 12));

        let rendered = state.render();

        assert!(rendered.contains(&format!(
            "\tpatch-hash\t{}:1:4:100:4:{}:text-hash,{}:3:8:112:12:{}:data-hash,{}:5:16:128:16:-:-\t-\n",
            hex::encode("a.o"),
            hex::encode(".text.foo"),
            hex::encode("a.o"),
            hex::encode(".data"),
            hex::encode("a.o"),
        )));
        assert_eq!(PersistedState::parse(&rendered).unwrap(), state);
    }

    #[test]
    fn persisted_state_round_trips_lazy_snapshot_proofs() {
        let mut state = state("args", b"output", &[("libarchive.a", b"archive")]);
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: "patch-hash".to_owned(),
            archive_member_set_proof: Some(ArchiveMemberSetProof {
                raw_ordered_hash: "raw-hash".to_owned(),
                normalized_ordered_hash: "normalized-hash".to_owned(),
                member_count: 3,
                rustc_link_content_digest: Some("a".repeat(blake3::OUT_LEN * 2)),
            }),
            archive_member_patch_fingerprints: Some(vec![ArchiveMemberPatchFingerprint {
                archive_member_index: 2,
                identifier: b"crate.cgu.rcgu.o".to_vec(),
                data_len: 42,
                ranges_hash: "b".repeat(blake3::OUT_LEN * 2),
                rustc_object_digest: "c".repeat(blake3::OUT_LEN * 2),
                fingerprint: "d".repeat(blake3::OUT_LEN * 2),
            }]),
            sections: vec![FilePatchSectionState {
                input: hex::encode("libarchive.a"),
                section_index: 1,
                section_name: Some("__cstring".to_owned()),
                input_size: 4,
                output_offset: 100,
                output_size: 4,
                data_hash: Some("data-hash".to_owned()),
                cstring_nul_boundaries_hash: Some("cstring-hash".to_owned()),
            }],
            raw_sections: None,
        });

        assert_eq!(PersistedState::parse(&state.render()).unwrap(), state);
    }

    #[test]
    fn v33_archive_member_set_proof_without_rustc_link_content_digest_is_accepted() {
        let mut state = state("args", b"output", &[("libarchive.a", b"archive")]);
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: "patch-hash".to_owned(),
            archive_member_set_proof: Some(ArchiveMemberSetProof {
                raw_ordered_hash: "raw-hash".to_owned(),
                normalized_ordered_hash: "normalized-hash".to_owned(),
                member_count: 3,
                rustc_link_content_digest: None,
            }),
            archive_member_patch_fingerprints: None,
            sections: Vec::new(),
            raw_sections: None,
        });
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V33, 1)
            .replacen(
                "raw-hash:normalized-hash:3:-",
                "raw-hash:normalized-hash:3",
                1,
            );

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed, state);
    }

    #[test]
    fn v32_patch_metadata_without_lazy_snapshot_proofs_is_accepted() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: "patch-hash".to_owned(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: vec![FilePatchSectionState {
                input: hex::encode("a.o"),
                section_index: 1,
                section_name: Some(".data".to_owned()),
                input_size: 4,
                output_offset: 100,
                output_size: 4,
                data_hash: Some("data-hash".to_owned()),
                cstring_nul_boundaries_hash: None,
            }],
            raw_sections: None,
        });
        let rendered = state.render().replacen(STATE_VERSION, STATE_VERSION_V32, 1);

        let parsed = PersistedState::parse(&rendered).unwrap();
        let patch = parsed.input_files[0].patch.as_ref().unwrap();

        assert!(patch.archive_member_set_proof.is_none());
        assert!(patch.sections[0].cstring_nul_boundaries_hash.is_none());
    }

    #[test]
    fn metadata_only_input_parse_preserves_raw_patch_sections() {
        let raw_sections = format!(
            "{}:7:11:100:13:{}:data-hash",
            hex::encode("a.o"),
            hex::encode(".text.a")
        );
        let line = format!(
            "input\t{}\t1\t{}\t-\tpatch-hash\t{}",
            hex::encode("a.o"),
            hash_bytes(b"a"),
            raw_sections
        );

        let parsed = parse_input_line(&line, PatchSectionReadMode::PreserveRaw).unwrap();
        let patch = parsed.patch.as_ref().unwrap();

        assert_eq!(patch.fingerprint, "patch-hash");
        assert!(patch.sections.is_empty());
        assert_eq!(patch.raw_sections.as_deref(), Some(raw_sections.as_str()));
        assert_eq!(render_patch_sections(patch), raw_sections);
    }

    #[test]
    fn changed_input_patch_metadata_is_parsed_lazily() {
        let raw_sections = format!(
            "{}:7:11:100:13:{}:data-hash",
            hex::encode("a.o"),
            hex::encode(".text.a")
        );
        let line = format!(
            "input\t{}\t1\t{}\t-\tpatch-hash\t{}",
            hex::encode("a.o"),
            hash_bytes(b"a"),
            raw_sections
        );
        let parsed = parse_input_line(&line, PatchSectionReadMode::PreserveRaw).unwrap();

        let previous = patch_sections_from_previous_state(&parsed, Path::new("a.o")).unwrap();

        assert_eq!(previous.fingerprint, "patch-hash");
        assert_eq!(previous.sections.len(), 1);
        assert_eq!(previous.sections[0].input, hex::encode("a.o"));
        assert_eq!(previous.sections[0].section_index, 7);
        assert_eq!(
            previous.sections[0].section_name.as_deref(),
            Some(".text.a")
        );
        assert_eq!(previous.sections[0].input_size, 11);
        assert_eq!(previous.sections[0].output_offset, 100);
        assert_eq!(previous.sections[0].output_size, 13);
        assert_eq!(previous.sections[0].data_hash.as_deref(), Some("data-hash"));
    }

    #[test]
    fn patch_state_matches_current_section_records() {
        let patch = FilePatchState {
            fingerprint: "patch-hash".to_owned(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: vec![
                FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 3,
                    section_name: Some(".text.a".to_owned()),
                    input_size: 4,
                    output_offset: 200,
                    output_size: 8,
                    data_hash: Some("text-hash".to_owned()),
                    cstring_nul_boundaries_hash: None,
                },
                FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 1,
                    section_name: Some(".data.a".to_owned()),
                    input_size: 4,
                    output_offset: 100,
                    output_size: 4,
                    data_hash: Some("data-hash".to_owned()),
                    cstring_nul_boundaries_hash: None,
                },
            ],
            raw_sections: None,
        };
        let first = section_record("a.o", 1, 100, 4);
        let second = section_record("a.o", 3, 200, 8);
        let moved = section_record("a.o", 3, 208, 8);

        assert!(patch_state_matches_section_records(
            &patch,
            &[&second, &first]
        ));
        assert!(!patch_state_matches_section_records(
            &patch,
            &[&first, &moved]
        ));
    }

    #[test]
    fn record_patch_fingerprints_preserves_matching_existing_patch() {
        let arena = colosseum::sync::Arena::new();
        let file_loader = FileLoader::new(&arena);
        let mut output =
            LazyOutputBytes::new(|| panic!("matching patch metadata should not read output bytes"));
        let mut input_files = vec![FileState {
            path: hex::encode("a.o"),
            content: FileContentState::from_bytes(b"a"),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: "patch-hash".to_owned(),
                archive_member_set_proof: None,
                archive_member_patch_fingerprints: None,
                sections: vec![FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 1,
                    section_name: Some(".data.a".to_owned()),
                    input_size: 4,
                    output_offset: 100,
                    output_size: 4,
                    data_hash: Some("patch-section-hash".to_owned()),
                    cstring_nul_boundaries_hash: None,
                }],
                raw_sections: None,
            }),
        }];
        let sections = vec![section_record("a.o", 1, 100, 4)];

        record_patch_fingerprints(
            &mut input_files,
            &file_loader,
            &sections,
            &[],
            &[],
            &[],
            &mut output,
        )
        .unwrap();

        assert_eq!(
            input_files[0].patch.as_ref().unwrap().fingerprint,
            "patch-hash"
        );
    }

    #[test]
    fn record_patch_fingerprints_preserves_matching_existing_patch_with_metadata() {
        let arena = colosseum::sync::Arena::new();
        let file_loader = FileLoader::new(&arena);
        let mut output =
            LazyOutputBytes::new(|| panic!("matching patch metadata should not read output bytes"));
        let mut input_files = vec![FileState {
            path: hex::encode("a.o"),
            content: FileContentState::from_bytes(b"a"),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: "patch-hash".to_owned(),
                archive_member_set_proof: None,
                archive_member_patch_fingerprints: None,
                sections: vec![FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 1,
                    section_name: Some(".data.a".to_owned()),
                    input_size: 4,
                    output_offset: 100,
                    output_size: 4,
                    data_hash: Some("patch-section-hash".to_owned()),
                    cstring_nul_boundaries_hash: None,
                }],
                raw_sections: None,
            }),
        }];
        let sections = vec![section_record("a.o", 1, 100, 4)];
        let relocations = vec![relocation_record(
            "a.o",
            1,
            42,
            Some(0x1000),
            0x1000,
            Some("target"),
            Some(("a.o", 1, 0)),
            0,
            100,
            8,
            1,
            0,
        )];
        let fdes = vec![fde_record("a.o", 1, 2, 0, 200, 24)];
        let dynamic_relocations = vec![dynamic_relocation_record("a.o", 1, 0, 300, 24)];

        record_patch_fingerprints(
            &mut input_files,
            &file_loader,
            &sections,
            &relocations,
            &fdes,
            &dynamic_relocations,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            input_files[0].patch.as_ref().unwrap().fingerprint,
            "patch-hash"
        );
    }

    #[test]
    fn record_patch_fingerprints_clears_stale_patch_without_loaded_input() {
        let arena = colosseum::sync::Arena::new();
        let file_loader = FileLoader::new(&arena);
        let mut output =
            LazyOutputBytes::new(|| panic!("missing loaded input should not read output bytes"));
        let mut input_files = vec![FileState {
            path: hex::encode("a.o"),
            content: FileContentState::from_bytes(b"a"),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: "patch-hash".to_owned(),
                archive_member_set_proof: None,
                archive_member_patch_fingerprints: None,
                sections: vec![FilePatchSectionState {
                    input: hex::encode("a.o"),
                    section_index: 1,
                    section_name: Some(".data.a".to_owned()),
                    input_size: 4,
                    output_offset: 100,
                    output_size: 4,
                    data_hash: Some("patch-section-hash".to_owned()),
                    cstring_nul_boundaries_hash: None,
                }],
                raw_sections: None,
            }),
        }];
        let sections = vec![section_record("a.o", 1, 108, 4)];

        record_patch_fingerprints(
            &mut input_files,
            &file_loader,
            &sections,
            &[],
            &[],
            &[],
            &mut output,
        )
        .unwrap();

        assert!(input_files[0].patch.is_none());
    }

    #[test]
    fn snapshot_loaded_files_drops_inherited_eager_patch_metadata() {
        let arena = colosseum::sync::Arena::new();
        let file_loader = FileLoader::new(&arena);
        let mut input_files = vec![FileState {
            path: hex::encode("a.o"),
            content: FileContentState::from_bytes(b"a"),
            snapshot_identity: None,
            patch: Some(FilePatchState {
                fingerprint: "legacy-patch-hash".to_owned(),
                archive_member_set_proof: None,
                archive_member_patch_fingerprints: None,
                sections: Vec::new(),
                raw_sections: None,
            }),
        }];

        snapshot_loaded_files(Path::new("unused"), &file_loader, &mut input_files, &[]).unwrap();

        assert!(input_files[0].patch.is_none());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn content_hash_matches_rewritten_input_without_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, b"object").unwrap();
        let previous = FileState {
            path: encode_path(&input),
            content: FileContentState::from_bytes(b"object"),
            snapshot_identity: None,
            patch: None,
        };

        assert!(!input_snapshot_path(&state_dir, &input).exists());
        assert!(input_content_matches_previous(&state_dir, &previous, &input).unwrap());
        std::fs::write(&input, b"changed").unwrap();
        assert!(!input_content_matches_previous(&state_dir, &previous, &input).unwrap());
    }

    #[test]
    fn v10_patch_metadata_without_section_names_is_accepted() {
        let line = format!(
            "input\t{}\t1\t{}\t-\tpatch-hash\t1:4:100:4,3:8:112:12",
            hex::encode("a.o"),
            hash_bytes(b"a")
        );

        let parsed = parse_input_line(&line, PatchSectionReadMode::Parse).unwrap();
        let sections = parsed.patch.unwrap().sections;

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].input, hex::encode("a.o"));
        assert_eq!(sections[1].input, hex::encode("a.o"));
        assert_eq!(sections[0].section_name, None);
        assert_eq!(sections[1].section_name, None);
    }

    #[test]
    fn v12_state_version_is_accepted_without_sld_version() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V12, 1)
            .lines()
            .filter(|line| !line.starts_with("sld-version\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.sections.len(), 1);
        assert!(parsed.sld_version.is_none());
    }

    #[test]
    fn v13_patch_metadata_is_accepted_without_section_hashes() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: "patch-hash".to_owned(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: vec![FilePatchSectionState {
                input: hex::encode("a.o"),
                section_index: 1,
                section_name: Some(".data".to_owned()),
                input_size: 4,
                output_offset: 100,
                output_size: 8,
                data_hash: Some("section-hash".to_owned()),
                cstring_nul_boundaries_hash: None,
            }],
            raw_sections: None,
        });
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V13, 1)
            .replace(":section-hash", "");

        let parsed = PersistedState::parse(&rendered).unwrap();
        let patch = parsed.input_files[0].patch.as_ref().unwrap();

        assert_eq!(patch.sections.len(), 1);
        assert_eq!(patch.sections[0].section_name.as_deref(), Some(".data"));
        assert_eq!(patch.sections[0].data_hash, None);
    }

    #[test]
    fn current_state_version_requires_sld_version() {
        let rendered = state("args", b"output", &[("a.o", b"a")])
            .render()
            .lines()
            .filter(|line| !line.starts_with("sld-version\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let error = PersistedState::parse(&rendered).unwrap_err();

        assert!(error.to_string().contains("Missing sld version"));
    }

    #[test]
    fn current_state_version_requires_link_options_hash() {
        let rendered = state("args", b"output", &[("a.o", b"a")])
            .render()
            .lines()
            .filter(|line| !line.starts_with("link-options\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let error = PersistedState::parse(&rendered).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Missing incremental link-options")
        );
    }

    #[test]
    fn current_state_version_requires_input_order_hash() {
        let rendered = state("args", b"output", &[("a.o", b"a")])
            .render()
            .lines()
            .filter(|line| !line.starts_with("input-order\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let error = PersistedState::parse(&rendered).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Missing incremental input-order")
        );
    }

    #[test]
    fn v16_state_version_is_accepted_without_input_order_hash() {
        let rendered = state("args", b"output", &[("a.o", b"a")])
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V16, 1)
            .lines()
            .filter(|line| !line.starts_with("input-order\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.input_order_hash, None);
    }

    #[test]
    fn v18_state_version_is_accepted_without_dynamic_relocation_records() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.fdes.push(fde_record("a.o", 1, 4, 32, 200, 24));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V18, 1)
            .lines()
            .filter(|line| !line.starts_with("dynrels\t") && !line.starts_with("dynrel\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.fdes.len(), 1);
        assert!(parsed.dynamic_relocations.is_empty());
    }

    #[test]
    fn v19_state_version_is_accepted_without_dynamic_relocation_offsets() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state
            .dynamic_relocations
            .push(dynamic_relocation_record("a.o", 1, 0, 300, 24));
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V19, 1)
            .replace("\ndynrel\t0\t1\t0\t300\t24\n", "\ndynrel\t0\t1\t300\t24\n");

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.dynamic_relocations, state.dynamic_relocations);
    }

    #[test]
    fn v15_state_version_is_accepted_without_link_options_hash() {
        let rendered = state("args", b"output", &[("a.o", b"a")])
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V15, 1)
            .lines()
            .filter(|line| !line.starts_with("link-options\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.link_options_hash, None);
    }

    #[test]
    fn old_patch_section_metadata_cannot_patch_changed_inputs() {
        let line = format!(
            "input\t{}\t1\t{}\t-\told-patch-hash\t1:4,3:8",
            hex::encode("a.o"),
            hash_bytes(b"a")
        );

        let parsed = parse_input_line(&line, PatchSectionReadMode::Parse).unwrap();

        assert_eq!(parsed.patch.unwrap().sections, Vec::new());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_input_patch_rejects_missing_metadata_before_reading_changed_input() {
        let dir = tempfile::tempdir().unwrap();
        let missing_input = dir.path().join("missing.o");
        let previous = PersistedState {
            args_hash: "args".to_owned(),
            link_options_hash: Some("args".to_owned()),
            input_order_hash: Some(input_order_hash_for_paths([missing_input
                .to_str()
                .unwrap()])),
            sld_version: Some("sld-test".to_owned()),
            link_start: None,
            output: FileContentState::from_bytes(b"output"),
            build_id_hashes: None,
            input_files: vec![FileState {
                path: encode_path(&missing_input),
                content: FileContentState::from_bytes(b"previous"),
                snapshot_identity: None,
                patch: None,
            }],
            sections: Vec::new(),
            relocations: Vec::new(),
            macho_symbol_resolutions: Vec::new(),
            fdes: Vec::new(),
            dynamic_relocations: Vec::new(),
            sections_file: None,
            macho_symbol_resolutions_file: None,
            patch_records_file: None,
            patch_record_locations: Vec::new(),
            raw_patch_record_locations: None,
        };

        let result = patch_changed_inputs(
            &crate::args::elf::ElfArgs::default(),
            dir.path(),
            previous,
            None,
            true,
            &[(0, missing_input)],
            &[0],
        )
        .unwrap();

        let ChangedInputPatchResult::Unsupported(reason) = result else {
            panic!("changed input was unexpectedly patched");
        };
        assert!(reason.contains("missing patch metadata"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn patchable_identity_rewrite_updates_state_without_patching_output() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let output = dir.path().join("output");
        let input = dir.path().join("input.o");
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"object").unwrap();

        let input_path = input.to_str().unwrap();
        let mut previous = state("args", b"output", &[(input_path, b"object")]);
        previous.output = FileContentState::from_path(&output).unwrap();
        previous.input_files[0].content = FileContentState::from_path(&input).unwrap();
        previous.input_files[0].patch = Some(FilePatchState {
            fingerprint: "unused-patch".to_owned(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: Vec::new(),
            raw_sections: None,
        });
        let replacement = dir.path().join("replacement.o");
        std::fs::write(&replacement, b"object").unwrap();
        std::fs::rename(&replacement, &input).unwrap();

        let result = patch_changed_inputs(
            &crate::args::elf::ElfArgs::default(),
            &state_dir,
            previous,
            None,
            true,
            &[(0, input.clone())],
            &[0],
        )
        .unwrap();

        assert!(matches!(result, ChangedInputPatchResult::Patched));
        assert_eq!(std::fs::read(&output).unwrap(), b"output");
        let updated = PersistedState::read(&state_dir).unwrap().unwrap();
        assert!(
            updated.input_files[0]
                .content
                .identity_matches_path(&input)
                .unwrap()
        );
        let log = std::fs::read_to_string(state_dir.join(LOG_FILE)).unwrap();
        assert!(log.contains("updated 1 rewritten input file before loading inputs"));
        assert!(log.contains("reused existing output before loading inputs"));
        assert!(!log.contains("patched 1 changed input file before loading inputs"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_input_derives_missing_patch_metadata_from_snapshot() {
        let bytes = growable_data_elf();
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let input = dir.path().join("input.o");
        std::fs::write(&input, &bytes).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let previous = FileState {
            path: encode_path(&input),
            content: content_hash_with_path_identity(&input, &bytes),
            snapshot_identity: None,
            patch: None,
        };
        let sections = vec![section_record(input.to_str().unwrap(), 1, 64, 8)];
        let mut output = vec![0; 72];
        output[64..68].copy_from_slice(&bytes[0x40..0x44]);

        let patch = current_patch_state_from_snapshot(
            &state_dir,
            &previous,
            &output,
            &sections,
            &[],
            &[],
            &[],
            true,
        )
        .unwrap()
        .unwrap();

        assert_eq!(patch.sections.len(), 1);
        assert_eq!(patch.sections[0].section_name.as_deref(), Some(".data"));
        assert_eq!(patch.sections[0].input_size, 4);
        assert_eq!(patch.sections[0].output_offset, 64);
        assert_eq!(patch.sections[0].output_size, 8);
    }

    #[test]
    fn persisted_state_round_trips_build_id_hashes() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        let output_len = 5 * BUILD_ID_HASH_GROUP_LEN + 100;
        let nodes = build_id_hash_node_count(output_len).unwrap();
        state.build_id_hashes = Some(BuildIdHashState {
            output_len: output_len as u64,
            nodes,
            tree_hash: Some("tree-hash".to_owned()),
        });

        let rendered = state.render();

        assert!(rendered.contains(&format!(
            "\nbuild-id-hash\t{output_len}\t{nodes}\ttree-hash\n"
        )));
        assert_eq!(PersistedState::parse(&rendered).unwrap(), state);
    }

    #[test]
    fn legacy_build_id_hashes_are_accepted_without_tree_hash() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        let output_len = 5 * BUILD_ID_HASH_GROUP_LEN + 100;
        let nodes = build_id_hash_node_count(output_len).unwrap();
        state.build_id_hashes = Some(BuildIdHashState {
            output_len: output_len as u64,
            nodes,
            tree_hash: Some("tree-hash".to_owned()),
        });
        let rendered = state
            .render()
            .replacen(STATE_VERSION, STATE_VERSION_V14, 1)
            .replace("\ttree-hash\n", "\n");

        let parsed = PersistedState::parse(&rendered).unwrap();

        assert_eq!(parsed.build_id_hashes.unwrap().tree_hash, None);
    }

    #[test]
    fn old_patch_fingerprint_without_section_list_is_ignored() {
        let line = format!(
            "input\t{}\t1\t{}\t-\told-patch-hash",
            hex::encode("a.o"),
            hash_bytes(b"a")
        );

        let parsed = parse_input_line(&line, PatchSectionReadMode::Parse).unwrap();

        assert!(parsed.patch.is_none());
    }

    #[test]
    fn compact_state_interns_repeated_section_inputs() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state.sections.push(section_record("a.o", 2, 112, 8));

        let rendered = state.render();

        assert!(rendered.contains("\nsection-inputs\t1\n"));
        assert!(rendered.contains("\nsection\t0\t1\t100\t12\n"));
        assert!(rendered.contains("\nsection\t0\t2\t112\t8\n"));
        assert_eq!(PersistedState::parse(&rendered).unwrap(), state);
    }

    #[test]
    fn record_text_interner_reuses_allocations() {
        let interner = RecordTextInterner::default();
        let first = interner.intern("same-input".to_owned());
        let second = interner.intern("same-input".to_owned());

        assert!(Arc::ptr_eq(&first.0, &second.0));
    }

    #[test]
    fn record_text_interner_caches_input_texts() {
        let mut input_file = crate::input_data::InputFile::for_testing();
        input_file.filename = PathBuf::from("a.o");
        let input = InputRef {
            file: &input_file,
            entry: None,
        };
        let interner = RecordTextInterner::default();

        let first = interner.intern_input(input);
        let second = interner.intern_input(input);

        assert!(Arc::ptr_eq(&first.0.0, &second.0.0));
        assert!(Arc::ptr_eq(&first.1.0, &second.1.0));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn read_metadata_skips_missing_sections_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state.write(dir.path()).unwrap();
        let sections_file = PersistedState::read_metadata(dir.path())
            .unwrap()
            .unwrap()
            .sections_file
            .unwrap();
        std::fs::remove_file(dir.path().join(sections_file)).unwrap();

        let metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        assert!(metadata.sections.is_empty());
        assert!(metadata.patch_record_locations.is_empty());
        assert!(metadata.raw_patch_record_locations.is_some());
        assert!(PersistedState::read(dir.path()).is_err());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn read_metadata_preserves_patch_sections_without_parsing_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: "patch-hash".to_owned(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: vec![FilePatchSectionState {
                input: hex::encode("a.o"),
                section_index: 1,
                section_name: Some(".text.a".to_owned()),
                input_size: 4,
                output_offset: 100,
                output_size: 8,
                data_hash: Some("section-hash".to_owned()),
                cstring_nul_boundaries_hash: None,
            }],
            raw_sections: None,
        });
        let raw_sections = render_patch_sections(state.input_files[0].patch.as_ref().unwrap());
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.write(dir.path()).unwrap();

        let metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        let patch = metadata.input_files[0].patch.as_ref().unwrap();

        assert!(metadata.sections.is_empty());
        assert!(patch.sections.is_empty());
        assert_eq!(patch.raw_sections.as_deref(), Some(raw_sections.as_str()));
        assert!(metadata.patch_record_locations.is_empty());
        assert!(metadata.raw_patch_record_locations.is_some());
        assert!(metadata.render().contains(&raw_sections));

        let full = PersistedState::read(dir.path()).unwrap().unwrap();
        let full_patch = full.input_files[0].patch.as_ref().unwrap();
        assert_eq!(full_patch.sections.len(), 1);
        assert_eq!(full_patch.raw_sections, None);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn metadata_update_overlay_updates_only_changed_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: "old-patch-hash".to_owned(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: vec![FilePatchSectionState {
                input: hex::encode("a.o"),
                section_index: 1,
                section_name: Some(".text.a".to_owned()),
                input_size: 4,
                output_offset: 100,
                output_size: 8,
                data_hash: Some("old-section-hash".to_owned()),
                cstring_nul_boundaries_hash: None,
            }],
            raw_sections: None,
        });
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.macho_symbol_resolutions_file =
            Some(format!("{COMPRESSED_MACHO_RESOLUTIONS_FILE_PREFIX}old"));
        state.write(dir.path()).unwrap();
        let base_index = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        let mut updated = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        updated.link_start = Some(FileIdentity {
            len: 0,
            dev: 1,
            ino: 2,
            modified_sec: 3,
            modified_nsec: 4,
            changed_sec: 5,
            changed_nsec: 6,
        });
        updated.output = FileContentState::from_bytes(b"new-output");
        updated.macho_symbol_resolutions_file = None;
        updated.input_files[0].content = FileContentState::from_bytes(b"aa");
        updated.input_files[0].patch = Some(FilePatchState {
            fingerprint: "new-patch-hash".to_owned(),
            archive_member_set_proof: Some(ArchiveMemberSetProof {
                raw_ordered_hash: "raw-hash".to_owned(),
                normalized_ordered_hash: "normalized-hash".to_owned(),
                member_count: 2,
                rustc_link_content_digest: None,
            }),
            archive_member_patch_fingerprints: None,
            sections: vec![FilePatchSectionState {
                input: hex::encode("a.o"),
                section_index: 1,
                section_name: Some(".text.a".to_owned()),
                input_size: 4,
                output_offset: 100,
                output_size: 8,
                data_hash: Some("new-section-hash".to_owned()),
                cstring_nul_boundaries_hash: Some("cstring-hash".to_owned()),
            }],
            raw_sections: None,
        });

        updated
            .write_metadata_update_for_inputs(dir.path(), &[0])
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap(),
            base_index
        );
        assert!(metadata_update_path(dir.path()).exists());
        let metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        assert_eq!(metadata.output, updated.output);
        assert!(metadata.macho_symbol_resolutions_file.is_none());
        assert_eq!(
            metadata.input_files[0].content,
            updated.input_files[0].content
        );
        let metadata_patch = metadata.input_files[0].patch.as_ref().unwrap();
        let updated_patch = updated.input_files[0].patch.as_ref().unwrap();
        assert_eq!(metadata_patch.fingerprint, updated_patch.fingerprint);
        assert_eq!(
            metadata_patch.archive_member_set_proof,
            updated_patch.archive_member_set_proof
        );
        assert!(metadata_patch.sections.is_empty());
        assert_eq!(
            metadata_patch.raw_sections.as_deref(),
            Some(render_patch_sections(updated_patch).as_str())
        );
        assert_eq!(metadata.input_files[1], state.input_files[1]);

        metadata.write_index(dir.path()).unwrap();
        assert!(!metadata_update_path(dir.path()).exists());
        let persisted = PersistedState::read(dir.path()).unwrap().unwrap();
        assert_eq!(persisted.input_files[0].patch, updated.input_files[0].patch);
        assert_eq!(persisted.sections, state.sections);
        assert!(persisted.macho_symbol_resolutions_file.is_none());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn v1_metadata_update_preserves_macho_resolution_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        let file_name = format!("{COMPRESSED_MACHO_RESOLUTIONS_FILE_PREFIX}old");
        state.macho_symbol_resolutions_file = Some(file_name.clone());
        state.write(dir.path()).unwrap();
        let rendered = state
            .render_metadata_update(&[0])
            .replacen(METADATA_UPDATE_VERSION, METADATA_UPDATE_VERSION_V1, 1)
            .lines()
            .filter(|line| !line.starts_with("macho-resolutions-file\t"))
            .fold(String::new(), |mut out, line| {
                writeln!(&mut out, "{line}").unwrap();
                out
            });
        std::fs::write(metadata_update_path(dir.path()), rendered).unwrap();

        let parsed = PersistedState::read_metadata(dir.path()).unwrap().unwrap();

        assert_eq!(
            parsed.macho_symbol_resolutions_file.as_deref(),
            Some(file_name.as_str())
        );
    }

    #[test]
    fn metadata_update_indices_include_changed_and_rewritten_inputs() {
        let changed_inputs = vec![
            (2, PathBuf::from("changed-c.o")),
            (0, PathBuf::from("changed-a.o")),
        ];
        let rewritten_inputs = vec![
            (1, PathBuf::from("rewritten-b.o")),
            (2, PathBuf::from("rewritten-c.o")),
        ];

        assert_eq!(
            metadata_update_indices_for_inputs(&changed_inputs, &rewritten_inputs),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn complete_record_retry_skips_only_definitive_anonymous_section_mismatch() {
        assert!(changed_input_patch_retry_may_benefit_from_complete_records(
            "changed input needs complete section records"
        ));
        assert!(changed_input_patch_retry_may_benefit_from_complete_records(
            "changed input needs complete dynamic relocation records"
        ));
        assert!(changed_input_patch_retry_may_benefit_from_complete_records(
            "changed input needs complete FDE records"
        ));
        assert!(
            !changed_input_patch_retry_may_benefit_from_complete_records(
                "could not match anonymous patch sections in `input.o`"
            )
        );
        assert!(changed_input_patch_retry_may_benefit_from_complete_records(
            "changed bytes outside patchable sections in `input.o`"
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn reloaded_metadata_refreshes_rewritten_input_identities() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("rewritten.o");
        std::fs::write(&input, b"same").unwrap();
        let input_path = input.to_str().unwrap();
        let mut state = state("args", b"output", &[(input_path, b"same")]);
        state.input_files[0].content = FileContentState {
            len: 4,
            hash: hash_bytes(b"same"),
            identity: Some(identity(4, 1, 2, 3, 4)),
        };
        state.write(dir.path()).unwrap();
        let mut metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();

        assert!(
            input_identity_mismatch_reason(&metadata.input_files)
                .unwrap()
                .is_some()
        );

        refresh_rewritten_input_identities(&mut metadata, &[(0, input.clone())]);

        assert_eq!(
            input_identity_mismatch_reason(&metadata.input_files).unwrap(),
            None
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn read_records_for_input_files_filters_sections_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.sections.push(section_record("b.o", 1, 200, 8));
        state.relocations.push(relocation_record(
            "a.o",
            1,
            4,
            Some(0x1000),
            0x1000,
            Some("target"),
            None,
            0,
            100,
            8,
            1,
            0,
        ));
        state.relocations.push(relocation_record(
            "b.o",
            1,
            4,
            Some(0x2000),
            0x2000,
            Some("target"),
            None,
            0,
            200,
            8,
            1,
            0,
        ));
        state.fdes.push(fde_record("a.o", 1, 2, 0, 300, 24));
        state.fdes.push(fde_record("b.o", 1, 2, 0, 400, 24));
        state
            .dynamic_relocations
            .push(dynamic_relocation_record("a.o", 1, 0, 500, 24));
        state
            .dynamic_relocations
            .push(dynamic_relocation_record("b.o", 1, 0, 600, 24));
        state.write(dir.path()).unwrap();
        let mut metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        let input_files = [hex::encode("a.o")].into_iter().collect::<HashSet<_>>();

        metadata
            .read_records_for_input_files(dir.path(), &input_files)
            .unwrap();

        assert_eq!(metadata.sections.len(), 1);
        assert_eq!(metadata.relocations.len(), 1);
        assert_eq!(metadata.fdes.len(), 1);
        assert_eq!(metadata.dynamic_relocations.len(), 1);
        assert!(metadata.sections_file.is_some());
        assert!(
            metadata
                .sections
                .iter()
                .all(|record| record.input_file == hex::encode("a.o"))
        );
        assert!(
            metadata
                .relocations
                .iter()
                .all(|record| record.input_file == hex::encode("a.o"))
        );
        assert!(
            metadata
                .fdes
                .iter()
                .all(|record| record.input_file == hex::encode("a.o"))
        );
        assert!(
            metadata
                .dynamic_relocations
                .iter()
                .all(|record| record.input_file == hex::encode("a.o"))
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn read_records_for_input_files_reads_canonical_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.sections.push(section_record("b.o", 1, 200, 8));
        state.write(dir.path()).unwrap();
        let mut metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        assert_eq!(metadata.sections_file, metadata.patch_records_file);
        assert!(metadata.patch_record_locations.is_empty());
        assert!(metadata.raw_patch_record_locations.is_some());
        let input_files = [hex::encode("a.o")].into_iter().collect::<HashSet<_>>();

        metadata
            .read_records_for_input_files(dir.path(), &input_files)
            .unwrap();

        assert_eq!(metadata.sections, vec![state.sections[0].clone()]);
        assert!(!metadata.patch_record_locations.is_empty());
        assert!(metadata.raw_patch_record_locations.is_none());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn read_metadata_preserves_deferred_separate_patch_record_table_on_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let state = state("args", b"output", &[("a.o", b"a")]);
        let location = PatchRecordLocation {
            input_file: hex::encode("a.o"),
            offset: 3,
            len: 5,
            hash: "record-hash".to_owned(),
        };
        let index = state.render_index(
            "sections-records",
            Some("sections-patches"),
            std::slice::from_ref(&location),
            None,
            None,
        );
        std::fs::write(dir.path().join(INDEX_FILE), &index).unwrap();

        let mut metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        assert!(metadata.patch_record_locations.is_empty());
        assert!(metadata.raw_patch_record_locations.is_some());

        metadata.write_index(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap(),
            index
        );

        metadata.materialize_patch_record_locations().unwrap();
        assert_eq!(metadata.patch_record_locations, vec![location]);
        assert!(metadata.raw_patch_record_locations.is_none());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn canonical_index_aliases_incoming_relocations_without_duplicate_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.sections.push(section_record("b.o", 1, 200, 8));
        state.relocations.push(relocation_record(
            "a.o",
            1,
            4,
            Some(0x1000),
            0x2000,
            Some("target"),
            Some(("b.o", 1, 0)),
            0,
            100,
            8,
            1,
            0,
        ));
        state.write(dir.path()).unwrap();
        let mut metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        let sections_file = metadata.sections_file.clone().unwrap();
        let sidecar = String::from_utf8(
            zstd::stream::decode_all(
                std::fs::read(dir.path().join(sections_file))
                    .unwrap()
                    .as_slice(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            sidecar
                .lines()
                .filter(|line| line.starts_with("reloc3\t"))
                .count(),
            1
        );
        let input_files = [hex::encode("b.o")].into_iter().collect::<HashSet<_>>();

        metadata
            .read_records_for_input_files(dir.path(), &input_files)
            .unwrap();

        assert_eq!(metadata.sections, vec![state.sections[1].clone()]);
        assert_eq!(metadata.relocations, state.relocations);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn canonical_index_reconstructs_all_record_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.sections.push(SectionRecord {
            input_file: GENERATED_SECTION_INPUT_FILE.into(),
            input: GENERATED_RELA_DYN_GENERAL.into(),
            section_index: 0,
            output_offset: 300,
            size: 24,
        });
        state.fdes.push(fde_record("a.o", 1, 2, 0, 400, 24));
        state
            .dynamic_relocations
            .push(dynamic_relocation_record("a.o", 1, 0, 500, 24));
        state.write(dir.path()).unwrap();

        let restored = PersistedState::read(dir.path()).unwrap().unwrap();
        let mut expected_sections = state.sections.clone();
        expected_sections.sort();
        assert_eq!(restored.sections, expected_sections);
        assert_eq!(restored.fdes, state.fdes);
        assert_eq!(restored.dynamic_relocations, state.dynamic_relocations);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn canonical_index_is_stable_for_differently_ordered_records() {
        let first_dir = tempfile::tempdir().unwrap();
        let second_dir = tempfile::tempdir().unwrap();
        let mut first = state("args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        first.sections.push(section_record("b.o", 2, 200, 8));
        first.sections.push(section_record("a.o", 1, 100, 8));
        first.relocations.push(relocation_record(
            "b.o",
            2,
            4,
            Some(0x2000),
            0x1000,
            Some("target"),
            Some(("a.o", 1, 0)),
            0,
            200,
            8,
            1,
            0,
        ));
        first.relocations.push(relocation_record(
            "a.o",
            1,
            3,
            Some(0x1000),
            0x2000,
            Some("target"),
            Some(("b.o", 2, 0)),
            0,
            100,
            8,
            1,
            0,
        ));
        first.fdes.push(fde_record("b.o", 2, 4, 0, 240, 24));
        first.fdes.push(fde_record("a.o", 1, 3, 0, 140, 24));
        first
            .dynamic_relocations
            .push(dynamic_relocation_record("b.o", 2, 0, 280, 24));
        first
            .dynamic_relocations
            .push(dynamic_relocation_record("a.o", 1, 0, 180, 24));

        let mut second = first.clone();
        second.sections.reverse();
        second.relocations.reverse();
        second.fdes.reverse();
        second.dynamic_relocations.reverse();

        first.write(first_dir.path()).unwrap();
        second.write(second_dir.path()).unwrap();

        let first = PersistedState::read_metadata(first_dir.path())
            .unwrap()
            .unwrap();
        let second = PersistedState::read_metadata(second_dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(first.sections_file, second.sections_file);
        assert_eq!(first.patch_records_file, second.patch_records_file);
        assert_eq!(
            first.raw_patch_record_locations,
            second.raw_patch_record_locations
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn v30_uncompressed_canonical_index_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        let sidecar = state.render_sections();
        let file_name = section_sidecar_file_name(&sidecar);
        state
            .write_sections(dir.path(), &file_name, &sidecar)
            .unwrap();
        let location = PatchRecordLocation {
            input_file: hex::encode("a.o"),
            offset: 0,
            len: sidecar.len() as u64,
            hash: hash_bytes(sidecar.as_bytes()),
        };
        let index = state
            .render_index(&file_name, Some(&file_name), &[location], None, None)
            .replacen(STATE_VERSION, STATE_VERSION_V30, 1);
        std::fs::write(dir.path().join(INDEX_FILE), index).unwrap();

        assert_eq!(
            PersistedState::read(dir.path()).unwrap().unwrap().sections,
            state.sections
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn v31_compressed_canonical_index_without_snapshot_identity_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.write(dir.path()).unwrap();

        let current = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        let legacy = current
            .replacen(STATE_VERSION, STATE_VERSION_V31, 1)
            .lines()
            .map(|line| {
                if line.starts_with("input\t") {
                    line.strip_suffix("\t-").unwrap()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(dir.path().join(INDEX_FILE), legacy).unwrap();

        let restored = PersistedState::read(dir.path()).unwrap().unwrap();
        assert!(restored.input_files[0].snapshot_identity.is_none());
        assert_eq!(restored.sections, state.sections);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn read_records_for_input_files_validates_indexed_sidecar_hash() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.write(dir.path()).unwrap();
        let mut metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        let patch_records_file = metadata.patch_records_file.clone().unwrap();
        let patch_records_path = dir.path().join(patch_records_file);
        let mut contents = std::fs::read(&patch_records_path).unwrap();
        contents[0] ^= 1;
        std::fs::write(&patch_records_path, contents).unwrap();
        let input_files = [hex::encode("a.o")].into_iter().collect::<HashSet<_>>();

        let error = metadata
            .read_records_for_input_files(dir.path(), &input_files)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("do not match their content hash")
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn read_records_for_input_files_validates_sections_sidecar_hash() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 8));
        state.write(dir.path()).unwrap();
        let sections_file = PersistedState::read_metadata(dir.path())
            .unwrap()
            .unwrap()
            .sections_file
            .unwrap();
        let path = dir.path().join(&sections_file);
        let mut contents = std::fs::read(&path).unwrap();
        contents[0] ^= 1;
        std::fs::write(&path, contents).unwrap();
        let mut metadata = PersistedState::read_metadata(dir.path()).unwrap().unwrap();
        let input_files = [hex::encode("a.o")].into_iter().collect::<HashSet<_>>();

        let error = metadata
            .read_records_for_input_files(dir.path(), &input_files)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("do not match their content hash")
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn hashed_sections_sidecar_must_match_contents() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        state.write(dir.path()).unwrap();
        let sections_file = PersistedState::read_metadata(dir.path())
            .unwrap()
            .unwrap()
            .sections_file
            .unwrap();
        let path = dir.path().join(&sections_file);
        let mut contents = std::fs::read(&path).unwrap();
        contents[0] ^= 1;
        std::fs::write(&path, contents).unwrap();

        let error = PersistedState::read(dir.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("do not match their content hash")
        );
    }

    #[test]
    fn sections_sidecar_name_must_stay_in_state_dir() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        let rendered = format!(
            "{}sections-file\t../sections\n",
            state.render_header_and_inputs()
        );

        let error = PersistedState::parse(&rendered).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Invalid incremental sections sidecar name")
        );
    }

    #[test]
    fn previous_sections_are_only_needed_for_reuse_capable_modes() {
        assert!(!mode_needs_previous_sections(&IncrementalMode::Disabled));
        assert!(mode_needs_previous_sections(&IncrementalMode::Reuse));
        assert!(mode_needs_previous_sections(&IncrementalMode::Relink {
            reason: "input file changed".to_owned(),
            can_reuse_unchanged_sections: true,
        }));
        assert!(!mode_needs_previous_sections(&IncrementalMode::Relink {
            reason: "linker arguments changed".to_owned(),
            can_reuse_unchanged_sections: false,
        }));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn metadata_update_writes_sections_for_inline_legacy_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        assert!(state.sections_file.is_none());

        state.write_metadata_update(dir.path()).unwrap();

        let sections_file = PersistedState::read_metadata(dir.path())
            .unwrap()
            .unwrap()
            .sections_file
            .unwrap();
        assert!(sections_file.starts_with(COMPRESSED_SECTIONS_FILE_PREFIX));
        assert!(dir.path().join(&sections_file).exists());
        let index = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        assert!(index.contains(&format!("\nindexed-sections-file\t{sections_file}\n")));
        assert_eq!(
            PersistedState::read(dir.path()).unwrap().unwrap().sections,
            state.sections
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn section_sidecars_are_not_replaced_before_index_update() {
        let dir = tempfile::tempdir().unwrap();
        let mut old_state = state("old-args", b"old-output", &[("a.o", b"a")]);
        old_state.sections.push(section_record("a.o", 1, 100, 12));
        old_state.write(dir.path()).unwrap();
        let old_sections_file = PersistedState::read(dir.path())
            .unwrap()
            .unwrap()
            .sections_file
            .unwrap();

        let mut new_state = state("new-args", b"new-output", &[("b.o", b"b")]);
        new_state.sections.push(section_record("b.o", 7, 900, 16));
        let new_sections = new_state.render_sections();
        let new_sections_file = section_sidecar_file_name(&new_sections);
        new_state
            .write_sections(dir.path(), &new_sections_file, &new_sections)
            .unwrap();

        let read_after_torn_write = PersistedState::read(dir.path()).unwrap().unwrap();
        assert_eq!(read_after_torn_write.sections, old_state.sections);
        assert_eq!(
            read_after_torn_write.sections_file.as_deref(),
            Some(old_sections_file.as_str())
        );
        assert!(dir.path().join(new_sections_file).exists());
    }

    #[test]
    fn previous_state_version_is_accepted_without_sections() {
        let state = state("args", b"output", &[("a.o", b"a")]);
        let rendered = render_legacy_state(&state, STATE_VERSION_V1)
            .split_once("\nsections")
            .unwrap()
            .0
            .to_owned();
        let parsed = PersistedState::parse(&format!("{rendered}\n")).unwrap();
        assert!(parsed.sections.is_empty());
    }

    #[test]
    fn v2_state_version_is_accepted_with_sections() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        let rendered = render_legacy_state(&state, STATE_VERSION_V2);
        assert_eq!(PersistedState::parse(&rendered).unwrap().sections.len(), 1);
    }

    #[test]
    fn v3_state_version_is_accepted_with_sections() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        let rendered = render_legacy_state(&state, STATE_VERSION_V3);
        assert_eq!(PersistedState::parse(&rendered).unwrap().sections.len(), 1);
    }

    #[test]
    fn v4_state_version_is_accepted_with_sections() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        let rendered = render_legacy_state(&state, STATE_VERSION_V4);
        assert_eq!(PersistedState::parse(&rendered).unwrap().sections.len(), 1);
    }

    #[test]
    fn v5_state_version_is_accepted_with_compact_sections() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        let rendered = render_v8_state(&state).replacen(STATE_VERSION_V8, STATE_VERSION_V5, 1);
        assert_eq!(PersistedState::parse(&rendered).unwrap().sections.len(), 1);
    }

    #[test]
    fn v8_state_version_is_accepted_without_build_id_hashes() {
        let mut state = state("args", b"output", &[("a.o", b"a")]);
        state.sections.push(section_record("a.o", 1, 100, 12));
        let rendered = render_v8_state(&state);
        assert_eq!(PersistedState::parse(&rendered).unwrap().sections.len(), 1);
    }

    #[test]
    fn corrupt_state_is_rejected() {
        assert!(PersistedState::parse("not-sld\n").is_err());
    }

    #[test]
    fn content_hash_detects_same_length_changes() {
        let first = FileContentState::from_bytes(b"abcd");
        let second = FileContentState::from_bytes(b"wxyz");
        assert_eq!(first.len, second.len);
        assert_ne!(first, second);
    }

    #[test]
    fn file_identity_does_not_affect_content_equality() {
        let first = FileContentState {
            identity: Some(identity(4, 1, 2, 3, 5)),
            ..FileContentState::from_bytes(b"abcd")
        };
        let second = FileContentState {
            identity: Some(identity(4, 10, 20, 30, 50)),
            ..FileContentState::from_bytes(b"abcd")
        };

        assert_eq!(first, second);
    }

    #[test]
    fn file_identity_compares_content_when_hash_is_absent() {
        let first = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 3, 5)),
        };
        let same = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 3, 5)),
        };
        let changed = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 4, 5)),
        };

        assert_eq!(first, same);
        assert_ne!(first, changed);
    }

    #[test]
    fn file_identity_compares_changed_time() {
        let first = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 3, 5)),
        };
        let changed = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 3, 6)),
        };

        assert_ne!(first, changed);
    }

    #[test]
    fn missing_hash_does_not_match_missing_identity() {
        let first = FileContentState {
            len: 4,
            hash: String::new(),
            identity: None,
        };
        let second = FileContentState {
            len: 4,
            hash: String::new(),
            identity: None,
        };

        assert_ne!(first, second);
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn file_identity_matches_current_file_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.o");
        std::fs::write(&path, b"abcd").unwrap();
        let content = FileContentState::from_path(&path).unwrap();

        assert!(content.identity_matches_path(&path).unwrap());

        std::fs::write(&path, b"abcde").unwrap();
        assert!(!content.identity_matches_path(&path).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn output_hash_validates_content_after_identity_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output");
        std::fs::write(&path, b"abcd").unwrap();
        let content = FileContentState::from_path(&path).unwrap();

        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"abcd").unwrap();
        std::fs::rename(&replacement, &path).unwrap();

        assert!(!content.identity_matches_path(&path).unwrap());
        assert!(output_content_matches_previous(&content, &path, false).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn trusted_persistent_output_accepts_change_time_settling_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output");
        std::fs::write(&path, b"abcd").unwrap();
        let mut content = FileContentState::from_path_identity_only(&path).unwrap();
        content.identity.as_mut().unwrap().changed_sec -= 1;

        assert!(!output_content_matches_previous(&content, &path, false).unwrap());
        assert!(output_content_matches_previous(&content, &path, true).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn directly_patched_elf_output_keeps_hash_for_snapshot_restoration() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("output");
        std::fs::write(&output, b"patched output").unwrap();
        let args = crate::args::elf::ElfArgs::default();

        let content = directly_patched_output_content_state(&args, &output).unwrap();

        assert_eq!(content.hash, hash_bytes(b"patched output"));
    }

    #[test]
    fn changed_output_snapshot_updates_only_patched_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let state_dir = dir.path().join("out.incr");
        std::fs::write(&output, b"abcdef").unwrap();
        install_output_snapshot(&state_dir, &output).unwrap();
        std::fs::write(&output, b"aBCXef").unwrap();

        update_output_snapshot_from_ranges(&state_dir, &output, &[1..3]).unwrap();

        assert_eq!(
            std::fs::read(output_snapshot_path(&state_dir)).unwrap(),
            b"aBCdef"
        );
    }

    #[test]
    fn changed_output_snapshot_reinstalls_missing_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let state_dir = dir.path().join("out.incr");
        std::fs::write(&output, b"before").unwrap();
        install_output_snapshot(&state_dir, &output).unwrap();
        std::fs::remove_file(output_snapshot_path(&state_dir)).unwrap();
        std::fs::write(&output, b"after!").unwrap();

        update_output_snapshot_from_ranges(&state_dir, &output, &[0..1]).unwrap();

        assert_eq!(
            std::fs::read(output_snapshot_path(&state_dir)).unwrap(),
            b"after!"
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn preloading_restores_deleted_output_after_runtime_availability_change() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("input.o");
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"input").unwrap();

        let mut previous_args = crate::args::elf::ElfArgs::default();
        previous_args.common.incremental = true;
        previous_args.common.available_threads = NonZeroUsize::new(1).unwrap();
        previous_args.output = Arc::from(output.as_path());
        let state_dir = state_dir_for_output(&output);
        let mut state = publishing_metadata_state(&previous_args, &output, &input);
        state.output = FileContentState::from_path(&output).unwrap();
        state.write(&state_dir).unwrap();
        install_output_snapshot(&state_dir, &output).unwrap();
        std::fs::remove_file(&output).unwrap();

        let mut current_args = crate::args::elf::ElfArgs::default();
        current_args.common.incremental = true;
        current_args.common.available_threads = NonZeroUsize::new(2).unwrap();
        current_args.output = Arc::from(output.as_path());

        assert_eq!(args_hash(&previous_args), args_hash(&current_args));
        assert!(maybe_reuse_output_before_loading(&current_args).unwrap());
        assert_eq!(std::fs::read(&output).unwrap(), b"output");
        assert!(
            std::fs::read_to_string(state_dir.join(LOG_FILE))
                .unwrap()
                .contains("restored missing output from retained snapshot")
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn preloading_does_not_restore_deleted_output_before_changed_input_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("input.o");
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"input").unwrap();

        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        let state_dir = state_dir_for_output(&output);
        let mut state = publishing_metadata_state(&args, &output, &input);
        state.output = FileContentState::from_path(&output).unwrap();
        state.input_files[0].patch = Some(FilePatchState {
            fingerprint: String::new(),
            archive_member_set_proof: None,
            archive_member_patch_fingerprints: None,
            sections: Vec::new(),
            raw_sections: None,
        });
        state.write(&state_dir).unwrap();
        install_output_snapshot(&state_dir, &output).unwrap();
        std::fs::remove_file(&output).unwrap();
        std::fs::write(&input, b"changed").unwrap();

        assert!(!maybe_reuse_output_before_loading(&args).unwrap());
        assert!(!output.exists());
        assert!(
            std::fs::read_to_string(state_dir.join(LOG_FILE))
                .unwrap()
                .contains("missing retained output requires full relink")
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn preloading_preserves_snapshot_for_same_content_input_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("libarchive.rlib");
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"unchanged").unwrap();

        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        let state_dir = state_dir_for_output(&output);
        let mut state = publishing_metadata_state(&args, &output, &input);
        state.output = FileContentState::from_path(&output).unwrap();
        snapshot_input_paths(&state_dir, [input.as_path()]).unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        state.input_files[0].snapshot_identity = FileIdentity::from_path(&snapshot).unwrap();
        state.write(&state_dir).unwrap();

        let replacement = dir.path().join("replacement.rlib");
        std::fs::write(&replacement, b"unchanged").unwrap();
        std::fs::rename(&replacement, &input).unwrap();
        let retained_snapshot_identity = FileIdentity::from_path(&snapshot).unwrap();

        assert!(maybe_reuse_output_before_loading(&args).unwrap());
        let metadata = PersistedState::read_metadata(&state_dir).unwrap().unwrap();
        let retained_snapshot_identity = retained_snapshot_identity.unwrap();
        assert!(
            FileIdentity::from_path(&snapshot)
                .unwrap()
                .is_some_and(|snapshot_identity| snapshot_identity
                    .matches_same_data_ignoring_change_time(&retained_snapshot_identity))
        );
        assert!(
            metadata.input_files[0]
                .snapshot_identity
                .as_ref()
                .is_some_and(|snapshot_identity| snapshot_identity
                    .matches_same_data_ignoring_change_time(&retained_snapshot_identity))
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn preloading_installs_missing_snapshot_for_same_content_input_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("libarchive.rlib");
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"unchanged").unwrap();

        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        let state_dir = state_dir_for_output(&output);
        let mut state = publishing_metadata_state(&args, &output, &input);
        state.output = FileContentState::from_path(&output).unwrap();
        state.write(&state_dir).unwrap();

        let replacement = dir.path().join("replacement.rlib");
        std::fs::write(&replacement, b"unchanged").unwrap();
        std::fs::rename(&replacement, &input).unwrap();

        assert!(maybe_reuse_output_before_loading(&args).unwrap());
        let metadata = PersistedState::read_metadata(&state_dir).unwrap().unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        assert!(snapshot.exists());
        let snapshot_identity = FileIdentity::from_path(&snapshot).unwrap().unwrap();
        assert!(
            metadata.input_files[0]
                .snapshot_identity
                .as_ref()
                .is_some_and(|metadata_snapshot_identity| {
                    metadata_snapshot_identity
                        .matches_same_data_ignoring_change_time(&snapshot_identity)
                })
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn loaded_classification_defers_deleted_output_restore_after_input_argument_change() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("input.o");
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"input").unwrap();

        let mut previous_args = crate::args::elf::ElfArgs::default();
        previous_args.common.incremental = true;
        previous_args.output = Arc::from(output.as_path());
        let state_dir = state_dir_for_output(&output);
        let mut state = publishing_metadata_state(&previous_args, &output, &input);
        state.output = FileContentState::from_path(&output).unwrap();
        install_output_snapshot(&state_dir, &output).unwrap();
        std::fs::remove_file(&output).unwrap();

        let mut current_args = crate::args::elf::ElfArgs::default();
        current_args.common.incremental = true;
        current_args.output = Arc::from(output.as_path());
        platform::Args::parse(&mut current_args, ["added.o"].into_iter()).unwrap();

        assert_ne!(args_hash(&previous_args), args_hash(&current_args));
        assert_eq!(
            link_options_hash(&previous_args),
            link_options_hash(&current_args)
        );
        assert!(
            retained_output_snapshot_matches_previous(&state_dir, &state.output, &output).unwrap()
        );
        assert!(!output.exists());
    }

    #[test]
    fn file_identity_is_ambiguous_when_timestamp_overlaps_link_start() {
        let link_start = identity(0, 1, 2, 10, 10);
        let before = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 9, 9)),
        };
        let same_tick = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 10, 9)),
        };
        let changed_same_tick = FileContentState {
            len: 4,
            hash: String::new(),
            identity: Some(identity(4, 1, 2, 9, 10)),
        };

        assert!(!before.identity_is_ambiguous_since(Some(&link_start)));
        assert!(same_tick.identity_is_ambiguous_since(Some(&link_start)));
        assert!(changed_same_tick.identity_is_ambiguous_since(Some(&link_start)));
        assert!(!same_tick.identity_is_ambiguous_since(None));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn stable_identity_read_records_matching_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.o");
        std::fs::write(&path, b"abcd").unwrap();

        let (bytes, content) = read_file_with_stable_identity(&path).unwrap().unwrap();

        assert_eq!(bytes, b"abcd");
        assert_eq!(content, FileContentState::from_path(&path).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn stable_identity_read_can_defer_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("libcrate.rlib");
        std::fs::write(&path, b"abcd").unwrap();

        let (bytes, mut content) = read_file_with_stable_identity_and_hashing(&path, false)
            .unwrap()
            .unwrap();

        assert_eq!(bytes, b"abcd");
        assert!(content.hash.is_empty());
        assert_eq!(content.identity, FileIdentity::from_path(&path).unwrap());

        ensure_loaded_input_content_hash(&bytes, &mut content);

        assert_eq!(content, FileContentState::from_path(&path).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn stable_identity_read_selects_universal_macho_arm64_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.a");
        let payload = b"arm64 payload";
        let bytes = universal_macho_arm64_input(payload);
        std::fs::write(&path, &bytes).unwrap();

        let (selected, content) = read_file_with_stable_identity(&path).unwrap().unwrap();

        assert_eq!(selected, payload);
        assert_eq!(content, FileContentState::from_path(&path).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn verified_snapshot_selects_universal_macho_arm64_payload() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("out.incr");
        let path = dir.path().join("input.a");
        let payload = b"arm64 payload";
        let bytes = universal_macho_arm64_input(payload);
        std::fs::write(&path, &bytes).unwrap();
        snapshot_input_paths(&state_dir, [path.as_path()]).unwrap();
        let input = FileState {
            path: encode_path(&path),
            content: FileContentState::from_path(&path).unwrap(),
            snapshot_identity: None,
            patch: None,
        };

        assert_eq!(
            read_verified_input_snapshot(&state_dir, &input)
                .unwrap()
                .unwrap(),
            payload
        );
    }

    fn universal_macho_arm64_input(payload: &[u8]) -> Vec<u8> {
        const PAYLOAD_OFFSET: usize = 0x100;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&object::macho::FAT_MAGIC.to_be_bytes());
        bytes.extend_from_slice(&1_u32.to_be_bytes());
        bytes.extend_from_slice(&(object::macho::CPU_TYPE_ARM64 as u32).to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&(PAYLOAD_OFFSET as u32).to_be_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.resize(PAYLOAD_OFFSET, 0);
        bytes.extend_from_slice(payload);
        bytes
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn input_identity_mismatch_reason_rechecks_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.o");
        std::fs::write(&path, b"abcd").unwrap();
        let input = FileState {
            path: encode_path(&path),
            content: FileContentState::from_path_identity_only(&path).unwrap(),
            snapshot_identity: None,
            patch: None,
        };

        assert!(
            input_identity_mismatch_reason(std::slice::from_ref(&input))
                .unwrap()
                .is_none()
        );

        std::fs::write(&path, b"abcde").unwrap();
        let reason = input_identity_mismatch_reason(&[input]).unwrap().unwrap();

        assert!(reason.contains("input file changed while incremental fast path was running"));
        assert!(reason.contains("input.o"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn input_content_mismatch_reason_rechecks_changed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.o");
        std::fs::write(&path, b"abcd").unwrap();
        let expected = ExpectedInputContent::from_bytes(&path, b"abcd");

        assert!(input_content_mismatch_reason(std::slice::from_ref(&expected), None).is_none());

        std::fs::write(&path, b"wxyz").unwrap();
        let reason = input_content_mismatch_reason(&[expected], None).unwrap();

        assert!(reason.contains("input file changed while incremental fast path was running"));
        assert!(reason.contains("input.o"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_rust_input_skips_recheck_only_until_atomic_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("libcrate.rlib");
        std::fs::write(&path, b"object").unwrap();
        let (_, content) = read_file_with_stable_identity(&path).unwrap().unwrap();
        let expected = ExpectedInputContent::from_content(&path, &content);

        assert!(expected.matches_unchanged_atomic_replacement_input());
        assert!(input_content_mismatch_reason(std::slice::from_ref(&expected), None).is_none());

        let replacement = dir.path().join("replacement.rlib");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(&replacement, &path).unwrap();

        assert!(!expected.matches_unchanged_atomic_replacement_input());
        let reason = input_content_mismatch_reason(&[expected], None).unwrap();
        assert!(reason.contains("input file changed while incremental fast path was running"));
        assert!(reason.contains("libcrate.rlib"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn hashless_changed_rust_input_skips_recheck_only_until_atomic_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("libcrate.rlib");
        std::fs::write(&path, b"object").unwrap();
        let content = FileContentState::from_path_identity_only(&path).unwrap();
        let expected = ExpectedInputContent::from_content(&path, &content);

        assert!(expected.hash.is_empty());
        assert!(input_content_mismatch_reason(std::slice::from_ref(&expected), None).is_none());

        let replacement = dir.path().join("replacement.rlib");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(&replacement, &path).unwrap();

        let reason = input_content_mismatch_reason(&[expected], None).unwrap();
        assert!(reason.contains("input file changed while incremental fast path was running"));
        assert!(reason.contains("libcrate.rlib"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_rust_input_accepts_installed_hardlink_snapshot_until_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("app.incr");
        let path = dir.path().join("libcrate.rlib");
        std::fs::write(&path, b"object").unwrap();
        let (_, content) = read_file_with_stable_identity(&path).unwrap().unwrap();
        let expected = ExpectedInputContent::from_content(&path, &content);

        assert_eq!(
            snapshot_input_paths(&state_dir, [path.as_path()]).unwrap(),
            1
        );
        assert!(expected.matches_installed_atomic_replacement_snapshot(&state_dir));
        assert!(
            input_content_mismatch_reason(std::slice::from_ref(&expected), Some(&state_dir))
                .is_none()
        );

        let replacement = dir.path().join("replacement.rlib");
        std::fs::write(&replacement, b"changed").unwrap();
        std::fs::rename(&replacement, &path).unwrap();

        assert!(!expected.matches_installed_atomic_replacement_snapshot(&state_dir));
        let reason = input_content_mismatch_reason(&[expected], Some(&state_dir)).unwrap();
        assert!(reason.contains("input file changed while incremental fast path was running"));
        assert!(reason.contains("libcrate.rlib"));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn preloading_check_records_link_start_before_depfile_exists() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        args.dependency_file = Some(dir.path().join("out.d"));

        assert!(!maybe_reuse_output_before_loading(&args).unwrap());

        let state_dir = state_dir_for_output(&args.output);
        assert!(link_start_marker_identity(&state_dir).is_some());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn publishing_metadata_allows_exact_no_change_reuse_while_locked() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("input.o");
        let state_dir = state_dir_for_output(&output);
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"input").unwrap();

        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        let state = publishing_metadata_state(&args, &output, &input);
        state.write_publishing_index(&state_dir).unwrap();
        mark_incremental_update_started(&state_dir, "publishing").unwrap();
        let _lock = acquire_incremental_state_lock(&state_dir).unwrap();

        assert!(maybe_reuse_output_during_publication(&args, &state_dir).unwrap());
        let metadata = PersistedState::read_metadata(&state_dir).unwrap().unwrap();
        assert_eq!(
            metadata.sections_file.as_deref(),
            Some(PUBLISHING_SECTIONS_FILE)
        );
        assert!(metadata.patch_records_file.is_none());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn publishing_metadata_rejects_changed_input() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("input.o");
        let state_dir = state_dir_for_output(&output);
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"input").unwrap();

        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        let state = publishing_metadata_state(&args, &output, &input);
        state.write_publishing_index(&state_dir).unwrap();
        mark_incremental_update_started(&state_dir, "publishing").unwrap();
        std::fs::write(&input, b"changed").unwrap();

        assert!(!maybe_reuse_output_during_publication(&args, &state_dir).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn publishing_metadata_rejects_ambiguous_mutated_hardlink_input() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("crate.0123456789abcdef.rcgu.o");
        let state_dir = state_dir_for_output(&output);
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"input").unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        assert!(hardlink_rust_snapshot_bytes(&input, &snapshot));

        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        let mut state = publishing_metadata_state(&args, &output, &input);

        std::fs::write(&input, b"other").unwrap();
        state.input_files[0].content.identity = FileIdentity::from_path(&input).unwrap();
        state.input_files[0].snapshot_identity = FileIdentity::from_path(&snapshot).unwrap();
        state.link_start = state.input_files[0].content.identity.clone();
        state.write_publishing_index(&state_dir).unwrap();
        mark_incremental_update_started(&state_dir, "publishing").unwrap();

        assert!(!maybe_reuse_output_during_publication(&args, &state_dir).unwrap());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn preloading_rejects_ambiguous_mutated_hardlink_input() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let input = dir.path().join("crate.0123456789abcdef.rcgu.o");
        let state_dir = state_dir_for_output(&output);
        std::fs::write(&output, b"output").unwrap();
        std::fs::write(&input, b"input").unwrap();
        let snapshot = input_snapshot_path(&state_dir, &input);
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        assert!(hardlink_rust_snapshot_bytes(&input, &snapshot));

        let mut args = crate::args::elf::ElfArgs::default();
        args.common.incremental = true;
        args.output = Arc::from(output.as_path());
        let mut state = publishing_metadata_state(&args, &output, &input);

        std::fs::write(&input, b"other").unwrap();
        state.input_files[0].content.identity = FileIdentity::from_path(&input).unwrap();
        state.input_files[0].snapshot_identity = FileIdentity::from_path(&snapshot).unwrap();
        state.link_start = state.input_files[0].content.identity.clone();
        state.write(&state_dir).unwrap();

        assert!(!maybe_reuse_output_before_loading(&args).unwrap());
    }

    fn publishing_metadata_state(
        args: &crate::args::elf::ElfArgs,
        output: &Path,
        input: &Path,
    ) -> PersistedState {
        PersistedState {
            args_hash: args_hash(args),
            link_options_hash: Some(link_options_hash(args)),
            input_order_hash: Some(String::new()),
            sld_version: Some(sld_version(args)),
            link_start: FileIdentity::from_path(input).unwrap(),
            output: FileContentState::from_path_identity_only(output).unwrap(),
            build_id_hashes: None,
            input_files: vec![FileState {
                path: encode_path(input),
                content: FileContentState::from_path(input).unwrap(),
                snapshot_identity: None,
                patch: None,
            }],
            sections: Vec::new(),
            relocations: Vec::new(),
            macho_symbol_resolutions: Vec::new(),
            fdes: Vec::new(),
            dynamic_relocations: Vec::new(),
            sections_file: None,
            macho_symbol_resolutions_file: None,
            patch_records_file: None,
            patch_record_locations: Vec::new(),
            raw_patch_record_locations: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn incremental_state_lock_serializes_state_publication() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let first = acquire_incremental_state_lock(dir.path()).unwrap();
        let state_dir = dir.path().to_owned();
        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            attempting_tx.send(()).unwrap();
            let _second = acquire_incremental_state_lock(&state_dir).unwrap();
            acquired_tx.send(()).unwrap();
        });

        attempting_rx.recv().unwrap();
        assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn classifies_reusable_state() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert_eq!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Reuse
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn classifies_reusable_state_from_output_identity_without_hash() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let mut previous = state("args", b"stale", &[("a.o", b"a")]);
        previous.output = FileContentState::from_path_identity_only(&output).unwrap();
        assert!(previous.output.hash.is_empty());
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert_eq!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Reuse
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn classifies_missing_output_from_matching_retained_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert_eq!(
            classify_incremental_mode_with_output_policy(&output, &current, &previous, false, true,),
            IncrementalMode::Reuse
        );
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn replaced_hashless_output_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let mut previous = state("args", b"stale", &[("a.o", b"a")]);
        previous.output = FileContentState::from_path_identity_only(&output).unwrap();
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"output").unwrap();
        std::fs::rename(&replacement, &output).unwrap();
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "output file changed since previous link"
        ));
        assert!(matches!(
            classify_incremental_mode_with_output_policy(&output, &current, &previous, true, false),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "output file changed since previous link"
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn interrupted_update_marker_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let state_dir = dir.path().join("out.incr");
        std::fs::write(&output, b"output").unwrap();
        mark_incremental_update_started(&state_dir, "test").unwrap();

        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: state_dir.clone(),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "previous incremental update did not complete"
        ));

        clear_incremental_update_marker(&state_dir).unwrap();
        assert!(!update_marker_path(&state_dir).exists());
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_args_force_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "new-args".to_owned(),
            link_options_hash: "new-args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "linker arguments changed"
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_sld_version_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "new-sld".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "linker version changed"
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn missing_sld_version_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let mut previous = state("args", b"output", &[("a.o", b"a")]);
        previous.sld_version = None;
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "linker version missing from previous state"
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_input_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: state("args", b"output", &[("a.o", b"b")]).input_files,
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: true,
            } if reason.contains("input file changed")
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_input_list_keeps_unchanged_section_reuse_available() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let previous = state("old-exact-args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "new-exact-args".to_owned(),
            link_options_hash: "old-exact-args".to_owned(),
            input_order_hash: input_order_hash_for_paths(["a.o", "b.o"]),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: state("new-exact-args", b"output", &[("a.o", b"a"), ("b.o", b"b")])
                .input_files,
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: true,
            } if reason.contains("input file added")
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn removed_input_list_keeps_unchanged_section_reuse_available() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let previous = state("old-exact-args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "new-exact-args".to_owned(),
            link_options_hash: "old-exact-args".to_owned(),
            input_order_hash: input_order_hash_for_paths(["a.o"]),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: state("new-exact-args", b"output", &[("a.o", b"a")]).input_files,
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: true,
            } if reason.contains("input file removed")
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn reordered_input_list_keeps_unchanged_section_reuse_available() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let previous = state("old-exact-args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "new-exact-args".to_owned(),
            link_options_hash: "old-exact-args".to_owned(),
            input_order_hash: input_order_hash_for_paths(["b.o", "a.o"]),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: true,
            } if reason == "input file order changed"
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn missing_input_order_hash_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"output").unwrap();

        let mut previous = state("args", b"output", &[("a.o", b"a")]);
        previous.input_order_hash = None;
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: input_order_hash_for_paths(["a.o"]),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "input file order missing from previous state"
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn missing_output_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason.contains("output file could not be reused")
        ));
    }

    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    #[test]
    fn changed_output_forces_initial_link() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        std::fs::write(&output, b"changed").unwrap();
        let previous = state("args", b"output", &[("a.o", b"a")]);
        let current = CurrentState {
            state_dir: dir.path().join("out.incr"),
            args_hash: "args".to_owned(),
            link_options_hash: "args".to_owned(),
            input_order_hash: previous.input_order_hash.clone().unwrap(),
            sld_version: "sld-test".to_owned(),
            link_start: None,
            input_files: previous.input_files.clone(),
        };

        assert!(matches!(
            classify_incremental_mode(&output, &current, &previous),
            IncrementalMode::Relink {
                reason,
                can_reuse_unchanged_sections: false,
            } if reason == "output file changed since previous link"
        ));
    }

    #[test]
    fn reusable_inputs_only_include_unchanged_files() {
        let previous = state("args", b"output", &[("a.o", b"a"), ("b.o", b"b")]);
        let current = state("args", b"output", &[("a.o", b"a"), ("b.o", b"changed")]);

        let reusable = reusable_input_files(&current.input_files, &previous.input_files);

        assert!(reusable.contains(&hex::encode("a.o")));
        assert!(!reusable.contains(&hex::encode("b.o")));
    }

    #[test]
    fn patchable_sections_must_be_allocated() {
        let data = object::SectionFlags::Elf {
            sh_flags: u64::from(object::elf::SHF_ALLOC | object::elf::SHF_WRITE),
        };
        let text = object::SectionFlags::Elf {
            sh_flags: u64::from(object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR),
        };
        let rodata = object::SectionFlags::Elf {
            sh_flags: u64::from(object::elf::SHF_ALLOC),
        };
        let mergeable = object::SectionFlags::Elf {
            sh_flags: u64::from(
                object::elf::SHF_ALLOC | object::elf::SHF_WRITE | object::elf::SHF_MERGE,
            ),
        };
        let non_alloc = object::SectionFlags::Elf {
            sh_flags: u64::from(object::elf::SHF_WRITE),
        };
        let macho_regular = object::SectionFlags::MachO {
            flags: object::macho::S_REGULAR,
        };
        let macho_cstring = object::SectionFlags::MachO {
            flags: object::macho::S_CSTRING_LITERALS,
        };
        let macho_zerofill = object::SectionFlags::MachO {
            flags: object::macho::S_ZEROFILL,
        };

        assert!(section_flags_allow_patching(data));
        assert!(section_flags_allow_patching(text));
        assert!(section_flags_allow_patching(rodata));
        assert!(section_flags_allow_patching(mergeable));
        assert!(section_flags_allow_patching(macho_regular));
        assert!(section_flags_allow_patching(macho_cstring));
        assert!(!section_flags_allow_patching(macho_zerofill));
        assert!(!section_flags_allow_patching(non_alloc));
        assert!(!section_flags_allow_patching(object::SectionFlags::None));
    }

    #[test]
    fn fixed_layout_macho_patches_require_a_stable_input_size() {
        assert!(section_size_allows_direct_patching(
            Some(b"__cstring"),
            4,
            4
        ));
        assert!(!section_size_allows_direct_patching(
            Some(b"__cstring"),
            4,
            5
        ));
        assert!(section_size_allows_direct_patching(Some(b"__text"), 4, 4));
        assert!(!section_size_allows_direct_patching(Some(b"__text"), 4, 3));
        assert!(!section_size_allows_direct_patching(Some(b"__text"), 4, 5));
        assert!(section_size_allows_direct_patching(Some(b"__data"), 4, 5));
    }

    #[test]
    fn macho_relocation_kind_round_trips_recorded_flags() {
        let relocation = object::macho::RelocationInfo {
            r_address: 0x1234,
            r_symbolnum: 42,
            r_pcrel: true,
            r_length: 2,
            r_extern: true,
            r_type: object::macho::ARM64_RELOC_BRANCH26,
        };

        let decoded =
            decode_macho_aarch64_relocation_kind(encode_macho_aarch64_relocation_kind(relocation))
                .expect("Mach-O AArch64 relocation kind should decode");

        assert_eq!(decoded.r_address, 0);
        assert_eq!(decoded.r_symbolnum, 0);
        assert_eq!(decoded.r_pcrel, relocation.r_pcrel);
        assert_eq!(decoded.r_length, relocation.r_length);
        assert_eq!(decoded.r_extern, relocation.r_extern);
        assert_eq!(decoded.r_type, relocation.r_type);
    }

    #[test]
    fn macho_page_relocation_uses_target_and_place_pages() {
        assert_eq!(
            macho_aarch64_relocated_value(object::macho::ARM64_RELOC_PAGE21, 0x5000, 0, 0x2ffc,),
            Some(0x3000)
        );
    }

    #[test]
    fn macho_branch_and_pageoff_relocations_use_full_values() {
        assert_eq!(
            macho_aarch64_relocated_value(object::macho::ARM64_RELOC_BRANCH26, 0x4000, 0, 0x1004,),
            Some(0x2ffc)
        );
        assert_eq!(
            macho_aarch64_relocated_value(
                object::macho::ARM64_RELOC_PAGEOFF12,
                0x1234_5000,
                0x678,
                0,
            ),
            Some(0x1234_5678)
        );
    }

    #[test]
    fn macho_indirect_branch_targets_ignore_symbol_addends() {
        assert_eq!(
            macho_aarch64_applied_target_addend(
                object::macho::ARM64_RELOC_BRANCH26,
                0x4000,
                0x4000,
                4,
            ),
            4
        );
        assert_eq!(
            macho_aarch64_applied_target_addend(
                object::macho::ARM64_RELOC_BRANCH26,
                0x4000,
                0x8000,
                4,
            ),
            0
        );
        assert_eq!(
            macho_aarch64_applied_target_addend(
                object::macho::ARM64_RELOC_PAGEOFF12,
                0x4000,
                0x8000,
                4,
            ),
            4
        );
    }

    #[test]
    fn macho_cross_input_relocations_follow_target_moves() {
        assert!(macho_aarch64_cross_input_relocation_is_supported(
            object::macho::RelocationInfo {
                r_address: 0,
                r_symbolnum: 0,
                r_pcrel: true,
                r_length: 2,
                r_extern: true,
                r_type: object::macho::ARM64_RELOC_BRANCH26,
            },
            4,
        ));
        assert!(!macho_aarch64_cross_input_relocation_is_supported(
            object::macho::RelocationInfo {
                r_address: 0,
                r_symbolnum: 0,
                r_pcrel: false,
                r_length: 2,
                r_extern: true,
                r_type: object::macho::ARM64_RELOC_BRANCH26,
            },
            4,
        ));
        assert_eq!(
            macho_aarch64_relocated_value_after_target_move(
                object::macho::ARM64_RELOC_BRANCH26,
                0x1000,
                0x2000,
                0x2004,
                0,
            ),
            Some(0x1004)
        );
        assert_eq!(
            macho_aarch64_relocated_value_after_target_move(
                object::macho::ARM64_RELOC_PAGE21,
                (-0x4000_i64) as u64,
                0x1ffc,
                0x2004,
                0,
            ),
            Some((-0x3000_i64) as u64)
        );
        assert_eq!(
            macho_aarch64_relocated_value_after_target_move(
                object::macho::ARM64_RELOC_PAGEOFF12,
                0x1ffc,
                0x1ffc,
                0x2004,
                0,
            ),
            Some(0x2004)
        );
    }

    #[test]
    fn macho_cross_input_branch_patch_encodes_the_new_displacement() {
        let raw_relocation = object::macho::RelocationInfo {
            r_address: 0,
            r_symbolnum: 0,
            r_pcrel: true,
            r_length: 2,
            r_extern: true,
            r_type: object::macho::ARM64_RELOC_BRANCH26,
        };
        let rel_info =
            <crate::macho_aarch64::MachOAArch64 as crate::platform::Arch>::relocation_from_raw(
                raw_relocation,
            )
            .unwrap();
        let previous_output = [0xfd, 0xff, 0xff, 0x17];
        let mut data = vec![0; previous_output.len()];

        materialize_deferred_relocation_patch(
            &mut data,
            &previous_output,
            DeferredRelocationPatch {
                rel_info,
                previous_written_value: (-12_i64) as u64,
                written_value: (-8_i64) as u64,
            },
        )
        .unwrap();

        assert_eq!(data, [0xfe, 0xff, 0xff, 0x17]);
    }

    #[test]
    fn macho_cross_input_target_lookup_rejects_duplicate_defined_names() {
        let mut bytes = test_macho_object(b"\0\0\0\0", b"\0\0\0\0", 0);
        const SYMTAB_COMMAND_OFFSET: usize = 32 + 72 + 2 * 80;
        let symoff = u32::from_le_bytes(
            bytes[SYMTAB_COMMAND_OFFSET + 8..SYMTAB_COMMAND_OFFSET + 12]
                .try_into()
                .unwrap(),
        ) as usize;
        let stroff = u32::from_le_bytes(
            bytes[SYMTAB_COMMAND_OFFSET + 16..SYMTAB_COMMAND_OFFSET + 20]
                .try_into()
                .unwrap(),
        ) as usize;
        let duplicate_symbol = bytes[symoff + 16..symoff + 32].to_vec();
        bytes.splice(stroff..stroff, duplicate_symbol);
        bytes[SYMTAB_COMMAND_OFFSET + 12..SYMTAB_COMMAND_OFFSET + 16]
            .copy_from_slice(&3_u32.to_le_bytes());
        bytes[SYMTAB_COMMAND_OFFSET + 16..SYMTAB_COMMAND_OFFSET + 20]
            .copy_from_slice(&((stroff + 16) as u32).to_le_bytes());
        let file = object::File::parse(bytes.as_slice()).unwrap();

        let result =
            unique_symbol_position_by_name(&bytes, 0, &file, &hex::encode(b"_data")).unwrap();

        assert!(matches!(
            result,
            Err(reason) if reason == "ambiguous cross-input Mach-O relocation target symbol"
        ));
    }

    #[test]
    fn cstring_patches_require_stable_literal_boundaries() {
        assert!(cstring_literal_boundaries_are_stable(
            b"first\0second\0",
            b"FIRST\0SECOND\0"
        ));
        assert!(!cstring_literal_boundaries_are_stable(
            b"first\0second\0",
            b"first second\0"
        ));
        assert!(!cstring_literal_boundaries_are_stable(
            b"first\0",
            b"first\0\0"
        ));
        assert_eq!(
            cstring_nul_boundaries_hash(b"first\0second\0"),
            cstring_nul_boundaries_hash(b"FIRST\0SECOND\0")
        );
        assert_ne!(
            cstring_nul_boundaries_hash(b"first\0second\0"),
            cstring_nul_boundaries_hash(b"first second\0")
        );
    }

    #[test]
    fn build_id_hash_tree_matches_full_hash() {
        for len in [
            BUILD_ID_HASH_GROUP_LEN + 1,
            2 * BUILD_ID_HASH_GROUP_LEN,
            2 * BUILD_ID_HASH_GROUP_LEN + 17,
            5 * BUILD_ID_HASH_GROUP_LEN + 100,
        ] {
            let output = (0..len).map(|i| (i % 251) as u8).collect::<Vec<_>>();
            let build_id_range = 100..148;
            let nodes = build_id_hash_node_count(output.len()).unwrap();
            let tree = build_id_hash_tree(&output, &build_id_range).unwrap();
            let state = BuildIdHashState {
                output_len: output.len() as u64,
                nodes,
                tree_hash: Some(build_id_hash_tree_hash(&tree)),
            };
            let mut expected = output;
            expected[build_id_range].fill(0);

            assert_eq!(tree.len(), nodes);
            assert_eq!(
                build_id_from_hash_tree(&state, &tree).unwrap(),
                blake3::hash(&expected)
            );
        }
    }

    #[test]
    fn build_id_hash_tree_updates_changed_chunks() {
        let mut output = (0..5 * BUILD_ID_HASH_GROUP_LEN + 100)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        let build_id_range = 1500..1548;
        let nodes = build_id_hash_node_count(output.len()).unwrap();
        let mut tree = build_id_hash_tree(&output, &build_id_range).unwrap();
        let mut state = BuildIdHashState {
            output_len: output.len() as u64,
            nodes,
            tree_hash: Some(build_id_hash_tree_hash(&tree)),
        };

        let changed_range = 2 * BUILD_ID_HASH_GROUP_LEN + 100..2 * BUILD_ID_HASH_GROUP_LEN + 110;
        output[changed_range.clone()].copy_from_slice(b"0123456789");
        let changed_chunks = touched_build_id_chunks(&[changed_range], output.len()).unwrap();
        assert!(update_build_id_hash_tree(
            &mut state,
            &mut tree,
            &output,
            &build_id_range,
            &changed_chunks,
        ));
        let mut expected = output;
        expected[build_id_range].fill(0);

        assert_eq!(
            build_id_from_hash_tree(&state, &tree).unwrap(),
            blake3::hash(&expected)
        );
        assert_eq!(
            state.tree_hash.as_deref(),
            Some(build_id_hash_tree_hash(&tree).as_str())
        );
    }

    #[test]
    #[cfg_attr(target_os = "wasi", ignore = "wasi doesn't have a temp dir")]
    fn build_id_hash_tree_must_match_state_hash() {
        let dir = tempfile::tempdir().unwrap();
        let output_len = 5 * BUILD_ID_HASH_GROUP_LEN + 100;
        let nodes = build_id_hash_node_count(output_len).unwrap();
        let tree = vec![[1; blake3::OUT_LEN]; nodes];
        write_build_id_hash_tree(dir.path(), Some(&tree)).unwrap();
        let state = BuildIdHashState {
            output_len: output_len as u64,
            nodes,
            tree_hash: Some("wrong-hash".to_owned()),
        };

        let error = read_build_id_hash_tree(dir.path(), &state).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not match its recorded hash")
        );
    }

    #[test]
    fn try_reuse_section_requires_unchanged_input_and_matching_record() {
        let mut input_file = crate::input_data::InputFile::for_testing();
        input_file.filename = PathBuf::from("a.o");
        let input = InputRef {
            file: &input_file,
            entry: None,
        };
        let record = SectionRecord::new(input, object::SectionIndex(3), 64, 16);
        let state = PreparedState {
            mode: IncrementalMode::Relink {
                reason: "input file changed: b.o".to_owned(),
                can_reuse_unchanged_sections: true,
            },
            current: CurrentState {
                state_dir: PathBuf::new(),
                args_hash: "args".to_owned(),
                link_options_hash: "args".to_owned(),
                input_order_hash: String::new(),
                sld_version: "sld-test".to_owned(),
                link_start: None,
                input_files: Vec::new(),
            },
            retained_output_to_restore: None,
            reusable_inputs: [encode_path(Path::new("a.o"))].into_iter().collect(),
            previous_sections: [record].into_iter().collect(),
            previous_relocations: Vec::new(),
            previous_macho_symbol_resolutions_file: None,
            previous_fdes: Vec::new(),
            previous_dynamic_relocations: Vec::new(),
            current_sections: RecordBuffers::default(),
            current_relocations: RecordBuffers::default(),
            current_macho_symbol_resolutions: RecordBuffers::default(),
            current_fdes: RecordBuffers::default(),
            current_dynamic_relocations: RecordBuffers::default(),
            record_texts: RecordTextInterner::default(),
            reused_sections: AtomicUsize::new(0),
            prepared_fast_build_id_state: Mutex::new(None),
        };

        assert!(state.try_reuse_section(input, object::SectionIndex(3), 64, 16, true, true));
        assert!(!state.try_reuse_section(input, object::SectionIndex(3), 80, 16, true, true));
        assert_eq!(state.reused_sections.load(Ordering::Relaxed), 1);
        assert_eq!(state.current_sections.take_all().len(), 2);
    }

    #[test]
    fn try_reuse_section_skips_non_reusable_records() {
        let mut input_file = crate::input_data::InputFile::for_testing();
        input_file.filename = PathBuf::from("a.o");
        let input = InputRef {
            file: &input_file,
            entry: None,
        };
        let state = PreparedState {
            mode: IncrementalMode::Relink {
                reason: "no previous incremental state".to_owned(),
                can_reuse_unchanged_sections: false,
            },
            current: CurrentState {
                state_dir: PathBuf::new(),
                args_hash: "args".to_owned(),
                link_options_hash: "args".to_owned(),
                input_order_hash: String::new(),
                sld_version: "sld-test".to_owned(),
                link_start: None,
                input_files: Vec::new(),
            },
            retained_output_to_restore: None,
            reusable_inputs: HashSet::new(),
            previous_sections: HashSet::new(),
            previous_relocations: Vec::new(),
            previous_macho_symbol_resolutions_file: None,
            previous_fdes: Vec::new(),
            previous_dynamic_relocations: Vec::new(),
            current_sections: RecordBuffers::default(),
            current_relocations: RecordBuffers::default(),
            current_macho_symbol_resolutions: RecordBuffers::default(),
            current_fdes: RecordBuffers::default(),
            current_dynamic_relocations: RecordBuffers::default(),
            record_texts: RecordTextInterner::default(),
            reused_sections: AtomicUsize::new(0),
            prepared_fast_build_id_state: Mutex::new(None),
        };

        assert!(!state.try_reuse_section(input, object::SectionIndex(3), 64, 16, false, true));
        assert!(state.current_sections.take_all().is_empty());
    }

    #[test]
    fn record_generated_section_records_non_empty_ranges() {
        let state = PreparedState {
            mode: IncrementalMode::Relink {
                reason: "no previous incremental state".to_owned(),
                can_reuse_unchanged_sections: false,
            },
            current: CurrentState {
                state_dir: PathBuf::new(),
                args_hash: "args".to_owned(),
                link_options_hash: "args".to_owned(),
                input_order_hash: String::new(),
                sld_version: "sld-test".to_owned(),
                link_start: None,
                input_files: Vec::new(),
            },
            retained_output_to_restore: None,
            reusable_inputs: HashSet::new(),
            previous_sections: HashSet::new(),
            previous_relocations: Vec::new(),
            previous_macho_symbol_resolutions_file: None,
            previous_fdes: Vec::new(),
            previous_dynamic_relocations: Vec::new(),
            current_sections: RecordBuffers::default(),
            current_relocations: RecordBuffers::default(),
            current_macho_symbol_resolutions: RecordBuffers::default(),
            current_fdes: RecordBuffers::default(),
            current_dynamic_relocations: RecordBuffers::default(),
            record_texts: RecordTextInterner::default(),
            reused_sections: AtomicUsize::new(0),
            prepared_fast_build_id_state: Mutex::new(None),
        };

        state.record_generated_section("generated:.rela.dyn.general", 256, 24);
        state.record_generated_section("generated:.relr.dyn", 512, 0);

        assert_eq!(
            state.current_sections.take_all(),
            vec![generated_section_record(
                "generated:.rela.dyn.general",
                256,
                24
            )]
        );
    }

    #[test]
    fn record_eh_frame_fde_records_non_empty_ranges() {
        let mut input_file = crate::input_data::InputFile::for_testing();
        input_file.filename = PathBuf::from("a.o");
        let input = InputRef {
            file: &input_file,
            entry: None,
        };
        let state = PreparedState {
            mode: IncrementalMode::Relink {
                reason: "no previous incremental state".to_owned(),
                can_reuse_unchanged_sections: false,
            },
            current: CurrentState {
                state_dir: PathBuf::new(),
                args_hash: "args".to_owned(),
                link_options_hash: "args".to_owned(),
                input_order_hash: String::new(),
                sld_version: "sld-test".to_owned(),
                link_start: None,
                input_files: Vec::new(),
            },
            retained_output_to_restore: None,
            reusable_inputs: HashSet::new(),
            previous_sections: HashSet::new(),
            previous_relocations: Vec::new(),
            previous_macho_symbol_resolutions_file: None,
            previous_fdes: Vec::new(),
            previous_dynamic_relocations: Vec::new(),
            current_sections: RecordBuffers::default(),
            current_relocations: RecordBuffers::default(),
            current_macho_symbol_resolutions: RecordBuffers::default(),
            current_fdes: RecordBuffers::default(),
            current_dynamic_relocations: RecordBuffers::default(),
            record_texts: RecordTextInterner::default(),
            reused_sections: AtomicUsize::new(0),
            prepared_fast_build_id_state: Mutex::new(None),
        };

        state.record_eh_frame_fde(
            input,
            object::SectionIndex(3),
            object::SectionIndex(5),
            32,
            256,
            24,
        );
        state.record_eh_frame_fde(
            input,
            object::SectionIndex(3),
            object::SectionIndex(5),
            56,
            280,
            0,
        );

        assert_eq!(
            state.current_fdes.take_all(),
            vec![FdeRecord::new(
                input,
                object::SectionIndex(3),
                object::SectionIndex(5),
                32,
                256,
                24
            )]
        );
    }

    #[test]
    fn deferred_relocation_records_materialize_non_empty_ranges() {
        let mut input_file = crate::input_data::InputFile::for_testing();
        input_file.filename = PathBuf::from("a.o");
        let input = InputRef {
            file: &input_file,
            entry: None,
        };
        let state = PreparedState {
            mode: IncrementalMode::Relink {
                reason: "no previous incremental state".to_owned(),
                can_reuse_unchanged_sections: false,
            },
            current: CurrentState {
                state_dir: PathBuf::new(),
                args_hash: "args".to_owned(),
                link_options_hash: "args".to_owned(),
                input_order_hash: String::new(),
                sld_version: "sld-test".to_owned(),
                link_start: None,
                input_files: Vec::new(),
            },
            retained_output_to_restore: None,
            reusable_inputs: HashSet::new(),
            previous_sections: HashSet::new(),
            previous_relocations: Vec::new(),
            previous_macho_symbol_resolutions_file: None,
            previous_fdes: Vec::new(),
            previous_dynamic_relocations: Vec::new(),
            current_sections: RecordBuffers::default(),
            current_relocations: RecordBuffers::default(),
            current_macho_symbol_resolutions: RecordBuffers::default(),
            current_fdes: RecordBuffers::default(),
            current_dynamic_relocations: RecordBuffers::default(),
            record_texts: RecordTextInterner::default(),
            reused_sections: AtomicUsize::new(0),
            prepared_fast_build_id_state: Mutex::new(None),
        };

        let target_metadata_calls = AtomicUsize::new(0);
        let records = [
            PreparedState::deferred_relocation_record(
                input,
                object::SectionIndex(3),
                42,
                8,
                256,
                4,
                2,
                -16,
                0x5678,
                0x1234,
                || {
                    target_metadata_calls.fetch_add(1, Ordering::Relaxed);
                    Ok((
                        Some(b"target".as_slice()),
                        Some((input, object::SectionIndex(7), 32)),
                    ))
                },
            )
            .unwrap(),
            PreparedState::deferred_relocation_record(
                input,
                object::SectionIndex(3),
                42,
                12,
                268,
                4,
                2,
                -8,
                0x5680,
                0x1234,
                || {
                    target_metadata_calls.fetch_add(1, Ordering::Relaxed);
                    Ok((
                        Some(b"target".as_slice()),
                        Some((input, object::SectionIndex(7), 32)),
                    ))
                },
            )
            .unwrap(),
            PreparedState::deferred_relocation_record(
                input,
                object::SectionIndex(3),
                43,
                16,
                280,
                0,
                2,
                0,
                0,
                0x5678,
                || Ok((None, None)),
            )
            .unwrap(),
        ]
        .into_iter()
        .flatten()
        .collect();
        state.record_relocations(records);

        assert_eq!(target_metadata_calls.load(Ordering::Relaxed), 2);
        let record_texts = RecordTextInterner::default();
        let records = state
            .current_relocations
            .take_all()
            .into_iter()
            .map(|record| record.materialize(&record_texts).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            records,
            vec![
                RelocationRecord::new(
                    input,
                    object::SectionIndex(3),
                    42,
                    8,
                    256,
                    4,
                    2,
                    -16,
                    0x5678,
                    0x1234,
                    Some(hex::encode("target")),
                    Some((input, object::SectionIndex(7), 32))
                ),
                RelocationRecord::new(
                    input,
                    object::SectionIndex(3),
                    42,
                    12,
                    268,
                    4,
                    2,
                    -8,
                    0x5680,
                    0x1234,
                    Some(hex::encode("target")),
                    Some((input, object::SectionIndex(7), 32))
                ),
            ]
        );
    }

    #[test]
    fn record_dynamic_relocation_records_non_empty_ranges() {
        let mut input_file = crate::input_data::InputFile::for_testing();
        input_file.filename = PathBuf::from("a.o");
        let input = InputRef {
            file: &input_file,
            entry: None,
        };
        let state = PreparedState {
            mode: IncrementalMode::Relink {
                reason: "no previous incremental state".to_owned(),
                can_reuse_unchanged_sections: false,
            },
            current: CurrentState {
                state_dir: PathBuf::new(),
                args_hash: "args".to_owned(),
                link_options_hash: "args".to_owned(),
                input_order_hash: String::new(),
                sld_version: "sld-test".to_owned(),
                link_start: None,
                input_files: Vec::new(),
            },
            retained_output_to_restore: None,
            reusable_inputs: HashSet::new(),
            previous_sections: HashSet::new(),
            previous_relocations: Vec::new(),
            previous_macho_symbol_resolutions_file: None,
            previous_fdes: Vec::new(),
            previous_dynamic_relocations: Vec::new(),
            current_sections: RecordBuffers::default(),
            current_relocations: RecordBuffers::default(),
            current_macho_symbol_resolutions: RecordBuffers::default(),
            current_fdes: RecordBuffers::default(),
            current_dynamic_relocations: RecordBuffers::default(),
            record_texts: RecordTextInterner::default(),
            reused_sections: AtomicUsize::new(0),
            prepared_fast_build_id_state: Mutex::new(None),
        };

        state.record_dynamic_relocation_with_output_info(
            input,
            object::SectionIndex(3),
            8,
            256,
            24,
            None,
        );
        state.record_dynamic_relocation_with_output_info(
            input,
            object::SectionIndex(3),
            16,
            280,
            0,
            None,
        );

        assert_eq!(
            state.current_dynamic_relocations.take_all(),
            vec![DynamicRelocationRecord::new(
                input,
                object::SectionIndex(3),
                8,
                256,
                24,
                None
            )]
        );
    }

    #[test]
    fn patch_output_ranges_must_not_overlap() {
        let patch = |output_offset, size| SectionPatch {
            output_offset,
            size,
            data: Vec::new(),
            deferred_relocation: None,
            preserve_ranges: Vec::new(),
            adjustments: Vec::new(),
        };

        assert!(patch_output_range_rejection_reason(&[patch(16, 8), patch(24, 8)]).is_none());
        assert!(patch_output_range_rejection_reason(&[patch(24, 8), patch(16, 8)]).is_none());
        assert_eq!(
            patch_output_range_rejection_reason(&[patch(16, 8), patch(23, 8)]).as_deref(),
            Some("changed patch output ranges overlap")
        );
        assert_eq!(
            patch_output_range_rejection_reason(&[patch(usize::MAX as u64, 8)]).as_deref(),
            Some("changed patch output range overflow")
        );
    }
}
