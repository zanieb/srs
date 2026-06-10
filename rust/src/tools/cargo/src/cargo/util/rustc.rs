use std::collections::{HashSet, hash_map::HashMap};
use std::env;
use std::hash::{Hash, Hasher};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Context as _;
use cargo_util::{ProcessBuilder, ProcessError, paths};
use filetime::FileTime;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::core::compiler::apply_env_config;
use crate::util::interning::InternedString;
use crate::util::{CargoResult, GlobalContext, StableHasher};

const ARTIFACT_CACHE_SAFE_CODEGEN_BACKEND_MARKER: &[u8] =
    b"rustc-codegen-backend-artifact-cache-safe-v1";

#[derive(Clone, Debug)]
struct ArtifactCacheIdentity {
    digest: blake3::Hash,
    witness: ArtifactCacheIdentityWitness,
}

#[derive(Clone, Debug)]
pub(crate) struct ArtifactCacheIdentityProvider {
    program: Option<PathBuf>,
    identity: Arc<OnceLock<Option<ArtifactCacheIdentity>>>,
}

impl ArtifactCacheIdentityProvider {
    pub(crate) fn program(&self) -> Option<&Path> {
        self.program.as_deref()
    }

    pub(crate) fn identity(&self) -> Option<blake3::Hash> {
        self.snapshot().map(|identity| identity.digest)
    }

    pub(crate) fn witness(&self) -> Option<ArtifactCacheIdentityWitness> {
        self.snapshot().map(|identity| identity.witness.clone())
    }

