//! Exact-path manifests for thin target-directory snapshots.
//!
//! A snapshot manifest records target outputs that can be reconstructed from
//! the shared artifact cache. Restoring one deliberately copies cache files:
//! target outputs must never share writable inodes with cache entries.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

use super::build_runner::OutputFile;
use super::{
    lock_rlib_cache_for_restore, rlib_cache_digest, verified_rlib_cache_manifest,
    verify_rlib_cache_manifest_files,
};
use crate::util::CargoResult;

const SNAPSHOT_VERSION: u32 = 1;
const PERMISSION_MODE_MASK: u32 = 0o7777;

/// Collects artifact-cache-backed target outputs for a thin snapshot manifest.
pub struct Recorder {
    output_path: PathBuf,
    target_root: PathBuf,
    cache_root: PathBuf,
    pending: Mutex<Vec<PendingRecord>>,
}

#[derive(Clone, Debug)]
struct PendingRecord {
    cache_entry: PathBuf,
    target_output: PathBuf,
}

/// Counts files handled by [`restore`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestoreSummary {
    pub cloned_files: u64,
    pub cloned_bytes: u64,
    pub copied_files: u64,
    pub copied_bytes: u64,
    pub existing_files: u64,
    pub existing_bytes: u64,
}

/// Counts target outputs certified by [`Recorder::write`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ManifestSummary {
    pub files: u64,
    pub logical_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotManifest {
    version: u32,
    target_root: String,
    records: Vec<SnapshotRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRecord {
    target_path: String,
    cache_entry: String,
    cache_filename: String,
    cache_digest: String,
    mode: u32,
    size: u64,
    mtime_sec: i64,
    mtime_nsec: u32,
}

struct RestorePlan {
    target: PathBuf,
    stored: PathBuf,
    record: SnapshotRecord,
    already_exists: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReconstructionKind {
    Cloned,
    Copied,
}

impl Recorder {
    pub fn new(
        output_path: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
        cache_root: impl Into<PathBuf>,
    ) -> CargoResult<Self> {
        let target_root = target_root.into();
        let cache_root = cache_root.into();
        validate_absolute_root(&target_root, "target root")?;
        validate_absolute_root(&cache_root, "artifact cache root")?;
        Ok(Self {
            output_path: output_path.into(),
            target_root,
            cache_root,
            pending: Mutex::new(Vec::new()),
        })
    }

    /// Removes any manifest left by an earlier invocation before build work
    /// starts, so a failed population build cannot leave stale output behind.
    pub fn invalidate_output(&self) -> CargoResult<()> {
        match fs::symlink_metadata(&self.output_path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                fs::remove_file(&self.output_path).with_context(|| {
                    format!(
                        "failed to invalidate old artifact cache snapshot manifest `{}`",
                        self.output_path.display()
                    )
                })?;
            }
            Ok(_) => bail!(
                "refusing to replace non-regular artifact cache snapshot manifest `{}`",
                self.output_path.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect artifact cache snapshot manifest `{}`",
                        self.output_path.display()
                    )
                });
            }
        }
        Ok(())
    }

    /// Records outputs backed by an accepted artifact-cache entry.
    ///
    /// Cache jobs may finish in parallel, so this method is infallible and
    /// synchronized. [`Self::write`] performs all filesystem validation after
    /// the build has stopped mutating these outputs.
    pub fn record(&self, cache_entry: &Path, outputs: &[OutputFile]) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.extend(outputs.iter().map(|output| PendingRecord {
            cache_entry: cache_entry.to_path_buf(),
            target_output: output.path.clone(),
        }));
    }

    /// Validates recorded outputs and atomically publishes a deterministic
    /// manifest.
    pub fn write(&self) -> CargoResult<ManifestSummary> {
        validate_absolute_root(&self.target_root, "target root")?;
        validate_absolute_root(&self.cache_root, "artifact cache root")?;
        validate_directory_no_follow(&self.target_root, "target root")?;
        validate_directory_no_follow(&self.cache_root, "artifact cache root")?;
        let target_root = path_as_utf8(&self.target_root, "target root")?;
        let Some(_cache_lock) = lock_rlib_cache_for_restore(&self.cache_root)? else {
            bail!("could not lock the artifact cache while writing a snapshot manifest");
        };

        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        pending.sort_by(|left, right| {
            (&left.target_output, &left.cache_entry)
                .cmp(&(&right.target_output, &right.cache_entry))
        });

        let mut records = BTreeMap::<String, SnapshotRecord>::new();
        for pending in pending {
            let target_relative = pending
                .target_output
                .strip_prefix(&self.target_root)
                .with_context(|| {
                    format!(
                        "snapshot target output `{}` is outside target root `{}`",
                        pending.target_output.display(),
                        self.target_root.display()
                    )
                })?;
            let target_path = encode_safe_relative(target_relative, "target output")?;

            let cache_relative = pending
                .cache_entry
                .strip_prefix(&self.cache_root)
                .with_context(|| {
                    format!(
                        "snapshot cache entry `{}` is outside artifact cache root `{}`",
                        pending.cache_entry.display(),
                        self.cache_root.display()
                    )
                })?;
            let cache_entry = encode_safe_relative(cache_relative, "cache entry")?;
            if let Err(error) = validate_descendant_directory_no_follow(
                &self.cache_root,
                cache_relative,
                "artifact cache entry",
            ) {
                tracing::debug!(
                    "omitting unavailable artifact cache snapshot entry {}: {error:#}",
                    pending.cache_entry.display()
                );
                continue;
            }

            let filename = pending.target_output.file_name().ok_or_else(|| {
                anyhow::anyhow!(
                    "snapshot target output has no filename: {}",
                    pending.target_output.display()
                )
            })?;
            let cache_filename = encode_safe_filename(filename, "cache filename")?;
            if let Err(error) = validate_directory_no_follow(
                &pending.cache_entry.join("files"),
                "artifact cache files directory",
            ) {
                tracing::debug!(
                    "omitting unavailable artifact cache snapshot files {}: {error:#}",
                    pending.cache_entry.display()
                );
                continue;
            }
            let stored = pending.cache_entry.join("files").join(filename);
            let expected = match verified_rlib_cache_manifest(&pending.cache_entry) {
                Ok(Some(expected)) => expected,
                Ok(None) => continue,
                Err(error) => {
                    tracing::debug!(
                        "omitting unreadable artifact cache snapshot entry {}: {error:#}",
                        pending.cache_entry.display()
                    );
                    continue;
                }
            };
            match verify_rlib_cache_manifest_files(
                &pending.cache_entry,
                &expected,
                [stored.clone()],
            ) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    tracing::debug!(
                        "omitting unreadable artifact cache snapshot output {}: {error:#}",
                        stored.display()
                    );
                    continue;
                }
            }
            let stored_manifest_path = stored
                .strip_prefix(&pending.cache_entry)
                .expect("stored cache file is below its entry")
                .to_string_lossy();
            let Some(cache_digest) = expected.get(stored_manifest_path.as_ref()).cloned() else {
                continue;
            };
            if validate_cache_digest(&cache_digest).is_err() {
                continue;
            }

            let Ok(output_metadata) =
                regular_file_metadata(&pending.target_output, "target output")
            else {
                continue;
            };
            let Ok(output_digest) = rlib_cache_digest(&pending.target_output) else {
                continue;
            };
            if output_digest != cache_digest {
                continue;
            }
            if !fs::metadata(&stored).is_ok_and(|metadata| metadata.len() == output_metadata.len())
            {
                continue;
            }

            let mtime = filetime::FileTime::from_last_modification_time(&output_metadata);
            let record = SnapshotRecord {
                target_path: target_path.clone(),
                cache_entry,
                cache_filename,
                cache_digest,
                mode: metadata_mode(&output_metadata),
                size: output_metadata.len(),
                mtime_sec: mtime.unix_seconds(),
                mtime_nsec: mtime.nanoseconds(),
            };
            match records.entry(target_path) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(record);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    // Multiple accepted entries can name an identical output.
                    // Choose one deterministically rather than depending on job
                    // completion order.
                    if record < *entry.get() {
                        entry.insert(record);
                    }
                }
            }
        }

        let manifest = SnapshotManifest {
            version: SNAPSHOT_VERSION,
            target_root,
            records: records.into_values().collect(),
        };
        let contents = manifest_json(&manifest)?;
        let summary = ManifestSummary {
            files: manifest.records.len() as u64,
            logical_bytes: manifest.records.iter().map(|record| record.size).sum(),
        };
        cargo_util::paths::write_atomic_no_follow(&self.output_path, contents).with_context(
            || {
                format!(
                    "failed to write artifact cache snapshot manifest `{}`",
                    self.output_path.display()
                )
            },
        )?;
        Ok(summary)
    }
}

