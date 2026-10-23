//! Fail-closed, streaming content fingerprints for reviewed deletion targets.
//!
//! This detects drift, not an atomic filesystem transaction. Call immediately
//! before deletion; concurrent writes after the last check remain possible.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, Metadata};
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeSnapshot {
    pub sha256: String,
    pub entries: u64,
    pub bytes: u64,
}

/// Fingerprint every entry, including empty directories and file contents.
/// Unreadable entries, links/reparse points, and special files are errors.
pub fn capture(root: &Path) -> io::Result<TreeSnapshot> {
    let mut digest = Sha256::new();
    digest.update(b"sweep-tree-snapshot-v1\0");
    let mut entries = 0;
    let mut bytes = 0;
    visit(root, root, &mut digest, &mut entries, &mut bytes)?;
    Ok(TreeSnapshot {
        sha256: format!("{:x}", digest.finalize()),
        entries,
        bytes,
    })
}

fn identity(path: &Path) -> io::Result<u64> {
    let handle = same_file::Handle::from_path(path)?;
    let mut hash = DefaultHasher::new();
    handle.hash(&mut hash);
    Ok(hash.finish())
}

fn metadata_digest(meta: &Metadata) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    data.extend_from_slice(&meta.len().to_le_bytes());
    data.push(u8::from(meta.permissions().readonly()));
    // SystemTime's Hash implementation preserves pre-epoch values too.
    let mut time = DefaultHasher::new();
    meta.modified()?.hash(&mut time);
    if let Ok(created) = meta.created() {
        created.hash(&mut time);
    }
    data.extend_from_slice(&time.finish().to_le_bytes());
    Ok(data)
}

fn visit(
    root: &Path,
    path: &Path,
    digest: &mut Sha256,
    entries: &mut u64,
    bytes: &mut u64,
) -> io::Result<()> {
    // Explicit work stack avoids overflowing a thread stack on deep trees.
    enum Pending {
        Visit(PathBuf),
        Verify {
            path: PathBuf,
            id: u64,
            metadata: Vec<u8>,
        },
    }
    let mut pending = vec![Pending::Visit(path.to_path_buf())];
    while let Some(next) = pending.pop() {
        let path = match next {
            Pending::Verify { path, id, metadata } => {
                let after = fs::symlink_metadata(&path)?;
                if crate::scanner::metadata_is_link_or_reparse(&after)
                    || metadata_digest(&after)? != metadata
                    || identity(&path)? != id
                {
                    return Err(io::Error::other("path changed while taking snapshot"));
                }
                continue;
            }
            Pending::Visit(path) => path,
        };
        let before = fs::symlink_metadata(&path)?;
        if crate::scanner::metadata_is_link_or_reparse(&before) {
            return Err(io::Error::other(
                "snapshot refuses links and reparse points",
            ));
        }
        if !before.is_dir() && !before.is_file() {
            return Err(io::Error::other("snapshot refuses special files"));
        }
        let id = identity(&path)?;
        let metadata = metadata_digest(&before)?;
        let name = path
            .strip_prefix(root)
            .map_err(io::Error::other)?
            .as_os_str()
            .as_encoded_bytes();
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name);
        digest.update([u8::from(before.is_dir())]);
        digest.update(id.to_le_bytes());
        digest.update(&metadata);
        *entries = entries
            .checked_add(1)
            .ok_or_else(|| io::Error::other("snapshot entry count overflow"))?;
        if before.is_dir() {
            let mut children = fs::read_dir(&path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()?;
            children.sort();
            pending.push(Pending::Verify { path, id, metadata });
            pending.extend(children.into_iter().rev().map(Pending::Visit));
        } else {
            let count = hash_file(&path, &before, &metadata, digest)?;
            *bytes = bytes
                .checked_add(count)
                .ok_or_else(|| io::Error::other("snapshot byte count overflow"))?;
            pending.push(Pending::Verify { path, id, metadata });
        }
    }
    Ok(())
}

fn hash_file(
    path: &Path,
    before: &Metadata,
    metadata: &[u8],
    digest: &mut Sha256,
) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut read_bytes = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        read_bytes = read_bytes
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("snapshot byte count overflow"))?;
    }
    if read_bytes != before.len() || metadata_digest(&file.metadata()?)? != metadata {
        return Err(io::Error::other("file changed while taking snapshot"));
    }
    Ok(read_bytes)
}
