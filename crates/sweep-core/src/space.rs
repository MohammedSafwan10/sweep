//! Identify the filesystem containing a clean target for before/after space
//! measurements. These measurements are observations, not deletion receipts.

use std::path::{Path, PathBuf};

#[cfg(windows)]
pub(crate) fn volume_root(path: &Path) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;

    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut output = vec![0u16; 32_768];
    // SAFETY: Both pointers refer to live, NUL-terminated/writable buffers;
    // the output length is the actual allocated number of UTF-16 code units.
    let ok =
        unsafe { GetVolumePathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    if ok == 0 {
        return None;
    }
    let end = output.iter().position(|&unit| unit == 0)?;
    Some(PathBuf::from(OsString::from_wide(&output[..end])))
}

#[cfg(unix)]
pub(crate) fn volume_root(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;

    let mut current = if path.is_dir() { path } else { path.parent()? };
    let device = std::fs::metadata(current).ok()?.dev();
    while let Some(parent) = current.parent() {
        if std::fs::metadata(parent).ok()?.dev() != device {
            break;
        }
        current = parent;
    }
    Some(current.to_path_buf())
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn volume_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().last().map(Path::to_path_buf)
}