/// Restores cache-backed outputs described by an exact-path manifest.
///
/// Every cache entry and existing target is validated before the first target
/// output is published. Missing outputs are copied through a temporary file in
/// the destination directory and installed without replacing an existing path.
pub fn restore(
    manifest_path: &Path,
    target_root: &Path,
    cache_root: &Path,
) -> CargoResult<RestoreSummary> {
    validate_absolute_root(target_root, "target root")?;
    validate_absolute_root(cache_root, "artifact cache root")?;
    let target_root_text = path_as_utf8(target_root, "target root")?;
    let contents = fs::read(manifest_path).with_context(|| {
        format!(
            "failed to read artifact cache snapshot manifest `{}`",
            manifest_path.display()
        )
    })?;
    let manifest: SnapshotManifest = serde_json::from_slice(&contents).with_context(|| {
        format!(
            "failed to parse artifact cache snapshot manifest `{}`",
            manifest_path.display()
        )
    })?;
    if manifest.version != SNAPSHOT_VERSION {
        bail!(
            "unsupported artifact cache snapshot manifest version {}",
            manifest.version
        );
    }
    if manifest.target_root != target_root_text {
        bail!(
            "artifact cache snapshot target root mismatch: manifest has `{}`, requested `{}`",
            manifest.target_root,
            target_root.display()
        );
    }
    validate_directory_no_follow(target_root, "target root")?;
    validate_directory_no_follow(cache_root, "artifact cache root")?;
    let Some(_cache_lock) = lock_rlib_cache_for_restore(cache_root)? else {
        bail!("could not lock the artifact cache while restoring a snapshot manifest");
    };

    let mut plans = Vec::with_capacity(manifest.records.len());
    let mut targets = BTreeMap::<String, ()>::new();
    for record in manifest.records {
        validate_snapshot_record(&record)?;
        if targets.insert(record.target_path.clone(), ()).is_some() {
            bail!(
                "artifact cache snapshot repeats target path `{}`",
                record.target_path
            );
        }

        let target_relative = decode_safe_relative(&record.target_path, "target path")?;
        let cache_relative = decode_safe_relative(&record.cache_entry, "cache entry")?;
        let cache_filename = decode_safe_filename(&record.cache_filename, "cache filename")?;
        if target_relative.file_name() != Some(cache_filename.as_os_str()) {
            bail!(
                "artifact cache snapshot target `{}` does not match cache filename `{}`",
                record.target_path,
                record.cache_filename
            );
        }

        let target = target_root.join(&target_relative);
        let cache_entry = cache_root.join(cache_relative);
        validate_descendant_directory_no_follow(
            cache_root,
            cache_entry
                .strip_prefix(cache_root)
                .expect("cache entry is below cache root"),
            "artifact cache entry",
        )?;
        validate_directory_no_follow(&cache_entry.join("files"), "artifact cache files directory")?;
        let stored = cache_entry.join("files").join(&cache_filename);
        let expected = verified_rlib_cache_manifest(&cache_entry)?.ok_or_else(|| {
            anyhow::anyhow!(
                "artifact cache entry is incomplete or has an invalid manifest: {}",
                cache_entry.display()
            )
        })?;
        let stored_manifest_path = stored
            .strip_prefix(&cache_entry)
            .expect("stored cache file is below its entry")
            .to_string_lossy();
        if expected.get(stored_manifest_path.as_ref()) != Some(&record.cache_digest) {
            bail!(
                "artifact cache snapshot digest does not match cache manifest for `{}`",
                stored.display()
            );
        }
        if !verify_rlib_cache_manifest_files(&cache_entry, &expected, [stored.clone()])? {
            bail!(
                "artifact cache snapshot references corrupt cache file `{}`",
                stored.display()
            );
        }
        let stored_metadata = regular_file_metadata(&stored, "artifact cache file")?;
        if stored_metadata.len() != record.size {
            bail!(
                "artifact cache snapshot metadata does not match cache file `{}`",
                stored.display()
            );
        }

        validate_target_parent_no_follow(target_root, &target_relative)?;
        let already_exists = match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if !metadata.file_type().is_file()
                    || !file_matches_record(&target, &metadata, &record)?
                {
                    bail!(
                        "existing artifact cache snapshot target does not match manifest: {}",
                        target.display()
                    );
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect snapshot target `{}`", target.display())
                });
            }
        };
        plans.push(RestorePlan {
            target,
            stored,
            record,
            already_exists,
        });
    }

    let mut summary = RestoreSummary::default();
    for plan in plans {
        if plan.already_exists {
            summary.existing_files += 1;
            summary.existing_bytes += plan.record.size;
            continue;
        }
        let parent = plan.target.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "artifact cache snapshot target has no parent: {}",
                plan.target.display()
            )
        })?;
        create_target_parent_no_follow(target_root, parent)?;
        if fs::symlink_metadata(&plan.target).is_ok() {
            bail!(
                "artifact cache snapshot target appeared during restore: {}",
                plan.target.display()
            );
        }

        let (mut temporary, copied, reconstruction) =
            clone_or_copy_file(&plan.stored, parent, true)?;
        if copied != plan.record.size {
            bail!(
                "artifact cache snapshot source changed while copying `{}`",
                plan.stored.display()
            );
        }
        temporary.flush()?;
        set_file_mode(temporary.as_file(), plan.record.mode)?;
        filetime::set_file_handle_times(
            temporary.as_file(),
            None,
            Some(filetime::FileTime::from_unix_time(
                plan.record.mtime_sec,
                plan.record.mtime_nsec,
            )),
        )?;
        let temporary_metadata = regular_file_metadata(temporary.path(), "temporary output")?;
        if !file_matches_record(temporary.path(), &temporary_metadata, &plan.record)? {
            bail!(
                "filesystem could not represent snapshot metadata for `{}`",
                plan.target.display()
            );
        }
        temporary
            .persist_noclobber(&plan.target)
            .map_err(|error| error.error)
            .with_context(|| {
                format!(
                    "failed to atomically install artifact cache snapshot target `{}`",
                    plan.target.display()
                )
            })?;
        match reconstruction {
            ReconstructionKind::Cloned => {
                summary.cloned_files += 1;
                summary.cloned_bytes += plan.record.size;
            }
            ReconstructionKind::Copied => {
                summary.copied_files += 1;
                summary.copied_bytes += plan.record.size;
            }
        }
    }
    Ok(summary)
}

