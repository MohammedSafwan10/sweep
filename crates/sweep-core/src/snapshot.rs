//! Fail-closed content fingerprints. These detect drift, not atomic transactions.
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, Metadata, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeSnapshot {
    pub sha256: String,
    pub entries: u64,
    pub bytes: u64,
}

/// Optional counters shared by planning/rechecking workers. Never carry approval.
#[derive(Debug, Default)]
pub struct SnapshotProgress {
    files: AtomicU64,
    bytes: AtomicU64,
}

impl SnapshotProgress {
    pub fn counts(&self) -> (u64, u64) {
        (
            self.files.load(Ordering::Relaxed),
            self.bytes.load(Ordering::Relaxed),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    volume: u64,
    index: u64,
}

struct Entry {
    path: PathBuf,
    metadata: Metadata,
    directory_identity: Option<Identity>,
}

struct Fingerprint {
    digest: [u8; 32],
    bytes: u64,
}

#[derive(Clone, Copy)]
enum OpenKind {
    Directory,
    FileContents,
    FileMetadata,
}

/// Hash every entry, including empty directories and full file contents.
/// Entry digests are folded in sorted path order, independent of worker scheduling.
pub fn capture(root: &Path) -> io::Result<TreeSnapshot> {
    capture_with_progress(root, None)
}

pub fn capture_with_progress(
    root: &Path,
    progress: Option<&SnapshotProgress>,
) -> io::Result<TreeSnapshot> {
    // One shared pool caps hashing even when independent cleanup jobs are parallel.
    static POOL: OnceLock<Result<rayon::ThreadPool, String>> = OnceLock::new();
    let pool = POOL
        .get_or_init(|| {
            let workers = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
            rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .thread_name(|n| format!("sweep-snapshot-{n}"))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| io::Error::other(error.clone()))?;
    capture_in_pool(root, progress, pool)
}

fn capture_in_pool(
    root: &Path,
    progress: Option<&SnapshotProgress>,
    pool: &rayon::ThreadPool,
) -> io::Result<TreeSnapshot> {
    let entries = inventory(root)?;
    let mut digest = Sha256::new();
    digest.update(b"sweep-tree-snapshot-v2\0");
    let mut bytes = 0_u64;
    // Bound intermediate hashes and open handles, even for a million-entry tree.
    for batch in entries.chunks(512) {
        let hashes: Vec<io::Result<Fingerprint>> = pool.install(|| {
            batch
                .par_iter()
                .map(|entry| fingerprint(root, entry, progress))
                .collect()
        });
        for hash in hashes {
            let hash = hash?;
            digest.update(hash.digest);
            bytes = bytes
                .checked_add(hash.bytes)
                .ok_or_else(|| io::Error::other("snapshot byte count overflow"))?;
        }
    }
    // Recheck every directory after the entire content pass: its children may
    // have changed while another file was being hashed.
    let checks: io::Result<()> = pool.install(|| {
        entries
            .par_iter()
            .filter(|entry| entry.directory_identity.is_some())
            .try_for_each(|entry| {
                let file = open_no_follow(&entry.path, OpenKind::Directory)?;
                let metadata = file.metadata()?;
                if !metadata.is_dir()
                    || crate::scanner::metadata_is_link_or_reparse(&metadata)
                    || metadata_digest(&metadata)? != metadata_digest(&entry.metadata)?
                    || Some(identity(&file)?) != entry.directory_identity
                {
                    return Err(io::Error::other("directory changed while taking snapshot"));
                }
                Ok(())
            })
    });
    checks?;
    Ok(TreeSnapshot {
        sha256: format!("{:x}", digest.finalize()),
        entries: entries.len() as u64,
        bytes,
    })
}

fn inventory(root: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        if crate::scanner::metadata_is_link_or_reparse(&metadata) {
            return Err(io::Error::other(
                "snapshot refuses links and reparse points",
            ));
        }
        let directory_identity = if metadata.is_dir() {
            let file = open_no_follow(&path, OpenKind::Directory)?;
            let opened = file.metadata()?;
            if !opened.is_dir()
                || crate::scanner::metadata_is_link_or_reparse(&opened)
                || metadata_digest(&opened)? != metadata_digest(&metadata)?
            {
                return Err(io::Error::other("directory changed while taking snapshot"));
            }
            let mut children = fs::read_dir(&path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()?;
            children.sort();
            pending.extend(children.into_iter().rev());
            Some(identity(&file)?)
        } else if metadata.is_file() {
            None
        } else {
            return Err(io::Error::other("snapshot refuses special files"));
        };
        entries.push(Entry {
            path,
            metadata,
            directory_identity,
        });
    }
    Ok(entries)
}

fn fingerprint(
    root: &Path,
    entry: &Entry,
    progress: Option<&SnapshotProgress>,
) -> io::Result<Fingerprint> {
    let mut digest = Sha256::new();
    let name = entry
        .path
        .strip_prefix(root)
        .map_err(io::Error::other)?
        .as_os_str()
        .as_encoded_bytes();
    digest.update((name.len() as u64).to_le_bytes());
    digest.update(name);
    digest.update([u8::from(entry.metadata.is_dir())]);
    let metadata = metadata_digest(&entry.metadata)?;
    digest.update(&metadata);
    let (id, bytes) = if let Some(id) = entry.directory_identity {
        (id, 0)
    } else {
        let mut file = open_no_follow(&entry.path, OpenKind::FileContents)?;
        let opened = file.metadata()?;
        if !opened.is_file()
            || crate::scanner::metadata_is_link_or_reparse(&opened)
            || metadata_digest(&opened)? != metadata
        {
            return Err(io::Error::other("file changed while taking snapshot"));
        }
        let id = identity(&file)?;
        let mut content = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut bytes = 0_u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            content.update(&buffer[..count]);
            bytes = bytes
                .checked_add(count as u64)
                .ok_or_else(|| io::Error::other("snapshot byte count overflow"))?;
            if let Some(progress) = progress {
                progress.bytes.fetch_add(count as u64, Ordering::Relaxed);
            }
        }
        if bytes != opened.len() || metadata_digest(&file.metadata()?)? != metadata {
            return Err(io::Error::other("file changed while taking snapshot"));
        }
        // Handle metadata alone cannot detect an atomically replaced name.
        let after = open_no_follow(&entry.path, OpenKind::FileMetadata)?;
        let after_meta = after.metadata()?;
        if !after_meta.is_file()
            || crate::scanner::metadata_is_link_or_reparse(&after_meta)
            || metadata_digest(&after_meta)? != metadata
            || identity(&after)? != id
        {
            return Err(io::Error::other("path changed while taking snapshot"));
        }
        digest.update(content.finalize());
        if let Some(progress) = progress {
            progress.files.fetch_add(1, Ordering::Relaxed);
        }
        (id, bytes)
    };
    digest.update(id.volume.to_le_bytes());
    digest.update(id.index.to_le_bytes());
    Ok(Fingerprint {
        digest: digest.finalize().into(),
        bytes,
    })
}

fn open_no_follow(path: &Path, kind: OpenKind) -> io::Result<File> {
    let directory = matches!(kind, OpenKind::Directory);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        };
        // Identity/metadata probes need no content access. Asking for GENERIC_READ
        // unnecessarily engages content filters and fails more access checks.
        if !matches!(kind, OpenKind::FileContents) {
            options.access_mode(FILE_READ_ATTRIBUTES);
        }
        options.custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            libc::O_NOFOLLOW
                | if directory {
                    libc::O_DIRECTORY
                } else {
                    libc::O_NONBLOCK
                },
        );
    }
    options.open(path)
}