    fn snapshot(&self) -> Option<&ArtifactCacheIdentity> {
        self.identity
            .get_or_init(|| {
                let path = self.program.as_ref()?;
                artifact_cache_identity_for_program(path)
            })
            .as_ref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArtifactCacheIdentityWitness {
    directories: Vec<ArtifactCacheDirectoryWitness>,
    files: Vec<ArtifactCacheFileWitness>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArtifactCacheDirectoryWitness {
    path: PathBuf,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl ArtifactCacheDirectoryWitness {
    fn is_current(&self) -> bool {
        let Ok(metadata) = std::fs::metadata(&self.path) else {
            return false;
        };
        if !metadata.is_dir() || metadata.modified().ok().as_ref() != Some(&self.modified) {
            return false;
        }
        #[cfg(unix)]
        if metadata.dev() != self.device
            || metadata.ino() != self.inode
            || metadata.ctime() != self.changed_seconds
            || metadata.ctime_nsec() != self.changed_nanoseconds
        {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ArtifactCacheFileWitness {
    path: PathBuf,
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl ArtifactCacheFileWitness {
    fn from_metadata(path: &Path, metadata: &std::fs::Metadata) -> Option<Self> {
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        })
    }

    fn is_current(&self) -> bool {
        let Ok(metadata) = std::fs::metadata(&self.path) else {
            return false;
        };
        if !metadata.is_file()
            || metadata.len() != self.len
            || metadata.modified().ok().as_ref() != Some(&self.modified)
        {
            return false;
        }
        #[cfg(unix)]
        if metadata.dev() != self.device
            || metadata.ino() != self.inode
            || metadata.ctime() != self.changed_seconds
            || metadata.ctime_nsec() != self.changed_nanoseconds
        {
            return false;
        }
        true
    }
}

impl ArtifactCacheIdentityWitness {
    pub(crate) fn is_current(&self) -> bool {
        self.directories
            .iter()
            .all(ArtifactCacheDirectoryWitness::is_current)
            && self.files.iter().all(ArtifactCacheFileWitness::is_current)
    }

    pub(crate) fn contains_file(&self, path: &Path) -> bool {
        self.files
            .binary_search_by(|witness| witness.path.as_path().cmp(path))
            .is_ok()
    }
}

/// Information on the `rustc` executable
#[derive(Debug)]
pub struct Rustc {
    /// The location of the exe
    pub path: PathBuf,
    /// An optional program that will be passed the path of the rust exe as its first argument, and
    /// rustc args following this.
    pub wrapper: Option<PathBuf>,
    /// An optional wrapper to be used in addition to `rustc.wrapper` for workspace crates
    pub workspace_wrapper: Option<PathBuf>,
    /// Verbose version information (the output of `rustc -vV`)
    pub verbose_version: String,
    /// The rustc version (`1.23.4-beta.2`), this comes from `verbose_version`.
    pub version: semver::Version,
    /// The host triple (arch-platform-OS), this comes from `verbose_version`.
    pub host: InternedString,
    /// The rustc full commit hash, this comes from `verbose_version`.
    pub commit_hash: Option<String>,
    artifact_cache_identity_provider: ArtifactCacheIdentityProvider,
    cache: Mutex<Cache>,
}

impl Rustc {
    /// Runs the compiler at `path` to learn various pieces of information about
    /// it, with an optional wrapper.
    ///
    /// If successful this function returns a description of the compiler along
    /// with a list of its capabilities.
    #[tracing::instrument(skip(gctx))]
    pub fn new(
        path: PathBuf,
        wrapper: Option<PathBuf>,
        workspace_wrapper: Option<PathBuf>,
        artifact_cache_identity_is_modeled: bool,
        rustup_rustc: &Path,
        cache_location: Option<PathBuf>,
        gctx: &GlobalContext,
    ) -> CargoResult<Rustc> {
        let mut cache = Cache::load(
            wrapper.as_deref(),
            workspace_wrapper.as_deref(),
            &path,
            rustup_rustc,
            cache_location,
            gctx,
        );

        let mut cmd = ProcessBuilder::new(&path)
            .wrapped(workspace_wrapper.as_ref())
            .wrapped(wrapper.as_deref());
        apply_env_config(gctx, &mut cmd)?;
        cmd.env(crate::CARGO_ENV, gctx.cargo_exe()?);
        cmd.arg("-vV");
        let verbose_version = cache.cached_output(&cmd, 0)?.0;

        let extract = |field: &str| -> CargoResult<&str> {
            verbose_version
                .lines()
                .find_map(|l| l.strip_prefix(field))
                .ok_or_else(|| {
                    anyhow::format_err!(
                        "`rustc -vV` didn't have a line for `{}`, got:\n{}",
                        field.trim(),
                        verbose_version
                    )
                })
        };

        let host = extract("host: ")?.into();
        let version = semver::Version::parse(extract("release: ")?).with_context(|| {
            format!(
                "rustc version does not appear to be a valid semver version, from:\n{}",
                verbose_version
            )
        })?;
        let commit_hash = extract("commit-hash: ").ok().map(|hash| {
            // Possible commit-hash values from rustc are SHA hex string and "unknown". See:
            // * https://github.com/rust-lang/rust/blob/531cb83fc/src/bootstrap/src/utils/channel.rs#L73
            // * https://github.com/rust-lang/rust/blob/531cb83fc/compiler/rustc_driver_impl/src/lib.rs#L911-L913
            #[cfg(debug_assertions)]
            if hash != "unknown" {
                debug_assert!(
                    hash.chars().all(|ch| ch.is_ascii_hexdigit()),
                    "commit hash must be a hex string, got: {hash:?}"
                );
                debug_assert!(
                    hash.len() == 40 || hash.len() == 64,
                    "hex string must be generated from sha1 or sha256 (i.e., it must be 40 or 64 characters long)\ngot: {hash:?}"
                );
            }
            hash.to_string()
        });
        let artifact_cache_identity_program = artifact_cache_identity_is_modeled
            .then(|| {
                let program = std::fs::canonicalize(paths::resolve_executable(&path).ok()?).ok()?;
                let rustup_proxy = paths::resolve_executable(rustup_rustc).ok();
                let is_rustup_proxy = rustup_proxy
                    .as_ref()
                    .is_some_and(|proxy| same_file::is_same_file(&program, proxy).unwrap_or(false));
                if !is_rustup_proxy {
                    let mut sysroot = ProcessBuilder::new(&path)
                        .wrapped(workspace_wrapper.as_ref())
                        .wrapped(wrapper.as_deref());
                    apply_env_config(gctx, &mut sysroot).ok()?;
                    sysroot.env(crate::CARGO_ENV, gctx.cargo_exe().ok()?);
                    sysroot.arg("--print").arg("sysroot");
                    let reported = cache.cached_output(&sysroot, 0).ok()?.0;
                    let reported = std::fs::canonicalize(reported.trim()).ok()?;
                    let adjacent = std::fs::canonicalize(program.parent()?.parent()?).ok()?;
                    return (reported == adjacent).then_some(program);
                }
                let rustup_home = home::rustup_home().ok()?;
                let rustup_toolchain = gctx.get_env_os("RUSTUP_TOOLCHAIN")?;
                let actual_program = rustup_home
                    .join("toolchains")
                    .join(rustup_toolchain)
                    .join("bin")
                    .join("rustc")
                    .with_extension(env::consts::EXE_EXTENSION);
                std::fs::canonicalize(paths::resolve_executable(&actual_program).ok()?).ok()
            })
            .flatten();

        Ok(Rustc {
            path,
            wrapper,
            workspace_wrapper,
            verbose_version,
            version,
            host,
            commit_hash,
            artifact_cache_identity_provider: ArtifactCacheIdentityProvider {
                program: artifact_cache_identity_program,
                identity: Arc::new(OnceLock::new()),
            },
            cache: Mutex::new(cache),
        })
    }

    pub(crate) fn artifact_cache_identity_provider(&self) -> ArtifactCacheIdentityProvider {
        self.artifact_cache_identity_provider.clone()
    }

    /// Gets a process builder set up to use the found rustc version, with a wrapper if `Some`.
    pub fn process(&self) -> ProcessBuilder {
        let mut cmd = ProcessBuilder::new(self.path.as_path()).wrapped(self.wrapper.as_ref());
        cmd.retry_with_argfile(true);
        cmd
    }

    /// Gets a process builder set up to use the found rustc version, with a wrapper if `Some`.
    pub fn workspace_process(&self) -> ProcessBuilder {
        let mut cmd = ProcessBuilder::new(self.path.as_path())
            .wrapped(self.workspace_wrapper.as_ref())
            .wrapped(self.wrapper.as_ref());
        cmd.retry_with_argfile(true);
        cmd
    }

    pub fn process_no_wrapper(&self) -> ProcessBuilder {
        let mut cmd = ProcessBuilder::new(&self.path);
        cmd.retry_with_argfile(true);
        cmd
    }

    /// Gets the output for the given command.
    ///
    /// This will return the cached value if available, otherwise it will run
    /// the command and cache the output.
    ///
    /// `extra_fingerprint` is extra data to include in the cache fingerprint.
    /// Use this if there is other information about the environment that may
    /// affect the output that is not part of `cmd`.
    ///
    /// Returns a tuple of strings `(stdout, stderr)`.
    pub fn cached_output(
        &self,
        cmd: &ProcessBuilder,
        extra_fingerprint: u64,
    ) -> CargoResult<(String, String)> {
        self.cache
            .lock()
            .unwrap()
            .cached_output(cmd, extra_fingerprint)
    }
}

fn directory_files(path: &Path, recursive: bool) -> Option<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();
        let file_type = entry.file_type().ok()?;
        if file_type.is_symlink() {
            return None;
        } else if file_type.is_file() {
            files.push(path);
        } else if recursive && file_type.is_dir() {
            files.extend(directory_files(&path, true)?);
        }
    }
    files.sort();
    Some(files)
}

fn is_runtime_library(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy());
    name.is_some_and(|name| {
        name.ends_with(".dylib") || name.ends_with(".dll") || name.contains(".so")
    })
}

fn directory_runtime_library_files(path: &Path) -> Option<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();
        let file_type = entry.file_type().ok()?;
        if file_type.is_symlink() {
            if is_runtime_library(&path) {
                return None;
            }
        } else if file_type.is_file() && is_runtime_library(&path) {
            files.push(path);
        }
    }
    files.sort();
    Some(files)
}

fn is_codegen_backend(sysroot: &Path, path: &Path) -> bool {
    path.strip_prefix(sysroot)
        .ok()
        .into_iter()
        .flat_map(Path::components)
        .any(|component| component.as_os_str() == "codegen-backends")
}

fn update_reader_and_find_marker(
    hasher: &mut blake3::Hasher,
    reader: &mut impl std::io::Read,
    marker: &[u8],
) -> Option<bool> {
    let mut buffer = [0; 64 * 1024];
    let mut tail = Vec::with_capacity(marker.len().saturating_sub(1));
    let mut found = marker.is_empty();
    loop {
        let read = reader.read(&mut buffer).ok()?;
        if read == 0 {
            return Some(found);
        }
        let bytes = &buffer[..read];
        hasher.update(bytes);
        if !found {
            let mut search = Vec::with_capacity(tail.len() + bytes.len());
            search.extend_from_slice(&tail);
            search.extend_from_slice(bytes);
            found = search.windows(marker.len()).any(|window| window == marker);
            if !found {
                let keep = marker.len().saturating_sub(1).min(search.len());
                tail.clear();
                tail.extend_from_slice(&search[search.len() - keep..]);
            }
        }
    }
}

fn has_nested_runtime_library(path: &Path, excluded_root_child: Option<&str>) -> Option<bool> {
    fn walk(
        path: &Path,
        nested: bool,
        excluded_root_child: Option<&str>,
        visited: &mut HashSet<PathBuf>,
    ) -> Option<bool> {
        let canonical = match std::fs::canonicalize(path) {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(false),
            Err(_) => return None,
        };
        if !visited.insert(canonical) {
            return Some(false);
        }
        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(false),
            Err(_) => return None,
        };
        for entry in entries {
            let path = entry.ok()?.path();
            if !nested
                && excluded_root_child
                    .is_some_and(|excluded| path.file_name() == Some(excluded.as_ref()))
            {
                continue;
            }
            let metadata = std::fs::metadata(&path).ok()?;
            if metadata.is_dir() {
                if walk(&path, true, None, visited)? {
                    return Some(true);
                }
            } else if nested && metadata.is_file() && is_runtime_library(&path) {
                return Some(true);
            }
        }
        Some(false)
    }