fn clone_or_copy_file(
    source_path: &Path,
    destination_parent: &Path,
    allow_clone: bool,
) -> CargoResult<(tempfile::NamedTempFile, u64, ReconstructionKind)> {
    let mut source_options = OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        source_options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut source = source_options.open(source_path).with_context(|| {
        format!(
            "failed to open artifact cache snapshot source `{}`",
            source_path.display()
        )
    })?;
    if !source.metadata()?.file_type().is_file() {
        bail!(
            "artifact cache snapshot source is not a regular file: {}",
            source_path.display()
        );
    }
    #[cfg(not(any(
        target_vendor = "apple",
        all(
            target_os = "linux",
            not(any(target_arch = "sparc", target_arch = "sparc64"))
        )
    )))]
    let _ = allow_clone;

    #[cfg(target_vendor = "apple")]
    if allow_clone {
        let cloned = tempfile::Builder::new()
            .prefix(".srs-artifact-snapshot-")
            .make_in(destination_parent, |destination| {
                clone_file_to_path(&source, destination)?;
                match open_snapshot_output(destination) {
                    Ok(file) => Ok(file),
                    Err(error) => {
                        let _ = fs::remove_file(destination);
                        Err(error)
                    }
                }
            });
        match cloned {
            Ok(destination) => {
                return Ok((
                    destination,
                    source.metadata()?.len(),
                    ReconstructionKind::Cloned,
                ));
            }
            Err(error) => {
                tracing::debug!(
                    "copy-on-write clone failed for snapshot output in {}: {error}",
                    destination_parent.display()
                );
            }
        }
    }

    let mut destination = tempfile::Builder::new()
        .prefix(".srs-artifact-snapshot-")
        .tempfile_in(destination_parent)
        .with_context(|| {
            format!(
                "failed to create temporary artifact cache snapshot output in `{}`",
                destination_parent.display()
            )
        })?;

    #[cfg(all(
        target_os = "linux",
        not(any(target_arch = "sparc", target_arch = "sparc64"))
    ))]
    if allow_clone {
        match try_clone_file(&source, destination.as_file()) {
            Ok(true) => {
                return Ok((
                    destination,
                    source.metadata()?.len(),
                    ReconstructionKind::Cloned,
                ));
            }
            Ok(false) => {}
            Err(error) => {
                tracing::debug!("copy-on-write clone failed for snapshot output: {error}");
            }
        }
        drop(destination);
        destination = tempfile::Builder::new()
            .prefix(".srs-artifact-snapshot-")
            .tempfile_in(destination_parent)?;
    }

    let copied = io::copy(&mut source, destination.as_file_mut()).with_context(|| {
        format!(
            "failed to copy artifact cache snapshot source `{}`",
            source_path.display()
        )
    })?;
    Ok((destination, copied, ReconstructionKind::Copied))
}