#[cfg(windows)]
fn identity(file: &File) -> io::Result<Identity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the borrowed File keeps this handle live; info is valid writable storage.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Identity {
        volume: u64::from(info.dwVolumeSerialNumber),
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

#[cfg(unix)]
fn identity(file: &File) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(Identity {
        volume: metadata.dev(),
        index: metadata.ino(),
    })
}

fn metadata_digest(meta: &Metadata) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity(17);
    data.extend_from_slice(&meta.len().to_le_bytes());
    data.push(u8::from(meta.permissions().readonly()));
    let mut time = DefaultHasher::new();
    meta.modified()?.hash(&mut time);
    if let Ok(created) = meta.created() {
        created.hash(&mut time);
    }
    data.extend_from_slice(&time.finish().to_le_bytes());
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_counts_produce_identical_snapshots_and_exact_progress() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("empty")).unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        for index in 0..600 {
            fs::write(
                root.path().join("nested").join(format!("{index}.bin")),
                [index as u8; 128],
            )
            .unwrap();
        }
        fs::write(root.path().join("large.bin"), vec![42; 150_000]).unwrap();
        let mut expected = None;
        for workers in [1, 2, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .build()
                .unwrap();
            let counters = SnapshotProgress::default();
            let snapshot = capture_in_pool(root.path(), Some(&counters), &pool).unwrap();
            assert_eq!(snapshot.entries, 604);
            assert_eq!(snapshot.bytes, 600 * 128 + 150_000);
            assert_eq!(counters.counts(), (601, snapshot.bytes));
            if let Some(expected) = &expected {
                assert_eq!(&snapshot, expected);
            }
            expected = Some(snapshot);
        }
    }

    #[test]
    fn empty_directories_and_zero_byte_files_are_distinct() {
        let root = tempfile::tempdir().unwrap();
        let empty = capture(root.path()).unwrap();
        assert_eq!((empty.entries, empty.bytes), (1, 0));
        fs::create_dir(root.path().join("empty")).unwrap();
        let directory = capture(root.path()).unwrap();
        assert_ne!(directory, empty);
        fs::write(root.path().join("zero"), []).unwrap();
        let file = capture(root.path()).unwrap();
        assert_ne!(file, directory);
        assert_eq!((file.entries, file.bytes), (3, 0));
    }

    #[test]
    fn inventory_to_open_changes_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file");
        fs::write(&path, b"old").unwrap();
        let entries = inventory(&path).unwrap();
        fs::write(&path, b"longer replacement").unwrap();
        assert!(fingerprint(&path, &entries[0], None).is_err());
        fs::remove_file(&path).unwrap();
        assert!(fingerprint(&path, &entries[0], None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_special_files_are_refused_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), b"protected").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
        assert!(capture(root.path()).is_err());
        assert!(open_no_follow(&root.path().join("link"), OpenKind::Directory).is_err());
        fs::remove_file(root.path().join("link")).unwrap();
        let socket = std::os::unix::net::UnixListener::bind(root.path().join("socket")).unwrap();
        assert!(capture(root.path()).is_err());
        drop(socket);
        assert!(outside.path().join("keep").exists());
    }
}