    walk(path, false, excluded_root_child, &mut HashSet::new())
}

fn artifact_cache_identity_witness_for_sysroot(
    sysroot: &Path,
) -> Option<ArtifactCacheIdentityWitness> {
    fn collect_directories(
        path: &Path,
        recursive: bool,
        excluded: &[PathBuf],
        visited: &mut HashSet<PathBuf>,
        directories: &mut Vec<ArtifactCacheDirectoryWitness>,
    ) -> Option<()> {
        if excluded.iter().any(|excluded| path == excluded) {
            return Some(());
        }
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(()),
            Err(_) => return None,
        };
        if !metadata.is_dir() {
            return None;
        }
        let canonical = std::fs::canonicalize(path).ok()?;
        if !visited.insert(canonical) {
            return Some(());
        }
        directories.push(ArtifactCacheDirectoryWitness {
            path: path.to_path_buf(),
            modified: metadata.modified().ok()?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        });
        if recursive {
            for entry in std::fs::read_dir(path).ok()? {
                let path = entry.ok()?.path();
                if std::fs::metadata(&path).ok()?.is_dir() {
                    collect_directories(&path, true, excluded, visited, directories)?;
                }
            }
        }
        Some(())
    }

    let excluded = [
        sysroot.join("lib").join("rustlib").join("src"),
        sysroot.join("lib").join("rustlib").join("rustc-src"),
    ];
    let mut directories = Vec::new();
    let mut visited = HashSet::new();
    collect_directories(
        &sysroot.join("lib"),
        true,
        &excluded,
        &mut visited,
        &mut directories,
    )?;
    collect_directories(
        &sysroot.join("bin"),
        false,
        &excluded,
        &mut visited,
        &mut directories,
    )?;
    directories.sort_by(|left, right| left.path.cmp(&right.path));
    Some(ArtifactCacheIdentityWitness {
        directories,
        files: Vec::new(),
    })
}