#[cfg(target_vendor = "apple")]
fn clone_file_to_path(source: &File, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    // Match byte-copy ownership: the restored target belongs to the restoring
    // process, not to a possibly differently owned cache source. This flag is
    // declared by clonefile(2), but is not exposed by libc.
    const CLONE_NOOWNERCOPY: u32 = 0x0002;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot destination contains a NUL byte",
        )
    })?;
    let result = unsafe {
        libc::fclonefileat(
            source.as_raw_fd(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            CLONE_NOOWNERCOPY,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn open_snapshot_output(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(all(
    target_os = "linux",
    not(any(target_arch = "sparc", target_arch = "sparc64"))
))]
fn try_clone_file(source: &File, destination: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd as _;

    let result = unsafe { libc::ioctl(destination.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) };
    if result == 0 {
        return Ok(true);
    }
    Err(io::Error::last_os_error())
}

fn manifest_json(manifest: &SnapshotManifest) -> CargoResult<Vec<u8>> {
    let mut json = serde_json::to_vec(manifest)?;
    json.push(b'\n');
    Ok(json)
}

fn validate_snapshot_record(record: &SnapshotRecord) -> CargoResult<()> {
    decode_safe_relative(&record.target_path, "target path")?;
    decode_safe_relative(&record.cache_entry, "cache entry")?;
    decode_safe_filename(&record.cache_filename, "cache filename")?;
    validate_cache_digest(&record.cache_digest)?;
    if record.mode & !PERMISSION_MODE_MASK != 0 {
        bail!("artifact cache snapshot contains an invalid file mode");
    }
    if record.mtime_nsec >= 1_000_000_000 {
        bail!("artifact cache snapshot contains an invalid mtime nanosecond value");
    }
    Ok(())
}

fn validate_cache_digest(digest: &str) -> CargoResult<()> {
    if digest.len() != blake3::OUT_LEN * 2
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("artifact cache snapshot contains an invalid BLAKE3 digest");
    }
    Ok(())
}

fn validate_absolute_root(path: &Path, description: &str) -> CargoResult<()> {
    if !path.is_absolute() {
        bail!(
            "snapshot {description} must be absolute: {}",
            path.display()
        );
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        bail!(
            "snapshot {description} must not contain traversal components: {}",
            path.display()
        );
    }
    path_as_utf8(path, description)?;
    Ok(())
}

fn path_as_utf8(path: &Path, description: &str) -> CargoResult<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("snapshot {description} is not valid UTF-8"))
}

