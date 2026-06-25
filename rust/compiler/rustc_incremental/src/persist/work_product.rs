//! Functions for saving and removing intermediate [work products].
//!
//! [work products]: WorkProduct

use std::path::{Path, PathBuf};
use std::{env, fs as std_fs, io};

use rustc_data_structures::unord::UnordMap;
use rustc_fs_util::link_or_copy;
use rustc_middle::dep_graph::{WorkProduct, WorkProductId};
use rustc_session::Session;
use rustc_span::{SourceFileHash, SourceFileHashAlgorithm};
use tracing::debug;

use crate::diagnostics;
use crate::persist::fs::*;

const SLD_RUSTC_WORK_PRODUCT_PROVENANCE_ENV: &str = "SLD_RUSTC_WORK_PRODUCT_PROVENANCE";
const SLD_CGU_OBJECT_DIGEST_FILE_ID: &str = "sld-blake3-o";

/// Copies a CGU work product to the incremental compilation directory, so next compilation can
/// find and reuse it.
///
/// Panics when incr comp is disabled.
pub fn copy_cgu_workproduct_to_incr_comp_cache_dir(
    sess: &Session,
    cgu_name: &str,
    files: &[(&'static str, &Path)],
    known_links: &[PathBuf],
    known_object_digest: Option<&str>,
) -> (WorkProductId, WorkProduct, Option<String>) {
    debug!(?cgu_name, ?files);
    assert!(sess.opts.incremental.is_some());

    let mut saved_files = UnordMap::default();
    let mut object_digest = None;
    for (ext, path) in files {
        let file_name = format!("{cgu_name}.{ext}");
        let path_in_incr_dir = in_incr_comp_dir_sess(sess, &file_name);
        let reused = known_links.contains(&path_in_incr_dir);
        if !reused && let Err(err) = link_or_copy(path, &path_in_incr_dir) {
            sess.dcx().emit_warn(diagnostics::CopyWorkProductToCache {
                from: path,
                to: &path_in_incr_dir,
                err,
            });
            continue;
        }
        let _ = saved_files.insert(ext.to_string(), file_name);
        if *ext == "o" {
            object_digest = track_sld_cgu_object_digest(
                cgu_name,
                &path_in_incr_dir,
                reused,
                known_object_digest,
                &mut saved_files,
            );
        }
    }

    let work_product = WorkProduct { cgu_name: cgu_name.to_string(), saved_files };
    debug!(?work_product);
    let work_product_id = WorkProductId::from_cgu_name(cgu_name);
    (work_product_id, work_product, object_digest)
}

fn track_sld_cgu_object_digest(
    cgu_name: &str,
    object_path: &Path,
    reused: bool,
    known_digest: Option<&str>,
    saved_files: &mut UnordMap<String, String>,
) -> Option<String> {
    let digest_path = sld_cgu_object_digest_path(object_path, cgu_name);
    if env::var_os(SLD_RUSTC_WORK_PRODUCT_PROVENANCE_ENV).as_deref() != Some("1".as_ref()) {
        remove_sld_cgu_object_digest(&digest_path);
        return None;
    }
    let existing_digest = read_sld_cgu_object_digest_file(&digest_path);
    if !can_reuse_sld_cgu_object_digest(reused, existing_digest.as_deref(), known_digest)
        && let Err(error) = write_sld_cgu_object_digest(object_path, &digest_path)
    {
        debug!("failed to write SLD CGU object digest for `{}`: {error}", object_path.display());
        remove_sld_cgu_object_digest(&digest_path);
        return None;
    }
    if let Some(digest) = read_sld_cgu_object_digest_file(&digest_path)
        && let Some(file_name) = digest_path.file_name().and_then(|name| name.to_str())
    {
        let _ = saved_files.insert(SLD_CGU_OBJECT_DIGEST_FILE_ID.to_owned(), file_name.to_owned());
        Some(digest)
    } else {
        debug!("ignored malformed SLD CGU object digest for `{}`", object_path.display());
        None
    }
}

fn can_reuse_sld_cgu_object_digest(
    reused: bool,
    existing_digest: Option<&str>,
    known_digest: Option<&str>,
) -> bool {
    reused && known_digest.is_some() && existing_digest == known_digest
}

fn write_sld_cgu_object_digest(object_path: &Path, digest_path: &Path) -> io::Result<()> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let object = std_fs::File::open(object_path)?;
    let digest = SourceFileHash::new(SourceFileHashAlgorithm::Blake3, object)?;
    let mut rendered = String::with_capacity(65);
    for byte in digest.hash_bytes() {
        rendered.push(HEX[(byte >> 4) as usize] as char);
        rendered.push(HEX[(byte & 0xf) as usize] as char);
    }
    rendered.push('\n');

    let tmp = digest_path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std_fs::remove_file(&tmp);
    std_fs::write(&tmp, rendered)?;
    let _ = std_fs::remove_file(digest_path);
    std_fs::rename(tmp, digest_path)
}

fn remove_sld_cgu_object_digest(digest_path: &Path) {
    if let Err(error) = std_fs::remove_file(digest_path)
        && error.kind() != io::ErrorKind::NotFound
    {
        debug!("failed to remove stale SLD CGU object digest `{}`: {error}", digest_path.display());
    }
}

fn sld_cgu_object_digest_path(object_path: &Path, cgu_name: &str) -> PathBuf {
    object_path.with_file_name(format!("{cgu_name}.{SLD_CGU_OBJECT_DIGEST_FILE_ID}"))
}

/// Reads the persisted BLAKE3 digest for a rustc incremental CGU object.
pub fn read_sld_cgu_object_digest(
    object_path: &Path,
    work_product: &WorkProduct,
) -> Option<String> {
    let digest_file = work_product.saved_files.get(SLD_CGU_OBJECT_DIGEST_FILE_ID)?;
    read_sld_cgu_object_digest_file(&object_path.with_file_name(digest_file))
}

fn read_sld_cgu_object_digest_file(digest_path: &Path) -> Option<String> {
    let digest = std_fs::read_to_string(digest_path).ok()?;
    let digest = digest.strip_suffix('\n').unwrap_or(&digest);
    (digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then(|| digest.to_owned())
}

/// Removes files for a given work product.
pub(crate) fn delete_workproduct_files(sess: &Session, work_product: &WorkProduct) {
    for (_, path) in work_product.saved_files.items().into_sorted_stable_ord() {
        let path = in_incr_comp_dir_sess(sess, path);
        if let Err(err) = std_fs::remove_file(&path) {
            sess.dcx().emit_warn(diagnostics::DeleteWorkProduct { path: &path, err });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::can_reuse_sld_cgu_object_digest;

    #[test]
    fn sld_cgu_object_digest_reuse_requires_tracked_token() {
        assert!(can_reuse_sld_cgu_object_digest(true, Some("digest"), Some("digest")));
        assert!(!can_reuse_sld_cgu_object_digest(false, Some("digest"), Some("digest")));
        assert!(!can_reuse_sld_cgu_object_digest(true, None, None));
        assert!(!can_reuse_sld_cgu_object_digest(true, Some("digest"), None));
        assert!(!can_reuse_sld_cgu_object_digest(true, None, Some("digest")));
        assert!(!can_reuse_sld_cgu_object_digest(true, Some("old"), Some("new")));
    }
}