fn artifact_cache_modeled_files(sysroot: &Path) -> Option<Vec<PathBuf>> {
    if cfg!(target_os = "linux")
        && has_nested_runtime_library(&sysroot.join("lib"), Some("rustlib"))?
    {
        return None;
    }

    let mut modeled_files = directory_files(&sysroot.join("lib"), false)?;
    modeled_files.extend(directory_runtime_library_files(&sysroot.join("bin"))?);
    let rustlib = sysroot.join("lib").join("rustlib");
    for target in std::fs::read_dir(&rustlib).ok()? {
        let target = target.ok()?;
        if target.file_type().ok()?.is_symlink() {
            return None;
        }
        let target = target.path();
        if target.is_dir() {
            if cfg!(target_os = "linux") && has_nested_runtime_library(&target.join("lib"), None)? {
                return None;
            }
            modeled_files.extend(directory_files(&target.join("lib"), true)?);
            modeled_files.extend(directory_files(&target.join("codegen-backends"), true)?);
        }
    }
    modeled_files.sort();
    modeled_files.dedup();
    Some(modeled_files)
}

fn update_artifact_cache_file_digest(
    hasher: &mut blake3::Hasher,
    sysroot: &Path,
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Option<ArtifactCacheFileWitness> {
    let witness = ArtifactCacheFileWitness::from_metadata(path, metadata)?;
    let mut file = std::fs::File::open(path).ok()?;
    if ArtifactCacheFileWitness::from_metadata(path, &file.metadata().ok()?)? != witness {
        return None;
    }
    let relative = path
        .strip_prefix(sysroot)
        .ok()?
        .as_os_str()
        .as_encoded_bytes();
    hasher.update(b"\0sysroot-file-content\0");
    hasher.update(&(relative.len() as u64).to_le_bytes());
    hasher.update(relative);
    hasher.update(&witness.len.to_le_bytes());
    if is_codegen_backend(sysroot, path) {
        if !update_reader_and_find_marker(
            hasher,
            &mut file,
            ARTIFACT_CACHE_SAFE_CODEGEN_BACKEND_MARKER,
        )? {
            return None;
        }
    } else {
        hasher.update_reader(&mut file).ok()?;
    }
    if ArtifactCacheFileWitness::from_metadata(path, &file.metadata().ok()?)? != witness {
        return None;
    }
    Some(witness)
}

fn artifact_cache_identity_for_program(path: &Path) -> Option<ArtifactCacheIdentity> {
    let sysroot = path.parent()?.parent()?;
    let witness_before = artifact_cache_identity_witness_for_sysroot(sysroot)?;
    let rustc_metadata = std::fs::metadata(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cargo-artifact-cache-compiler-identity-v2\0");
    let mut file_witnesses = vec![update_artifact_cache_file_digest(
        &mut hasher,
        sysroot,
        path,
        &rustc_metadata,
    )?];
    let modeled_files = artifact_cache_modeled_files(sysroot)?;
    for modeled_file in &modeled_files {
        let metadata = std::fs::metadata(modeled_file).ok()?;
        file_witnesses.push(update_artifact_cache_file_digest(
            &mut hasher,
            sysroot,
            modeled_file,
            &metadata,
        )?);
    }
    if artifact_cache_modeled_files(sysroot)? != modeled_files {
        return None;
    }
    let mut witness = artifact_cache_identity_witness_for_sysroot(sysroot)?;
    if witness.directories != witness_before.directories {
        return None;
    }
    file_witnesses.sort_by(|left, right| left.path.cmp(&right.path));
    witness.files = file_witnesses;
    if !witness.is_current() {
        return None;
    }
    Some(ArtifactCacheIdentity {
        digest: hasher.finalize(),
        witness,
    })
}

#[cfg(test)]
pub(crate) fn artifact_cache_identity_for_program_for_test(
    path: &Path,
) -> Option<(blake3::Hash, ArtifactCacheIdentityWitness)> {
    let identity = artifact_cache_identity_for_program(path)?;
    Some((identity.digest, identity.witness))
}

#[cfg(test)]
mod artifact_cache_identity_tests {
    use super::*;

    fn write_portable_test_sysroot(sysroot: &Path, rustc_contents: &[u8]) -> PathBuf {
        let rustc = sysroot.join("bin").join("rustc");
        let lib = sysroot.join("lib");
        let rustlib = lib.join("rustlib");
        let target = rustlib.join("test-target");
        let target_lib = target.join("lib");
        let driver = lib.join("librustc_driver.dylib");
        let standard_library = target_lib.join("libcore.rlib");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&target_lib).unwrap();
        std::fs::write(&rustc, rustc_contents).unwrap();
        std::fs::write(&driver, b"driver").unwrap();
        std::fs::write(&standard_library, b"standard library").unwrap();

        let timestamp = FileTime::from_unix_time(1_700_000_000, 0);
        for file in [&rustc, &driver, &standard_library] {
            filetime::set_file_times(file, timestamp, timestamp).unwrap();
        }
        for directory in [
            &target_lib,
            &target,
            &rustlib,
            &lib,
            rustc.parent().unwrap(),
        ] {
            filetime::set_file_times(directory, timestamp, timestamp).unwrap();
        }
        rustc
    }

    #[test]
    fn compiler_identity_is_portable_across_sysroot_extractions() {
        let temp = tempfile::tempdir().unwrap();
        let first_rustc =
            write_portable_test_sysroot(&temp.path().join("first-toolchain"), b"rustc");
        let second_rustc =
            write_portable_test_sysroot(&temp.path().join("second-toolchain"), b"rustc");
        let changed_timestamp = FileTime::from_unix_time(1_700_000_100, 0);
        filetime::set_file_times(&second_rustc, changed_timestamp, changed_timestamp).unwrap();
        filetime::set_file_times(
            second_rustc.parent().unwrap(),
            changed_timestamp,
            changed_timestamp,
        )
        .unwrap();
        #[cfg(unix)]
        {
            assert_ne!(
                std::fs::metadata(&first_rustc).unwrap().ino(),
                std::fs::metadata(&second_rustc).unwrap().ino()
            );
            assert_ne!(
                std::fs::metadata(first_rustc.parent().unwrap())
                    .unwrap()
                    .ino(),
                std::fs::metadata(second_rustc.parent().unwrap())
                    .unwrap()
                    .ino()
            );
        }

        let first = artifact_cache_identity_for_program(&first_rustc).unwrap();
        let second = artifact_cache_identity_for_program(&second_rustc).unwrap();

        assert!(first.witness.is_current());
        assert!(second.witness.is_current());
        assert_ne!(first.witness, second.witness);
        assert_eq!(first.digest, second.digest);
    }

    #[test]
    fn compiler_identity_ignores_codegen_backend_parent_directory_names() {
        let temp = tempfile::tempdir().unwrap();
        let first_rustc =
            write_portable_test_sysroot(&temp.path().join("first-toolchain"), b"rustc");
        let second_rustc = write_portable_test_sysroot(
            &temp
                .path()
                .join("codegen-backends")
                .join("second-toolchain"),
            b"rustc",
        );

        let first = artifact_cache_identity_for_program(&first_rustc).unwrap();
        let second = artifact_cache_identity_for_program(&second_rustc).unwrap();

        assert_eq!(first.digest, second.digest);
    }

    #[test]
    fn portable_compiler_identity_hashes_rustc_contents() {
        let temp = tempfile::tempdir().unwrap();
        let first_rustc =
            write_portable_test_sysroot(&temp.path().join("first-toolchain"), b"rustc A");
        let second_rustc =
            write_portable_test_sysroot(&temp.path().join("second-toolchain"), b"rustc B");

        let first = artifact_cache_identity_for_program(&first_rustc).unwrap();
        let second = artifact_cache_identity_for_program(&second_rustc).unwrap();

        assert_ne!(first.digest, second.digest);
    }

    #[test]
    fn portable_compiler_identity_hashes_library_contents() {
        let temp = tempfile::tempdir().unwrap();
        let first_sysroot = temp.path().join("first-toolchain");
        let second_sysroot = temp.path().join("second-toolchain");
        let first_rustc = write_portable_test_sysroot(&first_sysroot, b"rustc");
        let second_rustc = write_portable_test_sysroot(&second_sysroot, b"rustc");
        let second_driver = second_sysroot.join("lib").join("librustc_driver.dylib");
        let timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&second_driver).unwrap());
        std::fs::write(&second_driver, b"change").unwrap();
        filetime::set_file_times(&second_driver, timestamp, timestamp).unwrap();

        let first = artifact_cache_identity_for_program(&first_rustc).unwrap();
        let second = artifact_cache_identity_for_program(&second_rustc).unwrap();

        assert_ne!(first.digest, second.digest);
    }

    #[test]
    fn sysroot_codegen_backend_contents_change_compiler_identity() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let backends = sysroot
            .join("lib")
            .join("rustlib")
            .join("test-host")
            .join("codegen-backends");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&backends).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        let backend = backends.join("librustc_codegen_cranelift.dylib");
        std::fs::write(
            &backend,
            b"rustc-codegen-backend-artifact-cache-safe-v1 first backend",
        )
        .unwrap();
        let timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&backend).unwrap());
        let first = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        std::fs::write(
            &backend,
            b"rustc-codegen-backend-artifact-cache-safe-v1 other backend",
        )
        .unwrap();
        filetime::set_file_times(&backend, timestamp, timestamp).unwrap();
        let second = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        assert_ne!(first, second);
    }

    #[test]
    fn unmarked_sysroot_codegen_backend_disables_compiler_identity() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let backends = sysroot
            .join("lib")
            .join("rustlib")
            .join("test-host")
            .join("codegen-backends");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&backends).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        std::fs::write(
            backends.join("librustc_codegen_unmodeled.dylib"),
            b"unmodeled backend",
        )
        .unwrap();

        assert!(artifact_cache_identity_for_program(&rustc).is_none());
    }

    #[test]
    fn sysroot_compiler_library_updates_change_compiler_identity() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let rustlib = sysroot.join("lib").join("rustlib");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&rustlib).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        let driver = sysroot.join("lib").join("librustc_driver.dylib");
        std::fs::write(&driver, b"first driver").unwrap();
        let timestamp = FileTime::from_last_modification_time(&std::fs::metadata(&driver).unwrap());
        let first = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        std::fs::write(&driver, b"other driver").unwrap();
        let changed_timestamp =
            FileTime::from_unix_time(timestamp.unix_seconds() + 1, timestamp.nanoseconds());
        filetime::set_file_times(&driver, changed_timestamp, changed_timestamp).unwrap();
        let second = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        assert_ne!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn replaced_sysroot_compiler_library_invalidates_identity_witness() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let lib = sysroot.join("lib");
        let rustlib = lib.join("rustlib");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&rustlib).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        let driver = lib.join("librustc_driver.dylib");
        std::fs::write(&driver, b"first driver").unwrap();
        let driver_timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&driver).unwrap());
        let lib_timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&lib).unwrap());
        let first = artifact_cache_identity_for_program(&rustc).unwrap();

        let replacement_driver = lib.join("replacement-librustc_driver.dylib");
        std::fs::write(&replacement_driver, b"other driver").unwrap();
        filetime::set_file_times(&replacement_driver, driver_timestamp, driver_timestamp).unwrap();
        std::fs::rename(&replacement_driver, &driver).unwrap();
        filetime::set_file_times(&lib, lib_timestamp, lib_timestamp).unwrap();
        let second = artifact_cache_identity_for_program(&rustc).unwrap();

        assert!(!first.witness.is_current());
        assert_ne!(first.digest, second.digest);
    }

    #[cfg(unix)]
    #[test]
    fn replaced_sysroot_lib_tree_with_preserved_mtimes_invalidates_identity_witness() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let lib = sysroot.join("lib");
        let rustlib = lib.join("rustlib");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&rustlib).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        let driver = lib.join("librustc_driver.dylib");
        std::fs::write(&driver, b"first driver").unwrap();
        let driver_timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&driver).unwrap());
        let lib_timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&lib).unwrap());
        let rustlib_timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&rustlib).unwrap());
        let identity = artifact_cache_identity_for_program(&rustc).unwrap();

        let replacement_lib = temp.path().join("replacement-lib");
        let replacement_rustlib = replacement_lib.join("rustlib");
        std::fs::create_dir_all(&replacement_rustlib).unwrap();
        std::fs::write(
            replacement_lib.join("librustc_driver.dylib"),
            b"other driver",
        )
        .unwrap();
        filetime::set_file_times(
            replacement_lib.join("librustc_driver.dylib"),
            driver_timestamp,
            driver_timestamp,
        )
        .unwrap();
        filetime::set_file_times(&replacement_rustlib, rustlib_timestamp, rustlib_timestamp)
            .unwrap();
        filetime::set_file_times(&replacement_lib, lib_timestamp, lib_timestamp).unwrap();
        std::fs::rename(&lib, temp.path().join("displaced-lib")).unwrap();
        std::fs::rename(&replacement_lib, &lib).unwrap();
        let replacement_identity = artifact_cache_identity_for_program(&rustc).unwrap();

        assert!(!identity.witness.is_current());
        assert_ne!(identity.digest, replacement_identity.digest);
    }

    #[test]
    fn sysroot_standard_library_updates_change_compiler_identity() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let standard_library = sysroot
            .join("lib")
            .join("rustlib")
            .join("test-target")
            .join("lib")
            .join("libcore.rlib");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(standard_library.parent().unwrap()).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        std::fs::write(&standard_library, b"first core").unwrap();
        let timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&standard_library).unwrap());
        let first = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        std::fs::write(&standard_library, b"other core").unwrap();
        filetime::set_file_times(&standard_library, timestamp, timestamp).unwrap();
        let second = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        assert_ne!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn in_place_sysroot_update_with_preserved_mtime_invalidates_identity_witness() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = write_portable_test_sysroot(&sysroot, b"rustc");
        let driver = sysroot.join("lib").join("librustc_driver.dylib");
        let timestamp = FileTime::from_last_modification_time(&std::fs::metadata(&driver).unwrap());
        let before = std::fs::metadata(&driver).unwrap();
        let identity = artifact_cache_identity_for_program(&rustc).unwrap();

        std::fs::write(&driver, b"change").unwrap();
        filetime::set_file_times(&driver, timestamp, timestamp).unwrap();
        let after = std::fs::metadata(&driver).unwrap();

        assert_eq!(before.ino(), after.ino());
        assert_ne!(
            (before.ctime(), before.ctime_nsec()),
            (after.ctime(), after.ctime_nsec())
        );
        assert!(!identity.witness.is_current());
    }

    #[test]
    fn backend_marker_detection_spans_read_boundaries() {
        let marker = ARTIFACT_CACHE_SAFE_CODEGEN_BACKEND_MARKER;
        let prefix_len = 64 * 1024 - marker.len() / 2;
        let mut contents = vec![b'x'; prefix_len];
        contents.extend_from_slice(marker);
        contents.extend_from_slice(b"tail");
        let mut reader = std::io::Cursor::new(&contents);
        let mut hasher = blake3::Hasher::new();

        assert_eq!(
            update_reader_and_find_marker(&mut hasher, &mut reader, marker),
            Some(true)
        );
        assert_eq!(hasher.finalize(), blake3::hash(&contents));
    }

    #[cfg(unix)]
    #[test]
    fn sysroot_source_symlinks_are_excluded_from_compiler_identity() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let rustlib = sysroot.join("lib").join("rustlib");
        let source = temp.path().join("source");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(rustlib.join("test-target").join("lib")).unwrap();
        std::fs::create_dir_all(rustlib.join("src")).unwrap();
        std::fs::create_dir_all(rustlib.join("rustc-src")).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        std::fs::write(source.join("lib.rs"), b"first source").unwrap();
        symlink(&source, rustlib.join("src").join("rust")).unwrap();
        symlink(&source, rustlib.join("rustc-src").join("rust")).unwrap();

        let first = artifact_cache_identity_for_program(&rustc).unwrap();
        std::fs::write(source.join("other.rs"), b"other source").unwrap();
        let second = artifact_cache_identity_for_program(&rustc).unwrap();

        assert!(first.witness.is_current());
        assert_eq!(first.digest, second.digest);
    }

    #[cfg(unix)]
    #[test]
    fn sysroot_bin_non_runtime_symlinks_do_not_disable_compiler_identity() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let rustlib = sysroot.join("lib").join("rustlib");
        let wrapper = temp.path().join("cargo-srs.sh");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&rustlib).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        std::fs::write(&wrapper, b"wrapper").unwrap();
        symlink(&wrapper, sysroot.join("bin").join("cargo")).unwrap();

        assert!(artifact_cache_identity_for_program(&rustc).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn sysroot_bin_runtime_library_symlinks_disable_compiler_identity() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let rustlib = sysroot.join("lib").join("rustlib");
        let runtime = temp.path().join("librustc_driver.dylib");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&rustlib).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        std::fs::write(&runtime, b"driver").unwrap();
        symlink(&runtime, sysroot.join("bin").join("librustc_driver.dylib")).unwrap();

        assert!(artifact_cache_identity_for_program(&rustc).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn sysroot_symlink_cycle_disables_compiler_identity() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let libraries = sysroot
            .join("lib")
            .join("rustlib")
            .join("test-target")
            .join("lib");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&libraries).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        symlink(".", libraries.join("loop")).unwrap();

        assert!(artifact_cache_identity_for_program(&rustc).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysroot_hwcaps_runtime_library_disables_compiler_identity() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let library = sysroot
            .join("lib")
            .join("glibc-hwcaps")
            .join("x86-64-v3")
            .join("librustc_driver.so");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        std::fs::write(&library, b"driver").unwrap();

        assert!(artifact_cache_identity_for_program(&rustc).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rustlib_hwcaps_runtime_library_disables_compiler_identity() {
        let temp = tempfile::tempdir().unwrap();
        let sysroot = temp.path().join("toolchain");
        let rustc = sysroot.join("bin").join("rustc");
        let library = sysroot
            .join("lib")
            .join("rustlib")
            .join("test-host")
            .join("lib")
            .join("glibc-hwcaps")
            .join("x86-64-v3")
            .join("librust_runtime.so");
        std::fs::create_dir_all(rustc.parent().unwrap()).unwrap();
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&rustc, b"rustc").unwrap();
        std::fs::write(&library, b"runtime").unwrap();

        assert!(artifact_cache_identity_for_program(&rustc).is_none());
    }
}