fn encode_safe_relative(path: &Path, description: &str) -> CargoResult<String> {
    let mut encoded = Vec::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            bail!(
                "snapshot {description} is not a safe relative path: {}",
                path.display()
            );
        };
        encoded.push(encode_safe_filename(component, description)?);
    }
    if encoded.is_empty() {
        bail!("snapshot {description} must not be empty");
    }
    Ok(encoded.join("/"))
}

fn encode_safe_filename(filename: &std::ffi::OsStr, description: &str) -> CargoResult<String> {
    let filename = filename
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("snapshot {description} is not valid UTF-8"))?;
    validate_safe_component(filename, description)?;
    Ok(filename.to_owned())
}

fn decode_safe_relative(encoded: &str, description: &str) -> CargoResult<PathBuf> {
    if encoded.is_empty() || encoded.starts_with('/') || encoded.ends_with('/') {
        bail!("snapshot {description} is not a safe relative path: `{encoded}`");
    }
    let mut path = PathBuf::new();
    for component in encoded.split('/') {
        validate_safe_component(component, description)?;
        path.push(component);
    }
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("snapshot {description} is not a safe relative path: `{encoded}`");
    }
    Ok(path)
}

fn decode_safe_filename(encoded: &str, description: &str) -> CargoResult<PathBuf> {
    validate_safe_component(encoded, description)?;
    let path = PathBuf::from(encoded);
    if path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        bail!("snapshot {description} is not a safe filename: `{encoded}`");
    }
    Ok(path)
}

