use std::collections::{HashSet, hash_map::HashMap};
use std::env;
use std::hash::{Hash, Hasher};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::Context as _;
use cargo_util::{ProcessBuilder, ProcessError, paths};
use filetime::FileTime;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::core::compiler::apply_env_config;
use crate::util::interning::InternedString;
use crate::util::{CargoResult, GlobalContext, StableHasher};

#[derive(Clone, Debug)]
struct ArtifactCacheIdentity {
    digest: blake3::Hash,
    witness: ArtifactCacheIdentityWitness,
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
        if metadata.dev() != self.device || metadata.ino() != self.inode {
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
        if metadata.dev() != self.device || metadata.ino() != self.inode {
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

    fn update_digest(&self, sysroot: &Path, hasher: &mut blake3::Hasher) -> Option<()> {
        for directory in &self.directories {
            let modified = directory
                .modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            hasher.update(b"\0sysroot-directory-identity\0");
            hasher.update(
                directory
                    .path
                    .strip_prefix(sysroot)
                    .ok()?
                    .to_string_lossy()
                    .as_bytes(),
            );
            hasher.update(b"\0");
            hasher.update(&modified.as_secs().to_le_bytes());
            hasher.update(&modified.subsec_nanos().to_le_bytes());
            #[cfg(unix)]
            {
                hasher.update(&directory.device.to_le_bytes());
                hasher.update(&directory.inode.to_le_bytes());
            }
        }
        for file in &self.files {
            let modified = file.modified.duration_since(std::time::UNIX_EPOCH).ok()?;
            hasher.update(b"\0sysroot-file-identity\0");
            hasher.update(
                file.path
                    .strip_prefix(sysroot)
                    .ok()?
                    .to_string_lossy()
                    .as_bytes(),
            );
            hasher.update(b"\0");
            hasher.update(&file.len.to_le_bytes());
            hasher.update(&modified.as_secs().to_le_bytes());
            hasher.update(&modified.subsec_nanos().to_le_bytes());
            #[cfg(unix)]
            {
                hasher.update(&file.device.to_le_bytes());
                hasher.update(&file.inode.to_le_bytes());
            }
        }
        Some(())
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
    /// The actual compiler binary whose contents identify restorable output.
    ///
    /// This is absent for explicitly configured compiler drivers and
    /// unresolved rustup proxy invocations.
    artifact_cache_identity_program: Option<PathBuf>,
    artifact_cache_identity: OnceLock<Option<ArtifactCacheIdentity>>,
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
                let program = paths::resolve_executable(&path).ok()?;
                let rustup_proxy = paths::resolve_executable(rustup_rustc).ok();
                let is_rustup_proxy = rustup_proxy
                    .as_ref()
                    .is_some_and(|proxy| same_file::is_same_file(&program, proxy).unwrap_or(false));
                if !is_rustup_proxy {
                    return Some(program);
                }
                let rustup_home = home::rustup_home().ok()?;
                let rustup_toolchain = gctx.get_env_os("RUSTUP_TOOLCHAIN")?;
                let actual_program = rustup_home
                    .join("toolchains")
                    .join(rustup_toolchain)
                    .join("bin")
                    .join("rustc")
                    .with_extension(env::consts::EXE_EXTENSION);
                paths::resolve_executable(&actual_program).ok()
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
            artifact_cache_identity_program,
            artifact_cache_identity: OnceLock::new(),
            cache: Mutex::new(cache),
        })
    }

    /// Gets the compiler-content identity used by Cargo's artifact cache.
    ///
    /// Explicitly configured compiler programs and unresolved rustup proxies
    /// can delegate to side inputs Cargo cannot model, so they do not receive
    /// an identity for restoration.
    pub fn artifact_cache_identity(&self) -> Option<blake3::Hash> {
        self.artifact_cache_identity_snapshot()
            .map(|identity| identity.digest)
    }

    pub(crate) fn artifact_cache_identity_witness(&self) -> Option<ArtifactCacheIdentityWitness> {
        self.artifact_cache_identity_snapshot()
            .map(|identity| identity.witness.clone())
    }