/// It is a well known fact that `rustc` is not the fastest compiler in the
/// world.  What is less known is that even `rustc --version --verbose` takes
/// about a hundred milliseconds! Because we need compiler version info even
/// for no-op builds, we cache it here, based on compiler's mtime and rustup's
/// current toolchain.
///
/// <https://github.com/rust-lang/cargo/issues/5315>
/// <https://github.com/rust-lang/rust/issues/49761>
#[derive(Debug)]
struct Cache {
    cache_location: Option<PathBuf>,
    dirty: bool,
    data: CacheData,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct CacheData {
    rustc_fingerprint: u64,
    outputs: HashMap<u64, Output>,
    successes: HashMap<u64, bool>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Output {
    success: bool,
    status: String,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Cache {
    fn load(
        wrapper: Option<&Path>,
        workspace_wrapper: Option<&Path>,
        rustc: &Path,
        rustup_rustc: &Path,
        cache_location: Option<PathBuf>,
        gctx: &GlobalContext,
    ) -> Cache {
        match (
            cache_location,
            rustc_fingerprint(wrapper, workspace_wrapper, rustc, rustup_rustc, gctx),
        ) {
            (Some(cache_location), Ok(rustc_fingerprint)) => {
                let empty = CacheData {
                    rustc_fingerprint,
                    outputs: HashMap::new(),
                    successes: HashMap::new(),
                };
                let mut dirty = true;
                let data = match read(&cache_location) {
                    Ok(data) => {
                        if data.rustc_fingerprint == rustc_fingerprint {
                            debug!("reusing existing rustc info cache");
                            dirty = false;
                            data
                        } else {
                            debug!("different compiler, creating new rustc info cache");
                            empty
                        }
                    }
                    Err(e) => {
                        debug!("failed to read rustc info cache: {}", e);
                        empty
                    }
                };
                return Cache {
                    cache_location: Some(cache_location),
                    dirty,
                    data,
                };

                fn read(path: &Path) -> CargoResult<CacheData> {
                    let json = paths::read(path)?;
                    Ok(serde_json::from_str(&json)?)
                }
            }
            (_, fingerprint) => {
                if let Err(e) = fingerprint {
                    warn!("failed to calculate rustc fingerprint: {}", e);
                }
                debug!("rustc info cache disabled");
                Cache {
                    cache_location: None,
                    dirty: false,
                    data: CacheData::default(),
                }
            }
        }
    }

    fn cached_output(
        &mut self,
        cmd: &ProcessBuilder,
        extra_fingerprint: u64,
    ) -> CargoResult<(String, String)> {
        let key = process_fingerprint(cmd, extra_fingerprint);
        if let std::collections::hash_map::Entry::Vacant(e) = self.data.outputs.entry(key) {
            debug!("rustc info cache miss");
            debug!("running {}", cmd);
            let output = cmd.output()?;
            let stdout = String::from_utf8(output.stdout)
                .map_err(|e| anyhow::anyhow!("{}: {:?}", e, e.as_bytes()))
                .with_context(|| format!("`{}` didn't return utf8 output", cmd))?;
            let stderr = String::from_utf8(output.stderr)
                .map_err(|e| anyhow::anyhow!("{}: {:?}", e, e.as_bytes()))
                .with_context(|| format!("`{}` didn't return utf8 output", cmd))?;
            e.insert(Output {
                success: output.status.success(),
                status: if output.status.success() {
                    String::new()
                } else {
                    cargo_util::exit_status_to_string(output.status)
                },
                code: output.status.code(),
                stdout,
                stderr,
            });
            self.dirty = true;
        } else {
            debug!("rustc info cache hit");
        }
        let output = &self.data.outputs[&key];
        if output.success {
            Ok((output.stdout.clone(), output.stderr.clone()))
        } else {
            Err(ProcessError::new_raw(
                &format!("process didn't exit successfully: {}", cmd),
                output.code,
                &output.status,
                Some(output.stdout.as_ref()),
                Some(output.stderr.as_ref()),
            )
            .into())
        }
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(ref path) = self.cache_location {
            let json = serde_json::to_string(&self.data).unwrap();
            match paths::write(path, json.as_bytes()) {
                Ok(()) => info!("updated rustc info cache"),
                Err(e) => warn!("failed to update rustc info cache: {}", e),
            }
        }
    }
}

fn rustc_fingerprint(
    wrapper: Option<&Path>,
    workspace_wrapper: Option<&Path>,
    rustc: &Path,
    rustup_rustc: &Path,
    gctx: &GlobalContext,
) -> CargoResult<u64> {
    let mut hasher = StableHasher::new();

    let hash_exe = |hasher: &mut _, path| -> CargoResult<()> {
        let path = paths::resolve_executable(path)?;
        path.hash(hasher);

        let meta = paths::metadata(&path)?;
        meta.len().hash(hasher);

        // Often created and modified are the same, but not all filesystems support the former,
        // and distro reproducible builds may clamp the latter, so we try to use both.
        FileTime::from_creation_time(&meta).hash(hasher);
        FileTime::from_last_modification_time(&meta).hash(hasher);
        Ok(())
    };

    hash_exe(&mut hasher, rustc)?;
    if let Some(wrapper) = wrapper {
        hash_exe(&mut hasher, wrapper)?;
    }
    if let Some(workspace_wrapper) = workspace_wrapper {
        hash_exe(&mut hasher, workspace_wrapper)?;
    }

    // Rustup can change the effective compiler without touching
    // the `rustc` binary, so we try to account for this here.
    // If we see rustup's env vars, we mix them into the fingerprint,
    // but we also mix in the mtime of the actual compiler (and not
    // the rustup shim at `~/.cargo/bin/rustup`), because `RUSTUP_TOOLCHAIN`
    // could be just `stable-x86_64-unknown-linux-gnu`, i.e, it could
    // not mention the version of Rust at all, which changes after
    // `rustup update`.
    //
    // If we don't see rustup env vars, but it looks like the compiler
    // is managed by rustup, we conservatively bail out.
    let maybe_rustup = rustup_rustc == rustc;
    match (
        maybe_rustup,
        gctx.get_env("RUSTUP_HOME"),
        gctx.get_env("RUSTUP_TOOLCHAIN"),
    ) {
        (_, Ok(rustup_home), Ok(rustup_toolchain)) => {
            debug!("adding rustup info to rustc fingerprint");
            rustup_toolchain.hash(&mut hasher);
            rustup_home.hash(&mut hasher);
            let real_rustc = Path::new(&rustup_home)
                .join("toolchains")
                .join(rustup_toolchain)
                .join("bin")
                .join("rustc")
                .with_extension(env::consts::EXE_EXTENSION);
            paths::mtime(&real_rustc)?.hash(&mut hasher);
        }
        (true, _, _) => anyhow::bail!("probably rustup rustc, but without rustup's env vars"),
        _ => (),
    }

    Ok(Hasher::finish(&hasher))
}

fn process_fingerprint(cmd: &ProcessBuilder, extra_fingerprint: u64) -> u64 {
    let mut hasher = StableHasher::new();
    extra_fingerprint.hash(&mut hasher);
    cmd.get_args().for_each(|arg| arg.hash(&mut hasher));
    let mut env = cmd.get_envs().iter().collect::<Vec<_>>();
    env.sort_unstable();
    env.hash(&mut hasher);
    Hasher::finish(&hasher)
}