fn validate_safe_component(component: &str, description: &str) -> CargoResult<()> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains(['/', '\\', '\0', ':'])
    {
        bail!("snapshot {description} contains unsafe component `{component}`");
    }
    Ok(())
}

fn validate_directory_no_follow(path: &Path, description: &str) -> CargoResult<()> {
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "failed to inspect snapshot {description} `{}`",
            path.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        bail!(
            "snapshot {description} is not a directory without following links: {}",
            path.display()
        );
    }
    Ok(())
}

fn validate_descendant_directory_no_follow(
    root: &Path,
    relative: &Path,
    description: &str,
) -> CargoResult<()> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            bail!("snapshot {description} contains an unsafe component");
        };
        current.push(component);
        validate_directory_no_follow(&current, description)?;
    }
    Ok(())
}

fn regular_file_metadata(path: &Path, description: &str) -> CargoResult<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {description} `{}`", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!(
            "snapshot {description} is not a regular file: {}",
            path.display()
        );
    }
    Ok(metadata)
}

fn validate_target_parent_no_follow(target_root: &Path, relative: &Path) -> CargoResult<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut current = target_root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(component) = component else {
            bail!("snapshot target parent contains an unsafe component");
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => bail!(
                "snapshot target parent is not a directory without following links: {}",
                current.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect snapshot target parent `{}`",
                        current.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn create_target_parent_no_follow(target_root: &Path, parent: &Path) -> CargoResult<()> {
    let relative = parent.strip_prefix(target_root).with_context(|| {
        format!(
            "snapshot target parent `{}` is outside target root `{}`",
            parent.display(),
            target_root.display()
        )
    })?;
    let mut current = target_root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            bail!("snapshot target parent contains an unsafe component");
        };
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                validate_directory_no_follow(&current, "target parent")?;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create snapshot target parent `{}`",
                        current.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn file_matches_record(
    path: &Path,
    metadata: &fs::Metadata,
    record: &SnapshotRecord,
) -> CargoResult<bool> {
    if !metadata.file_type().is_file()
        || metadata.len() != record.size
        || metadata_mode(metadata) != record.mode
    {
        return Ok(false);
    }
    let mtime = filetime::FileTime::from_last_modification_time(metadata);
    if mtime.unix_seconds() != record.mtime_sec || mtime.nanoseconds() != record.mtime_nsec {
        return Ok(false);
    }
    Ok(rlib_cache_digest(path)? == record.cache_digest)
}

#[cfg(unix)]
fn metadata_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & PERMISSION_MODE_MASK
}

#[cfg(not(unix))]
fn metadata_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o666
    }
}