    fn artifact_cache_identity_snapshot(&self) -> Option<&ArtifactCacheIdentity> {
        self.artifact_cache_identity
            .get_or_init(|| {
                let path = self.artifact_cache_identity_program.as_ref()?;
                artifact_cache_identity_for_program(path)
            })
            .as_ref()
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
        let path = entry.ok()?.path();
        let metadata = std::fs::metadata(&path).ok()?;
        if metadata.is_file() {
            files.push(path);
        } else if recursive && metadata.is_dir() {
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

fn is_codegen_backend(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "codegen-backends")
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
        visited: &mut HashSet<PathBuf>,
        directories: &mut Vec<ArtifactCacheDirectoryWitness>,
    ) -> Option<()> {
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
        });
        if recursive {
            for entry in std::fs::read_dir(path).ok()? {
                let path = entry.ok()?.path();
                if std::fs::metadata(&path).ok()?.is_dir() {
                    collect_directories(&path, true, visited, directories)?;
                }
            }
        }
        Some(())
    }

    let mut directories = Vec::new();
    let mut visited = HashSet::new();
    collect_directories(&sysroot.join("lib"), true, &mut visited, &mut directories)?;
    collect_directories(&sysroot.join("bin"), false, &mut visited, &mut directories)?;
    directories.sort_by(|left, right| left.path.cmp(&right.path));
    Some(ArtifactCacheIdentityWitness {
        directories,
        files: Vec::new(),
    })
}

fn artifact_cache_identity_for_program(path: &Path) -> Option<ArtifactCacheIdentity> {
    let sysroot = path.parent()?.parent()?;
    let witness_before = artifact_cache_identity_witness_for_sysroot(sysroot)?;
    let rustc_metadata = std::fs::metadata(path).ok()?;
    let mut file_witnesses = vec![ArtifactCacheFileWitness::from_metadata(
        path,
        &rustc_metadata,
    )?];
    let contents = std::fs::read(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(&contents);
    if cfg!(target_os = "linux")
        && has_nested_runtime_library(&sysroot.join("lib"), Some("rustlib"))?
    {
        return None;
    }

    let mut modeled_files = directory_files(&sysroot.join("lib"), false)?;
    modeled_files.extend(
        directory_files(&sysroot.join("bin"), false)?
            .into_iter()
            .filter(|path| is_runtime_library(path)),
    );
    let rustlib = sysroot.join("lib").join("rustlib");
    for target in std::fs::read_dir(&rustlib).ok()? {
        let target = target.ok()?.path();
        if std::fs::metadata(&target).ok()?.is_dir() {
            if cfg!(target_os = "linux") && has_nested_runtime_library(&target.join("lib"), None)? {
                return None;
            }
            modeled_files.extend(directory_files(&target.join("lib"), true)?);
            modeled_files.extend(directory_files(&target.join("codegen-backends"), true)?);
        }
    }
    modeled_files.sort();
    modeled_files.dedup();
    for modeled_file in modeled_files {
        // Normal sysroot installation publishes compiler and target libraries
        // with new metadata. Named backends are content-hashed because SRS can
        // select one directly while retaining ordinary cache startup cost.
        let metadata = std::fs::metadata(&modeled_file).ok()?;
        file_witnesses.push(ArtifactCacheFileWitness::from_metadata(
            &modeled_file,
            &metadata,
        )?);
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        hasher.update(b"\0sysroot-input\0");
        hasher.update(
            modeled_file
                .strip_prefix(sysroot)
                .ok()?
                .to_string_lossy()
                .as_bytes(),
        );
        hasher.update(b"\0");
        hasher.update(&metadata.len().to_le_bytes());
        hasher.update(&modified.as_secs().to_le_bytes());
        hasher.update(&modified.subsec_nanos().to_le_bytes());
        if is_codegen_backend(&modeled_file) {
            hasher.update(b"\0sysroot-codegen-backend-content\0");
            hasher.update(&std::fs::read(&modeled_file).ok()?);
        }
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
    witness.update_digest(sysroot, &mut hasher)?;
    Some(ArtifactCacheIdentity {
        digest: hasher.finalize(),
        witness,
    })
}

#[cfg(test)]
mod artifact_cache_identity_tests {
    use super::*;

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
        std::fs::write(&backend, b"first backend").unwrap();
        let timestamp =
            FileTime::from_last_modification_time(&std::fs::metadata(&backend).unwrap());
        let first = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        std::fs::write(&backend, b"other backend").unwrap();
        filetime::set_file_times(&backend, timestamp, timestamp).unwrap();
        let second = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        assert_ne!(first, second);
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

        std::fs::remove_file(&driver).unwrap();
        std::fs::write(&driver, b"other driver").unwrap();
        filetime::set_file_times(&driver, driver_timestamp, driver_timestamp).unwrap();
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
        let changed_timestamp =
            FileTime::from_unix_time(timestamp.unix_seconds() + 1, timestamp.nanoseconds());
        filetime::set_file_times(&standard_library, changed_timestamp, changed_timestamp).unwrap();
        let second = artifact_cache_identity_for_program(&rustc).unwrap().digest;

        assert_ne!(first, second);
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