#[cfg(unix)]
fn set_file_mode(file: &File, mode: u32) -> CargoResult<()> {
    use std::os::unix::fs::PermissionsExt as _;

    file.set_permissions(fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode(file: &File, mode: u32) -> CargoResult<()> {
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(mode & 0o222 == 0);
    file.set_permissions(permissions)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::compiler::FileFlavor;

    #[test]
    fn safe_relative_paths_round_trip_and_reject_traversal() {
        let path = Path::new("debug/deps/libexample-123.rlib");
        let encoded = encode_safe_relative(path, "test path").unwrap();
        assert_eq!(encoded, "debug/deps/libexample-123.rlib");
        assert_eq!(decode_safe_relative(&encoded, "test path").unwrap(), path);

        for malformed in [
            "",
            ".",
            "..",
            "../escape",
            "debug/../escape",
            "/absolute",
            "debug//file",
            "debug\\file",
            "C:/absolute",
        ] {
            assert!(
                decode_safe_relative(malformed, "test path").is_err(),
                "accepted {malformed:?}"
            );
        }
    }

    #[test]
    fn manifest_json_round_trip_is_deterministic() {
        let manifest = SnapshotManifest {
            version: SNAPSHOT_VERSION,
            target_root: "/target".to_owned(),
            records: vec![SnapshotRecord {
                target_path: "debug/deps/libexample.rlib".to_owned(),
                cache_entry: "action/input".to_owned(),
                cache_filename: "libexample.rlib".to_owned(),
                cache_digest: "a".repeat(blake3::OUT_LEN * 2),
                mode: 0o644,
                size: 7,
                mtime_sec: 1_700_000_000,
                mtime_nsec: 123_000_000,
            }],
        };
        let first = manifest_json(&manifest).unwrap();
        let decoded: SnapshotManifest = serde_json::from_slice(&first).unwrap();
        let second = manifest_json(&decoded).unwrap();
        assert_eq!(decoded, manifest);
        assert_eq!(second, first);
    }

    #[test]
    fn recorder_and_restorer_copy_verified_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let target_root = temp.path().join("target");
        let cache_root = temp.path().join("cache");
        let cache_entry = cache_root.join("action").join("input");
        let cache_files = cache_entry.join("files");
        fs::create_dir_all(target_root.join("debug/deps")).unwrap();
        fs::create_dir_all(&cache_files).unwrap();

        let target = target_root.join("debug/deps/libexample.rlib");
        let stored = cache_files.join("libexample.rlib");
        fs::write(&target, b"example artifact").unwrap();
        fs::write(&stored, b"example artifact").unwrap();
        let timestamp = filetime::FileTime::from_unix_time(1_700_000_000, 123_000_000);
        filetime::set_file_mtime(&target, timestamp).unwrap();

        let stored_digest = rlib_cache_digest(&stored).unwrap();
        let cache_manifest = format!("files/libexample.rlib\t{stored_digest}\n");
        fs::write(cache_entry.join("manifest.blake3"), cache_manifest).unwrap();
        let manifest_digest = rlib_cache_digest(&cache_entry.join("manifest.blake3")).unwrap();
        fs::write(cache_entry.join("complete"), format!("{manifest_digest}\n")).unwrap();

        let manifest_path = temp.path().join("snapshot.json");
        let recorder = Recorder::new(&manifest_path, &target_root, &cache_root).unwrap();
        recorder.record(
            &cache_entry,
            &[OutputFile {
                path: target.clone(),
                hardlink: None,
                export_path: None,
                flavor: FileFlavor::Linkable,
            }],
        );
        let evicted_target = target_root.join("debug/deps/libevicted.rlib");
        fs::write(&evicted_target, b"evicted artifact").unwrap();
        recorder.record(
            &cache_root.join("evicted-action").join("evicted-input"),
            &[OutputFile {
                path: evicted_target,
                hardlink: None,
                export_path: None,
                flavor: FileFlavor::Linkable,
            }],
        );
        let summary = recorder.write().unwrap();
        assert_eq!(summary.files, 1);
        assert_eq!(summary.logical_bytes, b"example artifact".len() as u64);

        fs::remove_file(&target).unwrap();
        let summary = restore(&manifest_path, &target_root, &cache_root).unwrap();
        assert_eq!(summary.cloned_files + summary.copied_files, 1);
        assert_eq!(
            summary.cloned_bytes + summary.copied_bytes,
            b"example artifact".len() as u64
        );
        assert_eq!(fs::read(&target).unwrap(), b"example artifact");

        let second = restore(&manifest_path, &target_root, &cache_root).unwrap();
        assert_eq!(second.existing_files, 1);
        assert_eq!(second.existing_bytes, b"example artifact".len() as u64);

        let different_target = temp.path().join("different-target");
        fs::create_dir(&different_target).unwrap();
        let error = restore(&manifest_path, &different_target, &cache_root).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("artifact cache snapshot target root mismatch")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;

            let target_metadata = fs::metadata(&target).unwrap();
            let stored_metadata = fs::metadata(&stored).unwrap();
            assert_ne!(
                (target_metadata.dev(), target_metadata.ino()),
                (stored_metadata.dev(), stored_metadata.ino())
            );
        }

        fs::write(&target, b"private target mutation").unwrap();
        assert_eq!(fs::read(&stored).unwrap(), b"example artifact");
        let error = restore(&manifest_path, &target_root, &cache_root).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("existing artifact cache snapshot target does not match manifest")
        );
    }

    #[test]
    fn reconstruction_falls_back_to_a_byte_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"copied artifact").unwrap();

        let (file, bytes, kind) = clone_or_copy_file(&source, temp.path(), false).unwrap();

        assert_eq!(kind, ReconstructionKind::Copied);
        assert_eq!(bytes, b"copied artifact".len() as u64);
        assert_eq!(fs::read(file.path()).unwrap(), b"copied artifact");
    }
}
